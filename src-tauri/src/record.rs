//! 录音编排：采集队列 → 重采样 → 识别会话 → 出字。
//!
//! 这一层是**纯逻辑**，不认识 Tauri：结果通过 `emit` 回调交出去，
//! 所以能被单测直接调用（本文件末尾的测试就是这么用的）。
//! Tauri 那层（事件、命令）在 lib.rs 里。
//!
//! 名字用 record 而不是 session：asr.rs 里的 `OfflineSession` / `StreamingSession`
//! 是「识别会话」，这里是「录音怎么跑」的编排，两回事。

use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(test)]
use std::sync::mpsc;
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::Duration;

use crate::asr;
use crate::asr::{OfflineEvent, StreamEvent, StreamingSession, Transcript};
use crate::engine::{Engine, PunctuationEngine, StreamingEngine};
use sherpa_onnx::LinearResampler;

use crate::audio;
use crate::log;
pub struct RecordingRun {
    pub stop: Arc<AtomicBool>,
    /// 取消录音：收尾时不再产出文本（结果框里不该冒出被取消的内容）。
    pub discard: Arc<AtomicBool>,
    pub rx: Receiver<Transcript>,
}

pub struct FinishedRecording {
    pub transcript: Transcript,
    pub capture_error: Option<String>,
}

/// 停止时等待识别线程收尾失败：调用方据此换成对应模式的超时文案。
pub const STOP_FAILED: &str = "record.stopFailed";

/// 界面能接受的最低音频时长（秒）。
pub const MIN_TRANSCRIPT_SECONDS: f32 = 0.2;

/// 这段转写能不能交给界面：太短或空文本都返回字典错误码。
///
/// 流式和整段走同一套判定 —— 两条路径的文案必须一致，
/// 否则一边显示「至少说 0.2 秒」、另一边显示「至少说 {seconds} 秒」。
pub fn ensure_transcript_usable(text: &str, duration_seconds: f32) -> Result<(), String> {
    if duration_seconds < MIN_TRANSCRIPT_SECONDS {
        return Err(crate::code_detail("record.tooShort", MIN_TRANSCRIPT_SECONDS));
    }
    if text.trim().is_empty() {
        return Err(crate::code("record.empty"));
    }
    Ok(())
}

/// 等识别线程交回结果：超时或通道关闭都算失败，不返回半截结果。
fn wait_transcript(
    rx: &Receiver<Transcript>,
    discard: &AtomicBool,
    timeout_secs: u64,
) -> Result<Transcript, String> {
    rx.recv_timeout(Duration::from_secs(timeout_secs))
        .map_err(|_| {
            discard.store(true, Ordering::SeqCst);
            crate::code(STOP_FAILED)
        })
}

/// 采集故障不等于取消：先让识别线程收尾，保住已有的转写。
pub fn finish_recording(
    recorder: audio::Recorder,
    run: RecordingRun,
    timeout_secs: u64,
) -> Result<FinishedRecording, String> {
    let stopped = recorder.stop();
    run.stop.store(true, Ordering::SeqCst);
    let discard = run.discard.clone();
    let transcript = wait_transcript(&run.rx, &discard, timeout_secs)?;
    if let Err(error) = stopped {
        if transcript.text.trim().is_empty() {
            return Err(error);
        }
        log::warn(format!("麦克风异常，保留已完成的转写：{error}"));
        return Ok(FinishedRecording {
            transcript,
            capture_error: Some(error),
        });
    }
    Ok(FinishedRecording {
        transcript,
        capture_error: None,
    })
}

/// 一段录音的统计：音频时长、识别耗时、RTF、字数。
///
/// RTF = 识别耗时 / 音频时长（小于 1 表示比实时快）。
pub fn log_transcript(kind: &str, transcript: &Transcript) {
    let audio_seconds = transcript.duration_seconds.max(f32::EPSILON);
    let rtf = transcript.elapsed_ms as f64 / 1000.0 / audio_seconds as f64;
    log::info(format!(
        "录音结束（{kind}）：时长 {:.1}s，耗时 {}ms，RTF {:.2}，字数 {}",
        transcript.duration_seconds,
        transcript.elapsed_ms,
        rtf,
        transcript.text.chars().count()
    ));
}

