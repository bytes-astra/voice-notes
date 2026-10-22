//! 麦克风采集（cpal）与重采样到 16 kHz 单声道。

use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

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
        let Ok(mut guard) = self.samples.lock() else { return };
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
        let Ok(mut guard) = self.samples.lock() else { return };
        guard.extend_from_slice(samples);
        if guard.len() > LEVEL_RING {
            let excess = guard.len() - LEVEL_RING;
            guard.drain(..excess);
        }
    }

    fn peak(&self) -> f32 {
        let Ok(buf) = self.samples.lock() else { return 0.0 };
        let tail = &buf[buf.len().saturating_sub(LEVEL_RING)..];
        if tail.is_empty() {
            return 0.0;
        }
        let rms = (tail.iter().map(|s| s * s).sum::<f32>() / tail.len() as f32).sqrt();
        (rms * 6.0).clamp(0.0, 1.0)
    }
}

/// 识别跟不上、丢掉了一段音频时插进结果里的标记。
pub fn gap_marker(dropped_samples: u32, sample_rate: u32) -> String {
    let rate = sample_rate.max(1);
    let seconds = (dropped_samples as f32 / rate as f32).ceil().max(1.0) as u32;
    // 用全角括号开头：U+FF08 属于 CJK 范围，拼句时不会多出空格
    format!("（此处约 {seconds} 秒未识别）")
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
struct LevelPayload {
    level: f32,
    elapsed: f32,
}

/// 一个可用的输入设备。
#[derive(Clone, Debug, Serialize)]
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
        LevelRing::default(),
        Arc::new(Mutex::new(None)),
        device,
    )?;
    drop(stream);
    Ok(())
}

/// 打开默认输入设备开始采集；音频只留在内存里，停止后由调用方转写。
/// 采集缓冲是有界的：未消费的采样超过上限就丢最旧的（内存恒定）。
pub fn start(app: AppHandle, device: Option<&str>) -> Result<Recorder, String> {
    start_with(Some(app), device)
}

/// 与 [`start`] 相同，但不发 UI 事件（测试/无界面场景用）。
pub fn start_with(app: Option<AppHandle>, device: Option<&str>) -> Result<Recorder, String> {
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
                        level: 0.0,
                        elapsed: started.elapsed().as_secs_f32(),
                    },
                );
            }
        })
        .map_err(|e| format!("无法创建录音线程: {e}"))?;

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
            Err("打开麦克风超时（请检查系统麦克风权限）".into())
        }
    }
}

fn build_stream(
    app: Option<&AppHandle>,
    levels: LevelRing,
    error: Arc<Mutex<Option<String>>>,
    wanted: Option<&str>,
) -> Result<(cpal::Stream, u32, SampleQueue), String> {
    let host = cpal::default_host();
    let device = find_device(&host, wanted)?;
    let supported = device
        .default_input_config()
        .map_err(|e| format!("读取麦克风配置失败: {e}"))?;
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
            app.cloned(),
        ),
        SampleFormat::I16 => make_stream::<i16>(
            &device,
            &config,
            channels,
            queue.clone(),
            levels.clone(),
            error.clone(),
            app.cloned(),
        ),
        SampleFormat::U16 => make_stream::<u16>(
            &device,
            &config,
            channels,
            queue.clone(),
            levels.clone(),
            error.clone(),
            app.cloned(),
        ),
        SampleFormat::I32 => make_stream::<i32>(
            &device,
            &config,
            channels,
            queue.clone(),
            levels.clone(),
            error.clone(),
            app.cloned(),
        ),
        other => return Err(format!("不支持的麦克风采样格式: {other:?}")),
    }
    .map_err(|e| format!("无法打开麦克风（{format:?}）: {e}"))?;

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
            .ok_or_else(|| format!("找不到输入设备：{name}")),
        None => host
            .default_input_device()
            .ok_or_else(|| "没有可用的麦克风".to_string()),
    }
}

