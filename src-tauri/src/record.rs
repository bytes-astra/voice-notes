//! 录音编排：采集队列 → 重采样 → 识别会话 → 出字。
//!
//! 这一层是**纯逻辑**，不认识 Tauri：结果通过 `emit` 回调交出去，
//! 所以能被单测直接调用（本文件末尾的测试就是这么用的）。
//! Tauri 那层（事件、命令）在 lib.rs 里。
//!
//! 名字用 record 而不是 session：asr.rs 里的 `OfflineSession` / `StreamingSession`
//! 是「识别会话」，这里是「录音怎么跑」的编排，两回事。

use std::sync::atomic::{AtomicBool, Ordering};
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
pub struct StreamSession {
    pub stop: Arc<AtomicBool>,
    /// 取消录音：收尾时不再产出文本（结果框里不该冒出被取消的内容）。
    pub discard: Arc<AtomicBool>,
    pub rx: Receiver<Transcript>,
}

/// 取走新到的采样（消费式：取走即释放，内存不随录音时长增长）。
pub fn take_new(queue: &audio::SampleQueue) -> audio::NewAudio {
    queue.take_new()
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

/// 整段模式的识别线程。
pub struct OfflineRun {
    pub stop: Arc<AtomicBool>,
    /// 取消录音：收尾时不再识别、不再发文本。
    pub discard: Arc<AtomicBool>,
    pub rx: mpsc::Receiver<Transcript>,
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
    let mut parts: Vec<String> = Vec::new();
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
            parts.push(marker.clone());
            let at = fed_16k as f32 / asr::SAMPLE_RATE as f32;
            if !discarded() {
                emit(OfflineEvent::Segment(asr::Segment {
                    text: marker,
                    start: at,
                    end: at,
                }));
            }
        }
        if !new_audio.samples.is_empty() {
            let samples = match &resampler {
                Some(resampler) => resampler.resample(&new_audio.samples, false),
                None => new_audio.samples,
            };
            fed_16k += samples.len();
            for event in session.push(&samples) {
                let OfflineEvent::Segment(segment) = event;
                parts.push(segment.text.clone());
                // 只记边界和字数，不记内容：怀疑「录音丢了」时能看出句子在不在、
                // 覆盖了哪段时间（配合总时长就能算出哪一段没被覆盖）。
                log::info(format!(
                    "识别出一句：{:.1}s → {:.1}s，{} 字",
                    segment.start,
                    segment.end,
                    segment.text.chars().count()
                ));
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
        for event in session.finish() {
            let OfflineEvent::Segment(segment) = event;
            parts.push(segment.text.clone());
            log::info(format!(
                "收尾识别出一句：{:.1}s → {:.1}s，{} 字",
                segment.start,
                segment.end,
                segment.text.chars().count()
            ));
            emit(OfflineEvent::Segment(segment));
        }
    }

    // 文本用 parts 拼（含缺口标记）；segments 只放真正的句子
    Transcript {
        text: asr::join_segments(parts.iter().map(|s| s.as_str())),
        segments: session.segments().to_vec(),
        duration_seconds: session.recorded_samples() as f32 / asr::SAMPLE_RATE as f32,
        elapsed_ms: session.elapsed_ms(),
        language: engine.language.clone(),
    }
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
    // 说话起点前留 0.4s，避免 VAD 起音延迟把字头切掉
    let preroll_limit = (asr::SAMPLE_RATE as usize) / 5 * 2;
    let mut preroll: Vec<f32> = Vec::new();
    let mut active = false;
    // 已经喂给识别器的 16 kHz 采样数（给缺口标记标时间；不能用设备采样率）
    let mut fed_16k = 0usize;
    // 识别跟不上时丢掉的音频，在结果里留个可见的缺口
    let mut gaps: Vec<asr::Segment> = Vec::new();
    let mut last_voice = std::time::Instant::now();
    while !stop.load(Ordering::Relaxed) && !discard.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(60));
        let new_audio = take_new(&queue);
        let at = fed_16k as f32 / asr::SAMPLE_RATE as f32;
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
        let samples = match &resampler {
            Some(resampler) => resampler.resample(&chunk, false),
            None => chunk,
        };
        if samples.is_empty() {
            continue;
        }
        fed_16k += samples.len();

        let Some(detector) = vad.as_ref() else {
            if !discard.load(Ordering::Relaxed) {
                emit(session.push(&samples));
            }
            continue;
        };

        for window in samples.chunks(512) {
            detector.accept_waveform(window);
        }
        let detected = detector.detected();
        if detected {
            last_voice = std::time::Instant::now();
        }
        let speaking = detected || last_voice.elapsed() < Duration::from_millis(700);

        if speaking {
            if !active {
                active = true;
                if !preroll.is_empty() && !discard.load(Ordering::Relaxed) {
                    emit(session.push(&std::mem::take(&mut preroll)));
                }
            }
            if !discard.load(Ordering::Relaxed) {
                emit(session.push(&samples));
            }
        } else {
            if active {
                active = false;
                // 静音开始：立刻收尾当前句子，避免模型在噪声上继续编。
                if !discard.load(Ordering::Relaxed) {
                    emit(session.force_end());
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
        };
    }

    // 收尾：把剩余采样和重采样器内部缓冲一起冲出去
    let last = take_new(&queue);
    if last.dropped > 0 {
        let seconds = audio::gap_seconds(last.dropped, rate);
        log::warn(format!("流式收尾丢掉 {seconds} 秒音频"));
        let marker = audio::gap_marker(last.dropped, rate);
        let at = fed_16k as f32 / asr::SAMPLE_RATE as f32;
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
        emit(session.push(&tail));
    }
    emit(session.finish());
    let mut transcript = session.transcript();
    asr::insert_gaps(&mut transcript, gaps);
    transcript
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use crate::models;

        #[test]
        fn streams_from_buffer_with_resampling() {
            let Some(dir) = std::env::var("VOICE_NOTES_STREAMING_MODEL_DIR")
                .ok()
                .map(PathBuf::from)
            else {
                eprintln!("跳过：未设置 VOICE_NOTES_STREAMING_MODEL_DIR");
                return;
            };
            let paths = models::StreamingPaths {
                dir: dir.display().to_string(),
                encoder: dir.join("encoder.int8.onnx").display().to_string(),
                decoder: dir.join("decoder.onnx").display().to_string(),
                joiner: dir.join("joiner.int8.onnx").display().to_string(),
                tokens: dir.join("tokens.txt").display().to_string(),
                vad: [dir.join("silero_vad.onnx")]
                    .into_iter()
                    .find(|path| path.is_file())
                    .map(|path| path.display().to_string()),
            };
            let engine = Arc::new(StreamingEngine::new(&paths, &[]).expect("加载流式模型"));
            let wave = sherpa_onnx::Wave::read("tests/fixtures/zh.wav").expect("读取测试音频");

            // 模拟 48 kHz 设备：每个 16 kHz 样本重复三次。
            let mut upsampled = Vec::with_capacity(wave.samples().len() * 3);
            for sample in wave.samples() {
                upsampled.push(*sample);
                upsampled.push(*sample);
                upsampled.push(*sample);
            }
            let queue = audio::SampleQueue::new(48_000);
            queue.push(&upsampled);
            let stop = Arc::new(AtomicBool::new(false));
            let stopper = stop.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(400));
                stopper.store(true, Ordering::SeqCst);
            });

            let mut partials = 0usize;
            let transcript = run_streaming_core(
                engine,
                None,
                queue,
                48_000,
                stop,
                Arc::new(AtomicBool::new(false)),
                |events| {
                    for event in events {
                        if let StreamEvent::Partial(_) = event {
                            partials += 1;
                        }
                    }
                },
            );
            println!("流式结果：{}", transcript.text);
            assert!(!transcript.text.is_empty(), "流式结果为空");
            assert!(partials > 0, "没有中间结果");
            assert!(transcript.duration_seconds > 5.0);
        }

        /// 识别跟不上、丢掉一段音频时，结果里必须留一个可见的缺口标记。
        ///
        /// 回归测试：之前流式路径完全忽略了 `dropped`，音频被静默丢弃。
        #[test]
        fn streaming_gap_marker_reaches_transcript() {
            let Some(dir) = std::env::var("VOICE_NOTES_STREAMING_MODEL_DIR")
                .ok()
                .map(PathBuf::from)
            else {
                eprintln!("跳过：未设置 VOICE_NOTES_STREAMING_MODEL_DIR");
                return;
            };
            let paths = models::StreamingPaths {
                dir: dir.display().to_string(),
                encoder: dir.join("encoder.int8.onnx").display().to_string(),
                decoder: dir.join("decoder.onnx").display().to_string(),
                joiner: dir.join("joiner.int8.onnx").display().to_string(),
                tokens: dir.join("tokens.txt").display().to_string(),
                vad: [dir.join("silero_vad.onnx")]
                    .into_iter()
                    .find(|path| path.is_file())
                    .map(|path| path.display().to_string()),
            };
            let engine = Arc::new(StreamingEngine::new(&paths, &[]).expect("加载流式模型"));
            let wave = sherpa_onnx::Wave::read("tests/fixtures/zh.wav").expect("读取测试音频");
            let mut upsampled = Vec::with_capacity(wave.samples().len() * 3);
            for sample in wave.samples() {
                upsampled.push(*sample);
                upsampled.push(*sample);
                upsampled.push(*sample);
            }
            // 上限调到 1000 个采样：音频远超上限 → 一定会触发丢弃
            let queue = audio::SampleQueue::with_limit(Some(1_000));
            queue.push(&upsampled);
            let stop = Arc::new(AtomicBool::new(false));
            let stopper = stop.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(400));
                stopper.store(true, Ordering::SeqCst);
            });

            let transcript = run_streaming_core(
                engine,
                None,
                queue,
                48_000,
                stop,
                Arc::new(AtomicBool::new(false)),
                |_| {},
            );
            println!("带缺口的流式结果：{}", transcript.text);
            assert!(
                transcript.text.contains("[[gap:"),
                "丢掉音频后结果里应该有缺口标记: {}",
                transcript.text
            );
            assert!(
                transcript.segments.iter().any(|s| s.text.contains("[[gap:")),
                "segments 里也该有缺口标记"
            );
        }

        /// 整段模式：丢掉音频后，缺口标记必须出现在最终文本里。
        ///
        /// 回归测试：之前标记只 push 进了本地 parts，最终文本却从 session.segments()
        /// 拼，标记被丢掉了。
        #[test]
        fn offline_gap_marker_reaches_transcript() {
            let Some(dir) = std::env::var("VOICE_NOTES_MODEL_DIR")
                .ok()
                .map(PathBuf::from)
            else {
                eprintln!("跳过：未设置 VOICE_NOTES_MODEL_DIR");
                return;
            };
            let model = [dir.join("model.int8.onnx"), dir.join("model.onnx")]
                .into_iter()
                .find(|path| path.is_file());
            let (Some(model), tokens) = (model, dir.join("tokens.txt")) else {
                eprintln!("跳过：找不到 SenseVoice 模型");
                return;
            };
            if !tokens.is_file() {
                eprintln!("跳过：找不到 tokens.txt");
                return;
            }
            let mut vad_candidates = vec![dir.join("silero_vad.onnx")];
            if let Some(parent) = dir.parent() {
                vad_candidates.push(parent.join("silero_vad.onnx"));
            }
            let vad_model = vad_candidates.into_iter().find(|path| path.is_file());
            let Some(vad_model) = vad_model else {
                eprintln!("跳过：找不到 VAD 模型");
                return;
            };
            let paths = models::ModelPaths {
                dir: dir.display().to_string(),
                model: model.display().to_string(),
                tokens: tokens.display().to_string(),
                vad: Some(vad_model.display().to_string()),
                source: "test".into(),
            };
            let engine = Arc::new(Engine::new(&paths, "zh", true).expect("加载模型"));
            let wave = sherpa_onnx::Wave::read("tests/fixtures/zh.wav").expect("读取测试音频");

            // 上限调到 1000 个采样：音频远超上限 → 一定会触发丢弃
            let queue = audio::SampleQueue::with_limit(Some(1_000));
            queue.push(wave.samples());
            let stop = Arc::new(AtomicBool::new(true));

            let vad = engine.vad_config().expect("VAD 配置");
            let mut events = Vec::new();
            let transcript = run_offline(
                engine,
                vad,
                queue,
                16_000,
                stop,
                Arc::new(AtomicBool::new(false)),
                |event| events.push(event),
            );
            println!("带缺口的整段结果：{}", transcript.text);
            assert!(
                transcript.text.contains("[[gap:"),
                "丢掉音频后文本里应该有缺口标记: {}",
                transcript.text
            );
            assert!(
                events.iter().any(|e| match e {
                    OfflineEvent::Segment(s) => s.text.contains("[[gap:"),
                }),
                "也应该实时发一个缺口事件"
            );
        }
}
