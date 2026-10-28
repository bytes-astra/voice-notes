//! 麦克风采集（cpal）与重采样到 16 kHz 单声道。
//!
//! 缺口占位符（`[[gap:秒数]]`）的格式也在这里 —— 构造和解析都在这一处，
//! 别处不要另写一份正则（前端 `core.js` 的 `localizeText()` 负责本地化）。

use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::{code, code_detail};

/// 未消费音频的上限（秒）。ASR 线程跟不上时丢掉最旧的，并在结果里标记缺口。
pub const MAX_PENDING_SECONDS: u32 = 60;
/// 电平环长度（采样数）：只用来算音量，必须和采集缓冲分开。
const LEVEL_RING: usize = 2048;
const LEVEL_INTERVAL: Duration = Duration::from_millis(60);

enum Cmd {
    Stop,
    Cancel,
}

/// 采集缓冲：生产者（cpal 回调）写入，消费者（ASR 线程）取走即释放。
///
/// 内存不随录音时长增长 —— ASR 线程每轮 `take_new()` 取走新到的采样，
/// 旧采样立刻释放。超过 `MAX_PENDING_SECONDS` 还没被取走的部分会丢掉最旧的，
/// 并把丢弃数量报给调用方：调用方要在结果里标记缺口，不能静默丢。
#[derive(Clone)]
pub struct SampleQueue {
    samples: Arc<Mutex<Vec<f32>>>,
    /// 未消费采样的上限；None = 不限。
    limit: Option<usize>,
}

/// 一次取走的新采样。
#[derive(Clone, Debug, Default)]
pub struct NewAudio {
    /// 新到的采样
    pub samples: Vec<f32>,
    /// 因为超过上限而被丢弃的采样数
    pub dropped: u32,
}

impl SampleQueue {
    /// 有上限的缓冲：ASR 线程跟不上时丢最旧的（流式模式用）。
    pub fn new(sample_rate: u32) -> Self {
        Self::with_limit(Some(MAX_PENDING_SECONDS as usize * sample_rate as usize))
    }

    /// 指定上限的缓冲（测试用：可以把上限调得很小来触发丢弃）。
    pub fn with_limit(limit: Option<usize>) -> Self {
        Self {
            samples: Arc::new(Mutex::new(Vec::new())),
            limit,
        }
    }

    pub fn push(&self, samples: &[f32]) {
        let Ok(mut guard) = self.samples.lock() else {
            return;
        };
        guard.extend_from_slice(samples);
    }

    /// 当前还没被取走的采样数（诊断用：正常应该一直很小）。
    pub fn pending(&self) -> usize {
        self.samples.lock().map(|g| g.len()).unwrap_or(0)
    }

    /// 取走全部未消费的采样；超出上限时（最旧的）会被丢掉并计数。
    pub fn take_new(&self) -> NewAudio {
        let Ok(mut guard) = self.samples.lock() else {
            return NewAudio::default();
        };
        let mut dropped = 0u32;
        if let Some(limit) = self.limit {
            if guard.len() > limit {
                let excess = guard.len() - limit;
                guard.drain(..excess);
                dropped = excess as u32;
            }
        }
        NewAudio {
            samples: std::mem::take(&mut *guard),
            dropped,
        }
    }
}

/// 电平环：只保留最近一小段采样，用来算音量。
///
/// 必须和采集缓冲分开 —— 采集缓冲改成消费式之后就没有历史了，
/// 音量表去读它会恒读 0（看起来像麦克风坏了）。
#[derive(Clone, Default)]
pub struct LevelRing {
    samples: Arc<Mutex<Vec<f32>>>,
}

impl LevelRing {
    fn push(&self, samples: &[f32]) {
        let Ok(mut guard) = self.samples.lock() else {
            return;
        };
        guard.extend_from_slice(samples);
        if guard.len() > LEVEL_RING {
            let excess = guard.len() - LEVEL_RING;
            guard.drain(..excess);
        }
    }