/// 记一句识别结果：只记边界和字数，不记内容 —— 怀疑「录音丢了」时能看出句子在不在、
/// 覆盖了哪段时间（配合总时长就能算出哪一段没被覆盖）。
pub fn log_segment(tag: &str, segment: &asr::Segment) {
    log::info(format!(
        "{tag}：{:.1}s → {:.1}s，{} 字",
        segment.start,
        segment.end,
        segment.text.chars().count()
    ));
}

/// 记流式识别出的句子（整段模式同款日志）。
///
/// 缺口占位符不算句子 —— 它是「这里丢了音频」的标记，不是识别结果。
fn log_stream_events(tag: &str, events: &[StreamEvent]) {
    for event in events {
        if let StreamEvent::Segment(segment) = event {
            if !audio::is_gap_marker(&segment.text) {
                log_segment(tag, segment);
            }
        }
    }
}

/// 整段模式：边录音边按句识别。
///
/// 采集线程把新采样放进队列，这里取走、重采样到 16k、喂给 VAD 切句，
/// 切出一句就立刻送 SenseVoice 识别并把结果推给界面。
/// 识别完的音频立刻丢掉，只留文本 —— 内存不随录音时长增长。
pub fn run_offline<F>(
    engine: Arc<Engine>,
    vad: sherpa_onnx::VadModelConfig,
    queue: audio::SampleQueue,
    rate: u32,
    stop: Arc<AtomicBool>,
    discard: Arc<AtomicBool>,
    mut emit: F,
) -> Transcript
where
    F: FnMut(OfflineEvent),
{
    let resampler = if rate as i32 == asr::SAMPLE_RATE {
        None
    } else {
        sherpa_onnx::LinearResampler::create(rate as i32, asr::SAMPLE_RATE)
    };
    let mut session = asr::OfflineSession::new(engine.clone(), vad);
    // 电平自动增益：偏小的录音会整段被 VAD 当成非语音丢掉（见 audio::Agc）。
    let mut agc = audio::Agc::new();
    // 识别跟不上时丢掉的音频：在结果里留个可见的缺口（按时间插进句子列表）。
    let mut gaps: Vec<asr::Segment> = Vec::new();
    // 已经喂给识别器的 16 kHz 采样数（给缺口标记标时间；不能用设备采样率）
    let mut fed_16k = 0usize;
    let discarded = || discard.load(Ordering::Relaxed);

    loop {
        let new_audio = queue.take_new();
        if new_audio.dropped > 0 {
            // 识别跟不上：丢掉了一段音频，要在结果里留下可见的缺口。
            // 标记既进最终文本，也作为一句实时发出去（前端追加后才看得到）。
            let seconds = audio::gap_seconds(new_audio.dropped, rate);
            log::warn(format!("离线识别跟不上，丢掉 {seconds} 秒音频"));
            let marker = audio::gap_marker(new_audio.dropped, rate);
            let at = fed_16k as f32 / asr::SAMPLE_RATE as f32;
            gaps.push(asr::Segment {
                text: marker.clone(),
                start: at,
                end: at,
            });
            if !discarded() {
                emit(OfflineEvent::Segment(asr::Segment {
                    text: marker,
                    start: at,
                    end: at,
                }));
            }
        }
        if !new_audio.samples.is_empty() {
            let mut samples = match &resampler {
                Some(resampler) => resampler.resample(&new_audio.samples, false),
                None => new_audio.samples,
            };
            // 提电平要在 VAD 之前 —— VAD 判的就是这段音频。
            agc.process(&mut samples);
            fed_16k += samples.len();
            for event in session.push(&samples) {
                let OfflineEvent::Segment(segment) = event;
                log_segment("识别出一句", &segment);
                if !discarded() {
                    emit(OfflineEvent::Segment(segment));
                }
            }
        }
        if stop.load(Ordering::Relaxed) || discarded() {
            break;
        }
        std::thread::sleep(Duration::from_millis(60));
    }

    // 取消录音：不冲 VAD 尾巴、不识别 —— 被取消的内容不该再出现在结果里。
    if !discarded() {
        // 收尾取样：循环里是「取音频 → 识别 → 看停止标志」，停止标志在识别期间
        // 被置位时循环会直接跳出，那段「识别期间麦克风采到」的音频就永远留在
        // 队列里 —— 必须补这一把，否则句尾会被切掉（流式路径也有同样的收尾）。
        let last = queue.take_new();
        if last.dropped > 0 {
            // 识别跟不上：丢掉了一段音频，要在结果里留下可见的缺口。
            let seconds = audio::gap_seconds(last.dropped, rate);
            log::warn(format!("离线收尾丢掉 {seconds} 秒音频"));
            let marker = audio::gap_marker(last.dropped, rate);
            let at = fed_16k as f32 / asr::SAMPLE_RATE as f32;
            gaps.push(asr::Segment {
                text: marker.clone(),
                start: at,
                end: at,
            });
            emit(OfflineEvent::Segment(asr::Segment {
                text: marker,
                start: at,
                end: at,
            }));
        }
        let mut samples = match &resampler {
            Some(resampler) => resampler.resample(&last.samples, true),
            None => last.samples,
        };
        if !samples.is_empty() {
            agc.process(&mut samples);
            for event in session.push(&samples) {
                let OfflineEvent::Segment(segment) = event;
                log_segment("收尾识别出一句", &segment);
                emit(OfflineEvent::Segment(segment));
            }
        }
        for event in session.finish() {
            let OfflineEvent::Segment(segment) = event;
            log_segment("收尾识别出一句", &segment);
            emit(OfflineEvent::Segment(segment));
        }
    }

    // 文本和 segments 都从同一份句子列表来（缺口标记按时间插进去）——
    // 和流式路径同一个做法，两种模式的 segments 语义才一致。
    let mut transcript = Transcript {
        text: asr::join_segments(session.segments().iter().map(|s| s.text.as_str())),
        segments: session.segments().to_vec(),
        duration_seconds: session.recorded_samples() as f32 / asr::SAMPLE_RATE as f32,
        elapsed_ms: session.elapsed_ms(),
        language: engine.language.clone(),
        warning: None,
    };
    asr::insert_gaps(&mut transcript, gaps);
    transcript
}

