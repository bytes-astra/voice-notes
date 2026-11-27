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
use std::sync::Mutex;
use std::time::Duration;

use crate::asr;
use crate::asr::{AsrSession, OfflineEvent, StreamEvent, StreamingSession, Transcript};
use crate::engine::{PunctuationEngine, StreamingEngine};
use crate::error_code::ErrorCode;
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

/// 一次录音跑的是哪种模式。
///
/// **只是标签**：超时秒数、失败文案、云端那套「断连保句」策略都由调用方
/// （app 层）按它分派 —— 这一层不认识界面文案。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionMode {
    /// 本地整段（SenseVoice）—— 音频不出本机。
    LocalOffline,
    /// 本地流式（Zipformer）—— 音频不出本机。
    LocalStreaming,
    /// 云端流式 —— 音频会传出去。
    CloudStreaming,
}

/// 一次正在进行的录音：采集器 + 识别会话的停止信号 + 模式。
///
/// **采集器与它的 `RecordingRun` 必须同行** —— 只持有其一，识别线程就永远
/// 收不到 stop（线程泄漏、永久占着识别模型）、麦克风不关、语言/ITN 永久锁死。
/// 装进**一个值**里，这个配对就由类型保证，而不是靠每个 `start_*` 各自记得
/// 写两个槽（三个会话槽类型完全相同，写错编译不报错）。
///
/// 认领的顺序（`Recorder::stop` 不取走队列里的采样 —— 尾巴由识别线程自己
/// `take_new()` 取）：**先停采集，再让线程收尾**。这个顺序只允许出现在
/// [`finish_recording`] 一处 —— `Session::finish` 调它，不自己重写。
pub struct Session {
    /// 采集器（麦克风流）。`None` = 已经被收尾流程取走。
    recorder: Option<audio::Recorder>,
    /// 识别线程的停止信号 + 结果通道。`None` = 已经被收尾流程取走。
    run: Option<RecordingRun>,
    mode: SessionMode,
}

impl Session {
    pub fn new(recorder: audio::Recorder, run: RecordingRun, mode: SessionMode) -> Self {
        Self {
            recorder: Some(recorder),
            run: Some(run),
            mode,
        }
    }

    /// 这一轮是哪种模式（app 层按它选超时与文案）。
    pub fn mode(&self) -> SessionMode {
        self.mode
    }

    /// 收尾：停采集 + 等识别线程交回结果。
    ///
    /// **顺序在 [`finish_recording`] 里**（先停采集、再停线程）—— 这里的
    /// 调用方拆不出「先停哪个」。
    pub fn finish(mut self, timeout_secs: u64) -> Result<FinishedRecording, String> {
        let recorder = self.recorder.take().expect("收尾前采集器还在");
        let run = self.run.take().expect("收尾前会话还在");
        finish_recording(recorder, run, timeout_secs)
    }

    /// 取消：置停止标志 + 关麦（不产出文本）。
    pub fn cancel(mut self) {
        if let Some(run) = self.run.take() {
            run.discard.store(true, Ordering::SeqCst);
            run.stop.store(true, Ordering::SeqCst);
        }
        if let Some(recorder) = self.recorder.take() {
            recorder.cancel();
        }
    }
}

impl Drop for Session {
    /// 兜底：**掉一个 `Session` 必须让识别线程退出**。
    ///
    /// `Recorder` 自己的 `Drop` 只发 `Cmd::Cancel` 关麦、**不置 `stop`**；
    /// 而 ASR 线程只看 `stop` / `discard`（不看 `rx` 是否关闭）→
    /// 不兜底就会「麦克风关了、线程空转到天荒地老」。
    fn drop(&mut self) {
        if let Some(run) = self.run.take() {
            run.discard.store(true, Ordering::SeqCst);
            run.stop.store(true, Ordering::SeqCst);
        }
    }
}

/// 登记一次录音：**一次写**，不留中间态。
///
/// 槽里已经有录音 → 报占用（上一轮没清干净，顶掉它的话上一轮的识别线程
/// 就永远收不到 stop）。
///
/// 装不进槽（锁被 poison）→ 报错，**槽里什么都没有**。
pub fn register_session<S>(session_slot: &Mutex<Option<S>>, session: S) -> Result<(), SlotError> {
    let mut guard = session_slot.lock().map_err(|_| SlotError::SessionLocked)?;
    if guard.is_some() {
        return Err(SlotError::AlreadyRecording);
    }
    *guard = Some(session);
    Ok(())
}