    fn peak(&self) -> f32 {
        let Ok(buf) = self.samples.lock() else {
            return 0.0;
        };
        let tail = &buf[buf.len().saturating_sub(LEVEL_RING)..];
        if tail.is_empty() {
            return 0.0;
        }
        let rms = (tail.iter().map(|s| s * s).sum::<f32>() / tail.len() as f32).sqrt();
        (rms * 6.0).clamp(0.0, 1.0)
    }
}

/// 缺口占位符的前缀：识别跟不上、丢掉音频时插进文本里。
///
/// 后端只给占位符（`[[gap:秒数]]`），界面语言由前端渲染时决定
/// （前端 `core.js` 的 `localizeText()` → `t('record.gap', {seconds})`）。
/// 占位符前后拼句不加空格（后端 `asr::join_segments` 与前端 `joinText()` 一致）。
pub const GAP_MARKER_PREFIX: &str = "[[gap:";

/// 丢掉音频折算成多少秒（向上取整，至少 1 秒）。
pub fn resampled_len(samples: u32, sample_rate: u32) -> usize {
    (samples as u64 * 16_000).div_ceil(sample_rate as u64) as usize
}

pub fn gap_seconds(dropped_samples: u32, sample_rate: u32) -> u32 {
    let rate = sample_rate.max(1);
    (dropped_samples as f32 / rate as f32).ceil().max(1.0) as u32
}

/// 识别跟不上、丢掉了一段音频时插进结果里的标记（语言中立占位符）。
pub fn gap_marker(dropped_samples: u32, sample_rate: u32) -> String {
    format!(
        "{GAP_MARKER_PREFIX}{}]]",
        gap_seconds(dropped_samples, sample_rate)
    )
}

/// `audio:error` 事件的载荷：`code` 是字典 key，`fatal` 表示是否要中断录音。
///
/// - `fatal: false` → 前端只弹提示，录音继续（丢掉音频的缺口提示）
/// - `fatal: true`  → 麦克风坏了之类，前端要停录音
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioErrorPayload {
    /// 字典 key（如 `record.micError`、`record.gapWarning`、`decode.partial`）
    pub code: String,
    /// 技术细节（系统错误文本、原因），不是展示文案
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// 丢掉的秒数（缺口提示用）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seconds: Option<u32>,
    /// 致命错误：前端要中断录音
    pub fatal: bool,
    /// 录音事件属于哪次会话；文件解码错误不属于录音，为 None。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<u64>,
}

impl AudioErrorPayload {
    /// 只有错误码的提示（致命）。
    pub fn fatal(code: &str) -> Self {
        Self {
            code: code.to_string(),
            detail: None,
            seconds: None,
            fatal: true,
            session_id: None,
        }
    }

    /// 带细节的提示（致命）。
    pub fn fatal_detail(code: &str, detail: impl std::fmt::Display) -> Self {
        Self {
            code: code.to_string(),
            detail: Some(detail.to_string()),
            seconds: None,
            fatal: true,
            session_id: None,
        }
    }

    /// 缺口提示：不致命，录音继续。
    pub fn gap(seconds: u32) -> Self {
        Self {
            code: "record.gapWarning".to_string(),
            detail: None,
            seconds: Some(seconds),
            fatal: false,
            session_id: None,
        }
    }

    /// 解码只完成一部分：不致命，但要把原因报给用户。
    pub fn partial(code: &str, detail: impl std::fmt::Display) -> Self {
        Self {
            code: code.to_string(),
            detail: Some(detail.to_string()),
            seconds: None,
            fatal: false,
            session_id: None,
        }
    }
}

pub struct Recorder {
    tx: mpsc::Sender<Cmd>,
    join: Option<thread::JoinHandle<()>>,
    queue: SampleQueue,
    error: Arc<Mutex<Option<String>>>,
    sample_rate: u32,
}

impl Recorder {
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// 共享的采集缓冲：识别线程每次 `take_new()` 取走新到的采样。
    pub fn queue(&self) -> SampleQueue {
        self.queue.clone()
    }