/// 判一次 VAD 并立刻清空队列。
///
/// 这里只拿 VAD 做门控，不消费识别句段 —— 判完必须 clear()，
/// 否则已切出的音频会一直压在队列里，越攒越多。
fn vad_is_speech(detector: &sherpa_onnx::VoiceActivityDetector, samples: &[f32]) -> bool {
    for window in samples.chunks(512) {
        detector.accept_waveform(window);
    }
    let detected = detector.detected();
    // 取完状态立即释放已切出的音频：门控不需要那些句段。
    detector.clear();
    detected
}

/// 边录音边喂流式识别器；每 60ms 取一次新采样，端点处提交一句。
pub fn run_streaming_core<F>(
    engine: Arc<StreamingEngine>,
    punctuator: Option<Arc<PunctuationEngine>>,
    queue: audio::SampleQueue,
    rate: u32,
    stop: Arc<AtomicBool>,
    discard: Arc<AtomicBool>,
    mut emit: F,
) -> Transcript
where
    F: FnMut(Vec<StreamEvent>),
{
    let vad_config = engine.vad().cloned();
    let mut session = StreamingSession::new(engine, punctuator);
    let resampler = if rate as i32 == asr::SAMPLE_RATE {
        None
    } else {
        LinearResampler::create(rate as i32, asr::SAMPLE_RATE)
    };
    let vad = vad_config
        .as_ref()
        .and_then(|config| sherpa_onnx::VoiceActivityDetector::create(config, 60.0));
    // 电平自动增益：偏小的录音会被门控 VAD 整段丢掉（见 audio::Agc）。
    let mut agc = audio::Agc::new();
    // 说话起点前留 0.4s，避免 VAD 起音延迟把字头切掉
    let preroll_limit = (asr::SAMPLE_RATE as usize) / 5 * 2;
    let mut preroll: Vec<f32> = Vec::new();
    let mut active = false;
    // 识别跟不上时丢掉的音频，在结果里留个可见的缺口
    let mut gaps: Vec<asr::Segment> = Vec::new();
    let mut last_voice = std::time::Instant::now();
    while !stop.load(Ordering::Relaxed) && !discard.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(60));
        let new_audio = queue.take_new();
        // 丢掉的输入同样占真实录音时间：缺口结束后才是后续音频的位置。
        session.advance_samples(audio::resampled_len(new_audio.dropped, rate));
        let at = session.elapsed_seconds();
        if new_audio.dropped > 0 {
            // 识别跟不上：丢掉了一段音频，标记出来，不能静默丢
            let seconds = audio::gap_seconds(new_audio.dropped, rate);
            log::warn(format!("流式识别跟不上，丢掉 {seconds} 秒音频"));
            let marker = audio::gap_marker(new_audio.dropped, rate);
            gaps.push(asr::Segment {
                text: marker.clone(),
                start: at,
                end: at,
            });
            if !discard.load(Ordering::Relaxed) {
                emit(vec![StreamEvent::Segment(asr::Segment {
                    text: marker,
                    start: at,
                    end: at,
                })]);
            }
        }
        let chunk = new_audio.samples;
        if chunk.is_empty() {
            continue;
        }
        let mut samples = match &resampler {
            Some(resampler) => resampler.resample(&chunk, false),
            None => chunk,
        };
        if samples.is_empty() {
            continue;
        }
        // 提电平要在 VAD 之前 —— 流式路径的门控 VAD 判的就是这段音频，
        // 偏小的电平会让它整段判成非语音（见 audio::Agc）。
        agc.process(&mut samples);
        session.advance_samples(samples.len());

        let Some(detector) = vad.as_ref() else {
            if !discard.load(Ordering::Relaxed) {
                let events = session.push(&samples);
                log_stream_events("流式识别出一句", &events);
                emit(events);
            }
            continue;
        };

        let detected = vad_is_speech(detector, &samples);
        if detected {
            last_voice = std::time::Instant::now();
        }
        let speaking = detected || last_voice.elapsed() < Duration::from_millis(700);

        if speaking {
            if !active {
                active = true;
                if !preroll.is_empty() && !discard.load(Ordering::Relaxed) {
                    let events = session.push(&std::mem::take(&mut preroll));
                    log_stream_events("流式识别出一句", &events);
                    emit(events);
                }
            }
            if !discard.load(Ordering::Relaxed) {
                let events = session.push(&samples);
                log_stream_events("流式识别出一句", &events);
                emit(events);
            }
        } else {
            if active {
                active = false;
                // 静音开始：立刻收尾当前句子，避免模型在噪声上继续编。
                if !discard.load(Ordering::Relaxed) {
                    let events = session.force_end();
                    log_stream_events("流式识别出一句", &events);
                    emit(events);
                }
            }
            preroll.extend_from_slice(&samples);
            let excess = preroll.len().saturating_sub(preroll_limit);
            if excess > 0 {
                preroll.drain(..excess);
            }
        }
    }

    // 取消录音：不冲尾巴、不出字。
    if discard.load(Ordering::Relaxed) {
        return Transcript {
            text: String::new(),
            segments: Vec::new(),
            duration_seconds: 0.0,
            elapsed_ms: 0,
            language: "zh-en".into(),
            warning: None,
        };
    }

    // 收尾：把剩余采样和重采样器内部缓冲一起冲出去
    let last = queue.take_new();
    session.advance_samples(audio::resampled_len(last.dropped, rate));
    if last.dropped > 0 {
        let seconds = audio::gap_seconds(last.dropped, rate);
        log::warn(format!("流式收尾丢掉 {seconds} 秒音频"));
        let marker = audio::gap_marker(last.dropped, rate);
        let at = session.elapsed_seconds();
        gaps.push(asr::Segment {
            text: marker.clone(),
            start: at,
            end: at,
        });
        emit(vec![StreamEvent::Segment(asr::Segment {
            text: marker,
            start: at,
            end: at,
        })]);
    }
    let chunk = last.samples;
    let tail = match &resampler {
        Some(resampler) => resampler.resample(&chunk, true),
        None => chunk,
    };
    if !tail.is_empty() {
        // 循环退出后才采到的尾包也占真实录音时长。
        session.advance_samples(tail.len());
        // 尾部即使仍被门控判为语音，也保持原有识别行为。
        let events = session.push(&tail);
        log_stream_events("流式收尾识别出一句", &events);
        emit(events);
    }
    let events = session.finish();
    log_stream_events("流式收尾识别出一句", &events);
    emit(events);
    let mut transcript = session.transcript();
    asr::insert_gaps(&mut transcript, gaps);
    transcript
}

#[cfg(test)]
mod tests;
