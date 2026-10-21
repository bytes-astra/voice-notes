//! 麦克风采集（cpal）与重采样到 16 kHz 单声道。

use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample};
use serde::Serialize;
use sherpa_onnx::LinearResampler;
use tauri::{AppHandle, Emitter};

pub const TARGET_RATE: u32 = 16_000;
const MAX_SECONDS: u32 = 600;
const LEVEL_INTERVAL: Duration = Duration::from_millis(60);

enum Cmd {
    Stop,
    Cancel,
}

pub struct Recorder {
    tx: mpsc::Sender<Cmd>,
    join: Option<thread::JoinHandle<()>>,
    samples: Arc<Mutex<Vec<f32>>>,
    error: Arc<Mutex<Option<String>>>,
    sample_rate: u32,
}

impl Recorder {
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// 共享的原始采样缓冲，流式识别线程按游标增量读取。
    pub fn buffer(&self) -> Arc<Mutex<Vec<f32>>> {
        self.samples.clone()
    }

    /// 结束录音并取回设备原始采样（未重采样）。
    pub fn stop(mut self) -> Result<Vec<f32>, String> {
        let _ = self.tx.send(Cmd::Stop);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
        if let Some(error) = self.error.lock().unwrap().take() {
            return Err(error);
        }
        let samples = std::mem::take(&mut *self.samples.lock().unwrap());
        if samples.is_empty() {
            return Err("没有录到声音".into());
        }
        Ok(samples)
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

/// 打开默认输入设备开始采集；音频只留在内存里，停止后由调用方转写。
pub fn start(app: AppHandle) -> Result<Recorder, String> {
    start_with(Some(app))
}

/// 与 [`start`] 相同，但不发 UI 事件（测试/无界面场景用）。
pub fn start_with(app: Option<AppHandle>) -> Result<Recorder, String> {
    let (tx, rx) = mpsc::channel::<Cmd>();
    let (ready_tx, ready_rx) = mpsc::channel::<Result<u32, String>>();
    let samples: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
    let error: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

    let thread_samples = samples.clone();
    let thread_error = error.clone();
    let join = thread::Builder::new()
        .name("microphone".into())
        .spawn(move || {
            let stream =
                match build_stream(app.as_ref(), thread_samples.clone(), thread_error.clone()) {
                    Ok((stream, rate)) => {
                        let _ = ready_tx.send(Ok(rate));
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
                    let level = peak(&thread_samples);
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
        Ok(Ok(rate)) => Ok(Recorder {
            tx,
            join: Some(join),
            samples,
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
    samples: Arc<Mutex<Vec<f32>>>,
    error: Arc<Mutex<Option<String>>>,
) -> Result<(cpal::Stream, u32), String> {
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or_else(|| "没有可用的麦克风".to_string())?;
    let supported = device
        .default_input_config()
        .map_err(|e| format!("读取麦克风配置失败: {e}"))?;
    let sample_rate = supported.sample_rate().0;
    let channels = supported.channels() as usize;
    let format = supported.sample_format();
    let config: cpal::StreamConfig = supported.config();
    let cap = MAX_SECONDS as usize * sample_rate as usize;

    let on_error = |app: Option<AppHandle>, slot: Arc<Mutex<Option<String>>>| {
        move |err: cpal::StreamError| {
            let message = format!("麦克风异常: {err}");
            if let Ok(mut guard) = slot.lock() {
                *guard = Some(message.clone());
            }
            if let Some(app) = &app {
                let _ = app.emit("audio:error", message);
            }
        }
    };
    let stream = match format {
        SampleFormat::F32 => {
            let buf = samples.clone();
            device.build_input_stream(
                &config,
                move |data: &[f32], _| append(data, channels, &buf, cap),
                on_error(app.cloned(), error.clone()),
                None,
            )
        }
        SampleFormat::I16 => {
            let buf = samples.clone();
            device.build_input_stream(
                &config,
                move |data: &[i16], _| append(data, channels, &buf, cap),
                on_error(app.cloned(), error.clone()),
                None,
            )
        }
        SampleFormat::U16 => {
            let buf = samples.clone();
            device.build_input_stream(
                &config,
                move |data: &[u16], _| append(data, channels, &buf, cap),
                on_error(app.cloned(), error.clone()),
                None,
            )
        }
        SampleFormat::I32 => {
            let buf = samples.clone();
            device.build_input_stream(
                &config,
                move |data: &[i32], _| append(data, channels, &buf, cap),
                on_error(app.cloned(), error.clone()),
                None,
            )
        }
        other => return Err(format!("不支持的麦克风采样格式: {other:?}")),
    }
    .map_err(|e| format!("无法打开麦克风（{format:?}）: {e}"))?;

    Ok((stream, sample_rate))
}

/// 多声道混成单声道并写入共享缓冲；F32/I16/U16 统一归一化到 -1..1。
fn append<T>(data: &[T], channels: usize, buffer: &Arc<Mutex<Vec<f32>>>, cap: usize)
where
    T: SizedSample + Sample,
    f32: FromSample<T>,
{
    let Ok(mut out) = buffer.lock() else { return };
    if out.len() >= cap {
        return;
    }
    for frame in data.chunks(channels.max(1)) {
        let mut sum = 0f32;
        for sample in frame {
            sum += f32::from_sample(*sample);
        }
        out.push(sum / frame.len().max(1) as f32);
    }
}

fn peak(samples: &Arc<Mutex<Vec<f32>>>) -> f32 {
    let Ok(buf) = samples.lock() else { return 0.0 };
    let tail = &buf[buf.len().saturating_sub(2_048)..];
    if tail.is_empty() {
        return 0.0;
    }
    let rms = (tail.iter().map(|s| s * s).sum::<f32>() / tail.len() as f32).sqrt();
    (rms * 6.0).clamp(0.0, 1.0)
}

/// 设备采样率转 16 kHz（带抗混叠滤波），已符合则原样返回。
pub fn resample_to_16k(samples: Vec<f32>, rate: u32) -> Vec<f32> {
    if rate == TARGET_RATE || samples.is_empty() {
        return samples;
    }
    match LinearResampler::create(rate as i32, TARGET_RATE as i32) {
        Some(resampler) => {
            let clamped: Vec<f32> = samples.into_iter().map(|s| s.clamp(-1.0, 1.0)).collect();
            resampler.resample(&clamped, true)
        }
        None => samples,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_input_is_usable() {
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
        let recorder = start_with(None).expect("打开麦克风");
        let rate = recorder.sample_rate();
        std::thread::sleep(std::time::Duration::from_secs(2));
        let samples = recorder.stop().expect("停止录音");
        let peak = samples.iter().fold(0f32, |acc, s| acc.max(s.abs()));
        println!("采到 {} 个采样 @ {rate} Hz，峰值 {peak:.4}", samples.len());
        assert!(samples.len() > rate as usize, "录音样本太少");
        assert!(peak <= 1.0, "样本超出 [-1, 1]");
    }

    #[test]
    fn resample_48k_to_16k_keeps_duration() {
        let samples = vec![0.0f32; 48_000];
        let out = resample_to_16k(samples, 48_000);
        assert!((out.len() as i64 - 16_000).abs() < 64, "got {}", out.len());
    }
}