    /// 结束录音。
    ///
    /// **不取走队列里的采样**：识别线程收尾时还要自己 `take_new()` 那一小段
    /// 尾巴（主线程先取走的话，句尾最后一个字会被切掉）。
    pub fn stop(mut self) -> Result<(), String> {
        let _ = self.tx.send(Cmd::Stop);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
        if let Some(error) = self.error.lock().unwrap().take() {
            return Err(error);
        }
        Ok(())
    }

    #[cfg(test)]
    pub fn inject_error(&self, error: String) {
        *self.error.lock().unwrap() = Some(error);
    }

    pub fn cancel(mut self) {
        let _ = self.tx.send(Cmd::Cancel);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Cancel);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct LevelPayload {
    session_id: Option<u64>,
    level: f32,
    elapsed: f32,
}

/// 一个可用的输入设备。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InputDevice {
    pub name: String,
    pub is_default: bool,
}

/// 列出系统里的输入设备（麦克风）。
pub fn list_input_devices() -> Vec<InputDevice> {
    let host = cpal::default_host();
    let default_name = host
        .default_input_device()
        .and_then(|device| device.name().ok());
    let Ok(devices) = host.input_devices() else {
        return Vec::new();
    };
    devices
        .filter_map(|device| {
            let name = device.name().ok()?;
            let is_default = default_name.as_deref() == Some(name.as_str());
            Some(InputDevice { name, is_default })
        })
        .collect()
}

/// 在设备名列表里挑一个：精确匹配才选它，其余情况用系统默认（返回 None）。
pub fn pick_device_name(requested: Option<&str>, available: &[String]) -> Option<String> {
    let wanted = requested?.trim();
    if wanted.is_empty() {
        return None;
    }
    available
        .iter()
        .find(|name| name.as_str() == wanted)
        .cloned()
}

/// 打开一次输入设备（不录音），用来提前触发系统的麦克风权限弹窗。
///
/// macOS 是在第一次真正打开输入流时才弹权限框的；放在界面渲染完之后调用，
/// 用户看到的是已经画好的界面，而不是对着白屏等弹窗。
pub fn probe_input(device: Option<&str>) -> Result<(), String> {
    // 打开再丢掉：系统权限是在打开输入流的时候触发的。
    let (stream, _, _) = build_stream(
        None,
        None,
        LevelRing::default(),
        Arc::new(Mutex::new(None)),
        device,
    )?;
    drop(stream);
    Ok(())
}

/// 打开默认输入设备开始采集；音频只留在内存里，停止后由调用方转写。
/// 采集缓冲是有界的：未消费的采样超过上限就丢最旧的（内存恒定）。
pub fn start(app: AppHandle, device: Option<&str>, session_id: u64) -> Result<Recorder, String> {
    start_with(Some(app), device, Some(session_id))
}