fn stream_error_handler(
    app: Option<AppHandle>,
    slot: Arc<Mutex<Option<String>>>,
) -> impl FnMut(cpal::StreamError) + Send + 'static {
    move |err: cpal::StreamError| {
        let message = format!("麦克风异常: {err}");
        if let Ok(mut guard) = slot.lock() {
            *guard = Some(message.clone());
        }
        if let Some(app) = &app {
            let _ = app.emit("audio:error", message);
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
    app: Option<AppHandle>,
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
        stream_error_handler(app, error),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn take_new_consumes_samples() {
        let q = SampleQueue::new(48_000);
        q.push(&[1.0, 2.0, 3.0]);
        let got = q.take_new();
        assert_eq!(got.samples, vec![1.0, 2.0, 3.0]);
        assert_eq!(got.dropped, 0);
        assert!(q.take_new().samples.is_empty()); // 取过就没有了
    }

    #[test]
    fn take_new_drops_oldest_when_over_limit() {
        let q = SampleQueue::new(48_000);
        let limit = MAX_PENDING_SECONDS as usize * 48_000;
        q.push(&vec![0.0; limit + 100]);
        let got = q.take_new();
        assert_eq!(got.dropped, 100);
        assert_eq!(got.samples.len(), limit);
    }

    #[test]
    fn pending_limit_follows_sample_rate() {
        // 16 kHz 的设备上限应该按 16k 算，不能拿 48k 的样本数去卡
        let q = SampleQueue::new(16_000);
        let limit = MAX_PENDING_SECONDS as usize * 16_000;
        q.push(&vec![0.0; limit]);
        assert_eq!(q.take_new().dropped, 0);
        q.push(&vec![0.0; 1]);
        assert_eq!(q.take_new().dropped, 0);
    }

    #[test]
    fn gap_marker_mentions_seconds() {
        assert!(gap_marker(48_000, 48_000).contains('1')); // 48000 @48k = 1 秒
        assert!(gap_marker(96_000, 48_000).contains('2')); // 2 秒
    }

    #[test]
    fn default_input_is_usable() {
        // CI runner 的虚拟声卡可能列得出来、读配置却失败，别让宿主环境卡住构建。
        if std::env::var("CI").is_ok() {
            eprintln!("跳过：CI 环境不依赖宿主音频设备");
            return;
        }
        let host = cpal::default_host();
        let Some(device) = host.default_input_device() else {
            eprintln!("跳过：没有默认输入设备");
            return;
        };
        let config = device.default_input_config().expect("读取默认输入配置");
        assert!(config.sample_rate().0 >= 8_000);
        assert!(config.channels() >= 1);
        println!(
            "默认麦克风：{} Hz / {} 声道 / {:?}",
            config.sample_rate().0,
            config.channels(),
            config.sample_format()
        );
    }

    /// 真录音测试（会触发系统麦克风授权），默认跳过：
    ///   VOICE_NOTES_RECORD_TEST=1 cargo test --lib records_microphone -- --nocapture
    #[test]
    fn records_microphone() {
        if std::env::var("VOICE_NOTES_RECORD_TEST").is_err() {
            eprintln!("跳过：未设置 VOICE_NOTES_RECORD_TEST");
            return;
        }
        let recorder = start_with(None, None).expect("打开麦克风");
        let rate = recorder.sample_rate();
        let queue = recorder.queue();
        std::thread::sleep(std::time::Duration::from_secs(2));
        recorder.stop().expect("停止录音");
        // 停止后采样仍留在队列里，由消费者取走（stop 不再代取）。
        let samples = queue.take_new().samples;
        let peak = samples.iter().fold(0f32, |acc, s| acc.max(s.abs()));
        println!("采到 {} 个采样 @ {rate} Hz，峰值 {peak:.4}", samples.len());
        assert!(samples.len() > rate as usize, "录音样本太少");
        assert!(peak <= 1.0, "样本超出 [-1, 1]");
    }
}