/// 两个槽登记失败的原因。
#[derive(Debug)]
pub enum SlotError {
    /// 会话槽的锁被 poison
    SessionLocked,
    /// 已经有录音在跑
    AlreadyRecording,
}

/// 这个错误是不是「停止录音时识别线程没交回结果」。
///
/// 比的是**错误码**部分（':' 之前），不是整个字符串 ——
/// `code_detail(ErrorCode::RecordStopFailed, …)` 出来的是
/// `record.stopFailed:细节`，整串比较会失配，超时文案就换不上了。
///
/// 反查走 [`ErrorCode::from_key`] —— 与 `Display` 共用同一张 key 表，
/// 不会漂。**带细节 / 不带细节都要认得出**。
pub fn is_stop_failed(error: &str) -> bool {
    let key = error.split(':').next().unwrap_or(error);
    ErrorCode::from_key(key) == Some(ErrorCode::RecordStopFailed)
}

/// 界面能接受的最低音频时长（秒）。
pub const MIN_TRANSCRIPT_SECONDS: f32 = 0.2;

/// 这段转写能不能交给界面：太短或空文本都返回字典错误码。
///
/// 流式和整段走同一套判定 —— 两条路径的文案必须一致，
/// 否则一边显示「至少说 0.2 秒」、另一边显示「至少说 {seconds} 秒」。
pub fn ensure_transcript_usable(text: &str, duration_seconds: f32) -> Result<(), String> {
    if duration_seconds < MIN_TRANSCRIPT_SECONDS {
        return Err(crate::error_code::code_detail(
            ErrorCode::RecordTooShort,
            MIN_TRANSCRIPT_SECONDS,
        ));
    }
    if text.trim().is_empty() {
        return Err(crate::code(ErrorCode::RecordEmpty));
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
            crate::error_code::code(ErrorCode::RecordStopFailed)
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

/// 识别跟不上、丢掉一段音频时：造一个缺口标记段。
///
/// 缺口标记落在**缺口终点**（`at` 由调用方给）—— 丢掉的音频在这之后
/// 才开始被识别。流式与整段必须同一条规则，否则同一段录音在两个模式里
/// 缺口的位置不一样。
fn gap_segment(dropped: u32, rate: u32, at: f32, what: &str) -> asr::Segment {
    let seconds = audio::gap_seconds(dropped, rate);
    log::warn(format!("{what}丢掉 {seconds} 秒音频"));
    let marker = audio::gap_marker(dropped, rate);
    asr::Segment {
        text: marker,
        start: at,
        end: at,
    }
}

/// 真实录音时间（秒）：喂给识别器的采样 + 被丢掉的采样。
///
/// 丢掉的音频占真实录音时间（流式路径一直这么算）—— 不算进去，
/// 识别跟不上时录音时长会短一截。
fn recorded_seconds(fed_16k: usize, dropped_16k: usize) -> f32 {
    (fed_16k + dropped_16k) as f32 / asr::SAMPLE_RATE as f32
}

/// 整段模式的时间轴换算：把「喂给识别器的采样位置」换成真实录音位置。
///
/// 整段模式的 VAD 只见过喂进去的音频 —— 它的句段时间是**喂进去的时间轴**。
/// 被丢掉的音频同样占真实录音时间，所以句段时间要补上缺口。
///
/// `drops` 是 `(缺口位置(秒), 累计被丢掉的 16 kHz 采样数)`，按发生顺序递增；
/// 第二个字段是**含本次**的累计值。
///
/// 缺口位置存**秒**而不是采样下标：句子的 `start` 也是秒（`asr.rs` 里
/// `from as f32 / SAMPLE_RATE as f32`），同一个整数换出的 f32 逐位相等 ——
/// 直接比秒就精确了。绕道「秒 → 下标 → 比较」会在长录音上因 f32 往返
/// 差 ±1 个采样，边界正好压在缺口上时整句错位。
type Drops = Vec<(f32, usize)>;

/// 累计被丢掉的采样数：缺口位置 `pos` 落在 `at` 之前（`pos < at`）
/// 还是正好在它上面（`pos == at`）。
fn cum_dropped(drops: &Drops, at: f32, inclusive: bool) -> usize {
    let mut total = 0;
    for (pos, cum) in drops {
        let hit = if inclusive { *pos <= at } else { *pos < at };
        if hit {
            total = *cum;
        } else {
            break;
        }
    }
    total
}

/// 整段模式的一句：换算到真实录音时间轴。
///
/// 两种模式的时间轴必须一致 —— 否则同一次录音在两个模式里，
/// 缺口标记和句子的先后顺序会不一样。
fn to_real_time(segment: &asr::Segment, drops: &Drops) -> asr::Segment {
    let rate = asr::SAMPLE_RATE as f32;
    asr::Segment {
        text: segment.text.clone(),
        // 起点：缺口正好压在这个位置上的，也已经过去了（缺口在喂入之前发生）。
        start: segment.start + cum_dropped(drops, segment.start, true) as f32 / rate,
        // 终点：位置正好等于缺口位置的缺口还没发生 —— 它在句子之后。
        end: segment.end + cum_dropped(drops, segment.end, false) as f32 / rate,
    }
}

/// 缺口标记的落点：缺口**终点**（丢掉的音频之后才开始识别）。
///
/// 流式路径本来就是这样，整段原来落在缺口起点 —— 统一到终点。
fn gap_at(pos_16k: usize, drops: &Drops) -> f32 {
    let rate = asr::SAMPLE_RATE as f32;
    let pos = pos_16k as f32 / rate;
    pos + cum_dropped(drops, pos, true) as f32 / rate
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

/// **本地**整段模式：边录音边按句识别（音频不出本机）。
///
/// 采集线程把新采样放进队列，这里取走、重采样到 16k、喂给 VAD 切句，
/// 切出一句就立刻送 SenseVoice 识别并把结果推给界面。
/// 识别完的音频立刻丢掉，只留文本 —— 内存不随录音时长增长。
///
/// 会话由调用方建好传进来：VAD 建不起来要在**开始录音时**就报错，
/// 不能等到录完了才让用户撞上「停止超时」。
pub fn run_local_offline<F>(
    session: asr::OfflineSession,
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
    let mut session = session;
    // 电平自动增益：偏小的录音会整段被 VAD 当成非语音丢掉（见 audio::Agc）。
    let mut agc = audio::Agc::new();
    // 识别跟不上时丢掉的音频：在结果里留个可见的缺口（按时间插进句子列表）。
    let mut gaps: Vec<asr::Segment> = Vec::new();
    // 已经喂给识别器的 16 kHz 采样数（不能用设备采样率）
    let mut fed_16k = 0usize;
    // 累计被丢掉的 16 kHz 采样数（丢掉的音频同样占真实录音时间）
    let mut dropped_16k_total = 0usize;
    // 缺口发生的位置：(缺口位置(秒), 累计被丢掉的 16 kHz 采样数)。
    // 用来把 VAD 的「喂进去的时间轴」换算成真实录音时间。
    let mut drops: Drops = Vec::new();
    let discarded = || discard.load(Ordering::Relaxed);

    loop {
        let new_audio = queue.take_new();
        if new_audio.dropped > 0 {
            // 识别跟不上：丢掉了一段音频，要在结果里留下可见的缺口。
            // 标记既进最终文本，也作为一句实时发出去（前端追加后才看得到）。
            dropped_16k_total += audio::resampled_len(new_audio.dropped, rate);
            drops.push((fed_16k as f32 / asr::SAMPLE_RATE as f32, dropped_16k_total));
            let segment = gap_segment(
                new_audio.dropped,
                rate,
                gap_at(fed_16k, &drops),
                "离线识别跟不上，",
            );
            gaps.push(segment.clone());
            if !discarded() {
                emit(OfflineEvent::Segment(segment));
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
                // 实时事件也用真实录音时间轴 —— 否则同一个事件流里
                // 句子是「喂进去的时间轴」、缺口标记是真实时间轴，两套刻度混用。
                let segment = to_real_time(&segment, &drops);
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
            dropped_16k_total += audio::resampled_len(last.dropped, rate);
            drops.push((fed_16k as f32 / asr::SAMPLE_RATE as f32, dropped_16k_total));
            let segment = gap_segment(last.dropped, rate, gap_at(fed_16k, &drops), "离线收尾");
            gaps.push(segment.clone());
            emit(OfflineEvent::Segment(segment));
        }
        let mut samples = match &resampler {
            Some(resampler) => resampler.resample(&last.samples, true),
            None => last.samples,
        };
        if !samples.is_empty() {
            agc.process(&mut samples);
            for event in session.push(&samples) {
                let OfflineEvent::Segment(segment) = event;
                let segment = to_real_time(&segment, &drops);
                log_segment("收尾识别出一句", &segment);
                emit(OfflineEvent::Segment(segment));
            }
        }
        for event in session.finish() {
            let OfflineEvent::Segment(segment) = event;
            let segment = to_real_time(&segment, &drops);
            log_segment("收尾识别出一句", &segment);
            emit(OfflineEvent::Segment(segment));
        }
    }

    // 文本和 segments 都从同一份句子列表来（缺口标记按时间插进去）——
    // 和流式路径同一个做法，两种模式的 segments 语义才一致。
    // 句子的时间换算到真实录音时间轴（缺口标记也在这个轴上）——
    // 不换算的话缺口会排到它本该在的位置之后。
    let segments: Vec<asr::Segment> = session
        .segments()
        .iter()
        .map(|s| to_real_time(s, &drops))
        .collect();
    let mut transcript = asr::transcript_from(
        &segments,
        // 丢掉的音频同样占真实录音时间 —— 不算进去，识别跟不上时时长会短一截。
        recorded_seconds(session.recorded_samples(), dropped_16k_total),
        session.elapsed_ms(),
        &session.language(),
    );
    asr::insert_gaps(&mut transcript, gaps);
    transcript
}

/// 判一次 VAD 并立刻清空队列。
///
/// 这里只拿 VAD 做门控，不消费识别句段 —— 判完必须 clear()，
/// 否则已切出的音频会一直压在队列里，越攒越多。
fn vad_is_speech(detector: &sherpa_onnx::VoiceActivityDetector, samples: &[f32]) -> bool {
    let mut detected = false;
    for window in samples.chunks(512) {
        detector.accept_waveform(window);
        // 逐窗取「或」：一批里有一窗是语音就整批算语音。只看批末状态的话，
        // 一次喂进整批（或识别落后时的补批）会被批尾的静音判成非语音，
        // 整批音频被门控丢掉。
        detected |= detector.detected();
    }
    // 取完状态立即释放已切出的音频：门控不需要那些句段。
    detector.clear();
    detected
}

/// **本地**流式模式：边录音边喂流式识别器；每 60ms 取一次新采样，端点处提交一句。
///
/// 与 `run_local_offline` 一样是**本地**推理（音频不出本机）——
/// 两条本地路径的区别是「整段切句」还是「边说边出字」。
pub fn run_local_streaming<F>(
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
    let mut preroll_end = 0usize;
    let mut active = false;
    // 识别跟不上时丢掉的音频，在结果里留个可见的缺口
    let mut gaps: Vec<asr::Segment> = Vec::new();
    let mut last_voice = std::time::Instant::now();
    while !stop.load(Ordering::Relaxed) && !discard.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(60));
        let new_audio = queue.take_new();
        // 丢掉的输入同样占真实录音时间：缺口结束后才是后续音频的位置。
        // 走 advance_dropped：洞若落在这句之前，这句的起点要跟着后移
        // （否则缺口标记会排到句子**后面**，与整段模式不一致）。
        session.advance_dropped(audio::resampled_len(new_audio.dropped, rate));
        let at = session.elapsed_seconds();
        if new_audio.dropped > 0 {
            // 识别跟不上：丢掉了一段音频，标记出来，不能静默丢
            let segment = gap_segment(new_audio.dropped, rate, at, "流式识别跟不上，");
            gaps.push(segment.clone());
            if !discard.load(Ordering::Relaxed) {
                emit(vec![StreamEvent::Segment(segment)]);
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
        let sample_end = session.advance_samples(samples.len());

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
                    let events = session.push_at(&std::mem::take(&mut preroll), preroll_end);
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
            preroll_end = sample_end;
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
    session.advance_dropped(audio::resampled_len(last.dropped, rate));
    if last.dropped > 0 {
        let at = session.elapsed_seconds();
        let segment = gap_segment(last.dropped, rate, at, "流式收尾");
        gaps.push(segment.clone());
        emit(vec![StreamEvent::Segment(segment)]);
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

/// 门控决策（纯函数，可单测）：能力 + 本地 VAD 是否可用 → 是否本地门控。
///
/// - 客户端提交（`client_commit`）：需要本地 VAD 做断句。有 VAD → 门控；
///   没有 → **报错**（不能退化成「不门控也不提交」，那是主规格禁止的静默退化）。
/// - 服务端断句（`server_endpoint`）：不门控，静音照上传，由服务端判句尾。
fn gate(caps: asr::cloud::Capabilities, vad_available: bool) -> Result<bool, String> {
    match caps.endpoint_kind {
        asr::cloud::EndpointKind::ClientCommit => {
            if vad_available {
                Ok(true)
            } else {
                Err(crate::code(ErrorCode::CloudVadMissing))
            }
        }
        asr::cloud::EndpointKind::ServerEndpoint => Ok(false),
    }
}

/// **云端**模式：边录音边把音频送到云端服务，provider 那边出字。
///
/// 三个入口里**只有它会把音频传出去**（另两个是本地推理）。
///
/// 与 `run_local_streaming` 同构：VAD 门控 / 预录 / 电平 / 缺口 / 时间轴
/// 走同一套规则，识别那一步换成 `CloudStreamingSession`。
///
/// **为什么不是同一个函数**：本地流式的会话是 `StreamingSession`，
/// 云端的是 `CloudStreamingSession`；两者只共享 `AsrSession` 那层接口。
/// 合成一个泛型函数会把本地路径也拖进改动范围 —— 本地路径零行为改动
/// 是硬要求（`streaming_smoke` 守着），所以这里各写各的，
/// 共用的部分（VAD 参数、电平、缺口）是同一条规则的两个入口。
pub fn run_cloud_streaming<F>(
    session: asr::cloud::CloudStreamingSession,
    vad_config: Option<sherpa_onnx::VadModelConfig>,
    queue: audio::SampleQueue,
    rate: u32,
    stop: Arc<AtomicBool>,
    discard: Arc<AtomicBool>,
    mut emit: F,
) -> Transcript
where
    F: FnMut(Vec<StreamEvent>),
{
    let mut session = session;
    let resampler = if rate as i32 == asr::SAMPLE_RATE {
        None
    } else {
        LinearResampler::create(rate as i32, asr::SAMPLE_RATE)
    };
    let vad = vad_config
        .as_ref()
        .and_then(|config| sherpa_onnx::VoiceActivityDetector::create(config, 60.0));
    // 门控由**能力**决定，不是由「本机有没有 VAD 文件」决定：
    // 客户端提交族必须本地门控（拿不到 VAD 就报错，不静默退化）；
    // 服务端断句族一律不门控 —— 静音照上传，服务端靠静音判句尾。
    let gated = match gate(session.capabilities(), vad.is_some()) {
        Ok(gated) => gated,
        Err(error) => {
            // 启动阶段已校验过（lib.rs 的 `vad_precondition`）；到这里说明前提
            // 被破坏 —— 不能带着「不门控也不提交」的配置开录。
            log::error(format!("云端录音前置条件不满足：{error}"));
            return Transcript {
                text: String::new(),
                segments: Vec::new(),
                duration_seconds: 0.0,
                elapsed_ms: 0,
                language: "zh-en".into(),
                warning: Some(error),
            };
        }
    };
    // 电平自动增益：偏小的录音会被 VAD 整段当非语音丢掉（见 audio::Agc）。
    let mut agc = audio::Agc::new();
    // 说话起点前留 0.4s，避免 VAD 起音延迟把字头切掉
    let preroll_limit = (asr::SAMPLE_RATE as usize) / 5 * 2;
    let mut preroll: Vec<f32> = Vec::new();
    let mut preroll_end = 0usize;
    let mut active = false;
    // 采集跟不上时丢掉的音频，在结果里留个可见的缺口
    let mut gaps: Vec<asr::Segment> = Vec::new();
    let mut last_voice = std::time::Instant::now();
    while !stop.load(Ordering::Relaxed) && !discard.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(60));
        let new_audio = queue.take_new();
        // 丢掉的输入同样占真实录音时间：缺口结束后才是后续音频的位置。
        session.advance_dropped(audio::resampled_len(new_audio.dropped, rate));
        // 上传丢包（队列满，那段音频没送到云端）：**不推进时钟** —— 这些采样
        // 喂给会话时已经计入 `advance_samples`（用户确实录到了），再推进一次
        // 录音时长会翻倍。与采集丢包不同：那种采样根本没进过会话。
        //
        // 缺口标记打在**丢掉的起点**：这段音频已经算在包含它的那句区间里，
        // 打在终点会排到句子后面（读起来像句子之后才丢的）。
        for (start, dropped) in session.take_upload_gaps() {
            let marker = audio::gap_marker(dropped as u32, asr::SAMPLE_RATE as u32);
            // 起点是 16 kHz 采样数 → 转成秒（与句子区间同一把尺子）。
            let at = start as f32 / asr::SAMPLE_RATE as f32;
            let segment = asr::Segment {
                text: marker,
                start: at,
                end: at,
            };
            gaps.push(segment.clone());
            if !discard.load(Ordering::Relaxed) {
                emit(vec![StreamEvent::Segment(segment)]);
            }
        }
        let at = session.elapsed_seconds();
        if new_audio.dropped > 0 {
            // 采集跟不上：丢掉了一段音频，要在结果里留下可见的缺口。
            let segment = gap_segment(new_audio.dropped, rate, at, "云端发送跟不上，");
            gaps.push(segment.clone());
            if !discard.load(Ordering::Relaxed) {
                emit(vec![StreamEvent::Segment(segment)]);
            }
        }
        let chunk = new_audio.samples;
        if chunk.is_empty() {
            if !discard.load(Ordering::Relaxed) {
                let events = session.poll();
                log_stream_events("云端识别出一句", &events);
                emit(events);
            }
            continue;
        }
        let mut samples = match &resampler {
            Some(resampler) => resampler.resample(&chunk, false),
            None => chunk,
        };
        if samples.is_empty() {
            continue;
        }
        // 提电平要在 VAD 之前 —— 门控 VAD 判的就是这段音频，
        // 偏小的电平会让它整段判成非语音（见 audio::Agc）。
        agc.process(&mut samples);
        let sample_end = session.advance_samples(samples.len());

        if !discard.load(Ordering::Relaxed) {
            let events = session.poll();
            log_stream_events("云端识别出一句", &events);
            emit(events);
        }
        let Some(detector) = vad.as_ref().filter(|_| gated) else {
            // 不门控（服务端断句族）：全部上传，包括静音 —— 服务端靠静音判句尾。
            if !discard.load(Ordering::Relaxed) {
                let events = session.push(&samples);
                log_stream_events("云端识别出一句", &events);
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
                    let events = session.push_at(&std::mem::take(&mut preroll), preroll_end);
                    log_stream_events("云端识别出一句", &events);
                    emit(events);
                }
            }
            if !discard.load(Ordering::Relaxed) {
                let events = session.push(&samples);
                log_stream_events("云端识别出一句", &events);
                emit(events);
            }
        } else {
            if active {
                active = false;
                // 静音开始：立刻收尾当前句子（客户端提交，服务端回一条结果）。
                if !discard.load(Ordering::Relaxed) {
                    let events = session.force_end();
                    log_stream_events("云端识别出一句", &events);
                    emit(events);
                }
            }
            preroll.extend_from_slice(&samples);
            preroll_end = sample_end;
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
    session.advance_dropped(audio::resampled_len(last.dropped, rate));
    if last.dropped > 0 {
        let at = session.elapsed_seconds();
        let segment = gap_segment(last.dropped, rate, at, "云端收尾");
        gaps.push(segment.clone());
        emit(vec![StreamEvent::Segment(segment)]);
    }
    let chunk = last.samples;
    let tail = match &resampler {
        Some(resampler) => resampler.resample(&chunk, true),
        None => chunk,
    };
    if !tail.is_empty() {
        // 循环退出后才采到的尾包也占真实录音时长。
        session.advance_samples(tail.len());
        let events = session.push(&tail);
        log_stream_events("云端收尾识别出一句", &events);
        emit(events);
    }
    let events = session.finish();
    log_stream_events("云端收尾识别出一句", &events);
    emit(events);
    // 收尾这一批的上传丢包也要报告：不然结尾一段音频静默消失。
    // **必须在 finish 之后取** —— 尾部重采样的丢包与没收尾的丢包段都是
    // finish 时才落账的。
    for (start, dropped) in session.take_upload_gaps() {
        let marker = audio::gap_marker(dropped as u32, asr::SAMPLE_RATE as u32);
        let at = start as f32 / asr::SAMPLE_RATE as f32;
        let segment = asr::Segment {
            text: marker,
            start: at,
            end: at,
        };
        gaps.push(segment.clone());
        emit(vec![StreamEvent::Segment(segment)]);
    }
    let mut transcript = session.transcript();
    asr::insert_gaps(&mut transcript, gaps);
    transcript
}

#[cfg(test)]
mod tests;