/// 与 [`start`] 相同，但不发 UI 事件（测试/无界面场景用）。
pub fn start_with(
    app: Option<AppHandle>,
    device: Option<&str>,
    session_id: Option<u64>,
) -> Result<Recorder, String> {
    let device = device.map(|name| name.to_string());
    let (tx, rx) = mpsc::channel::<Cmd>();
    let (ready_tx, ready_rx) = mpsc::channel::<Result<(u32, SampleQueue), String>>();
    let levels = LevelRing::default();
    let error: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

    let thread_levels = levels.clone();
    let thread_error = error.clone();
    let join = thread::Builder::new()
        .name("microphone".into())
        .spawn(move || {
            let stream = match build_stream(
                app.as_ref(),
                session_id,
                thread_levels.clone(),
                thread_error.clone(),
                device.as_deref(),
            ) {
                Ok((stream, rate, queue)) => {
                    let _ = ready_tx.send(Ok((rate, queue)));
                    stream
                }
                Err(err) => {
                    let _ = ready_tx.send(Err(err));
                    return;
                }
            };
            let _ = stream.play();
            let started = Instant::now();
            let mut last_tick = Instant::now();
            loop {
                match rx.recv_timeout(LEVEL_INTERVAL) {
                    Ok(Cmd::Stop) | Ok(Cmd::Cancel) => break,
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => break,
                }
                if last_tick.elapsed() >= LEVEL_INTERVAL {
                    last_tick = Instant::now();
                    let level = thread_levels.peak();
                    if let Some(app) = &app {
                        let _ = app.emit(
                            "audio:level",
                            LevelPayload {
                                session_id,
                                level,
                                elapsed: started.elapsed().as_secs_f32(),
                            },
                        );
                    }
                }
            }
            drop(stream);
            if let Some(app) = &app {
                let _ = app.emit(
                    "audio:level",
                    LevelPayload {
                        session_id,
                        level: 0.0,
                        elapsed: started.elapsed().as_secs_f32(),
                    },
                );
            }
        })
        .map_err(|e| code_detail("error.recordThreadFailed", e))?;

    match ready_rx.recv_timeout(Duration::from_secs(10)) {
        Ok(Ok((rate, queue))) => Ok(Recorder {
            tx,
            join: Some(join),
            queue,
            error,
            sample_rate: rate,
        }),
        Ok(Err(err)) => {
            let _ = join.join();
            Err(err)
        }
        Err(_) => {
            let _ = tx.send(Cmd::Cancel);
            let _ = join.join();
            Err(code("record.micTimeout"))
        }
    }
}

fn build_stream(
    app: Option<&AppHandle>,
    session_id: Option<u64>,
    levels: LevelRing,
    error: Arc<Mutex<Option<String>>>,
    wanted: Option<&str>,
) -> Result<(cpal::Stream, u32, SampleQueue), String> {
    let host = cpal::default_host();
    let device = find_device(&host, wanted)?;
    let supported = device
        .default_input_config()
        .map_err(|e| code_detail("record.micConfigFailed", e))?;
    let sample_rate = supported.sample_rate().0;
    let channels = supported.channels() as usize;
    let format = supported.sample_format();
    let config: cpal::StreamConfig = supported.config();
    let queue = SampleQueue::new(sample_rate);

    let stream = match format {
        SampleFormat::F32 => make_stream::<f32>(
            &device,
            &config,
            channels,
            queue.clone(),
            levels.clone(),
            error.clone(),
            app.cloned().zip(session_id),
        ),
        SampleFormat::I16 => make_stream::<i16>(
            &device,
            &config,
            channels,
            queue.clone(),
            levels.clone(),
            error.clone(),
            app.cloned().zip(session_id),
        ),
        SampleFormat::U16 => make_stream::<u16>(
            &device,
            &config,
            channels,
            queue.clone(),
            levels.clone(),
            error.clone(),
            app.cloned().zip(session_id),
        ),
        SampleFormat::I32 => make_stream::<i32>(
            &device,
            &config,
            channels,
            queue.clone(),
            levels.clone(),
            error.clone(),
            app.cloned().zip(session_id),
        ),
        other => {
            return Err(code_detail(
                "record.micFormatUnsupported",
                format!("{other:?}"),
            ))
        }
    }
    .map_err(|e| code_detail("record.micOpenFailed", format!("{format:?}: {e}")))?;

    Ok((stream, sample_rate, queue))
}

/// 按名字挑一个输入设备；挑不到就退回系统默认（和设置页的语义一致）。
fn find_device(host: &cpal::Host, wanted: Option<&str>) -> Result<cpal::Device, String> {
    let available: Vec<String> = list_input_devices()
        .into_iter()
        .map(|device| device.name)
        .collect();
    match pick_device_name(wanted, &available) {
        Some(name) => host
            .input_devices()
            .ok()
            .and_then(|mut devices| {
                devices.find(|device| device.name().map(|n| n == name).unwrap_or(false))
            })
            .ok_or_else(|| code_detail("record.deviceNotFound", name)),
        None => host
            .default_input_device()
            .ok_or_else(|| code("record.noInputDevice")),
    }
}

fn stream_error_handler(
    context: Option<(AppHandle, u64)>,
    slot: Arc<Mutex<Option<String>>>,
) -> impl FnMut(cpal::StreamError) + Send + 'static {
    move |err: cpal::StreamError| {
        // 存进 error 槽的是错误码（前端查字典翻译），细节另存一份给事件载荷。
        if let Ok(mut guard) = slot.lock() {
            *guard = Some(code_detail("record.micError", &err));
        }
        if let Some((app, session_id)) = &context {
            let _ = app.emit(
                "audio:error",
                AudioErrorPayload {
                    session_id: Some(*session_id),
                    ..AudioErrorPayload::fatal_detail("record.micError", err)
                },
            );
        }
    }
}

fn make_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    channels: usize,
    queue: SampleQueue,
    levels: LevelRing,
    error: Arc<Mutex<Option<String>>>,
    context: Option<(AppHandle, u64)>,
) -> Result<cpal::Stream, cpal::BuildStreamError>
where
    T: SizedSample + Sample,
    f32: FromSample<T>,
{
    let mut capture = Capture {
        queue,
        levels,
        channels,
        mono: Vec::new(),
    };
    device.build_input_stream(
        config,
        move |data: &[T], _| capture.push(data),
        stream_error_handler(context, error),
        None,
    )
}

/// 采集回调里的工作区：复用 mono 缓冲，避免每块音频都分配一次 Vec。
struct Capture {
    queue: SampleQueue,
    levels: LevelRing,
    channels: usize,
    mono: Vec<f32>,
}

impl Capture {
    /// 多声道混成单声道，写进采集队列和电平环；F32/I16/U16 统一归一化到 -1..1。
    fn push<T>(&mut self, data: &[T])
    where
        T: SizedSample + Sample,
        f32: FromSample<T>,
    {
        let channels = self.channels.max(1);
        self.mono.clear();
        self.mono.reserve(data.len() / channels + 1);
        for frame in data.chunks(channels) {
            let mut sum = 0f32;
            for sample in frame {
                sum += f32::from_sample(*sample);
            }
            self.mono.push(sum / frame.len() as f32);
        }
        self.queue.push(&self.mono);
        self.levels.push(&self.mono);
    }
}

// ──────────────────── 缺口占位符 ────────────────────

/// 是不是缺口占位符（`[[gap:秒数]]`）——和前端 `core.js` 的 `isGapMarker()` 同一套规则。
///
/// 缺口是「丢掉了一段音频」的位置标记，不是句子：它前后都不加空格。
pub fn is_gap_marker(text: &str) -> bool {
    text.starts_with(crate::audio::GAP_MARKER_PREFIX)
}

/// 从缺口占位符里取出秒数（`[[gap:3]]` → 3）；不是占位符就返回 None。
///
/// 和前端 `core.js` 的 `GAP_RE`（`/\[\[gap:(\d+)\]\]/`）同一套规则。
pub fn gap_marker_seconds(text: &str) -> Option<u32> {
    let prefix = crate::audio::GAP_MARKER_PREFIX;
    let start = text.find(prefix)?;
    let rest = &text[start + prefix.len()..];
    let end = rest.find("]]")?;
    let digits = &rest[..end];
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// 累积文本结尾是不是完整的缺口占位符——和前端 `core.js` 的 `GAP_END_RE` 同一套规则。
///
/// 前端是 `/\[\[gap:\d+\]\]$/`：占位符必须整段落在结尾，中间只能是数字。
pub fn ends_with_gap_marker(text: &str) -> bool {
    let prefix = crate::audio::GAP_MARKER_PREFIX;
    let Some(start) = text.rfind(prefix) else {
        return false;
    };
    let rest = &text[start + prefix.len()..];
    let Some(end) = rest.find("]]") else {
        return false;
    };
    if end + 2 != rest.len() {
        return false;
    }
    let digits = &rest[..end];
    !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())
}

#[cfg(test)]
mod tests;
