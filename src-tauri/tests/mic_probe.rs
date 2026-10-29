//! 探针：**抓一段真实麦克风音频**，定位「整段模式整段丢音频」的根因。
//!
//! 背景：2026-10-28 线上排查 —— 整段模式里成片的音频没变成文字。
//! 线上日志（带仪表的构建）给出的判据：
//!
//! ```text
//! [WARN] 连续 10.1s 没切出句子（期间音频峰值 0.0330）
//! [WARN] 连续 20.1s 没切出句子（期间音频峰值 0.0799）
//! 录音结束（整段）：时长 31.8s，句数 1
//! ```
//!
//! 峰值只有 0.033（-30 dBFS），但 `gain_probe` 实测**干净语音压到 -56.5 dBFS
//! 仍被认出 99.2%** —— 所以「音量小」本身解释不了。这说明那 0.033 的峰值
//! **不是人声**：要么是直流/低频隆隆声占了大头，要么人声埋在噪声底之下。
//!
//! 这个探针抓真实麦克风音频（和 `audio.rs` 同一套设备协商 + 同样的
//! LinearResampler 重采样），落成 WAV，然后对比三条处理路径：
//!   1. 原样
//!   2. 去直流（一阶 DC blocker，30 Hz）
//!   3. 峰值归一化（把 99 分位峰值拉到 0.5，模拟 AGC 的最简形态）
//!
//! 三条路径各自的「VAD 判成语音的比例」和「识别出的字数」一摆出来，
//! 根因就定了：是直流/隆隆声（去直流能救）、还是人声被噪声底淹没
//! （归一化/降噪能救）、还是信号本身就不对。
//!
//! 运行：
//!   VOICE_NOTES_MODEL_DIR=... cargo test --test mic_probe -- --nocapture
//!
//! 环境变量：
//!   VOICE_NOTES_MIC_SECONDS=20   抓几秒（默认 15）
//!   VOICE_NOTES_MIC_OUT=/tmp/x   落盘前缀（默认 /tmp/voice-notes-mic）
//!   VOICE_NOTES_MIC_FILE=/tmp/x  不抓麦克风，直接分析已有的 16 kHz WAV（离线复现）

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait};
use cpal::Sample;
use voice_notes_lib::asr::{OfflineEvent, OfflineSession};
use voice_notes_lib::engine::Engine;

mod common;
use common::model_paths;

/// 抓 N 秒的单声道音频（混单声道 + 归一化到 -1..1，和 `audio.rs` 的 Capture 一致）。
fn capture(seconds: f32) -> Option<(Vec<f32>, u32)> {
    let host = cpal::default_host();
    let device = host.default_input_device()?;
    let supported = device.default_input_config().ok()?;
    let rate = supported.sample_rate().0;
    let channels = supported.channels() as usize;
    let format = supported.sample_format();
    let config: cpal::StreamConfig = supported.config();

    println!(
        "设备：{}（{format:?}，{rate} Hz，{channels} 声道）",
        device.name().unwrap_or_else(|_| "<无名>".into())
    );

    let collected: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
    let failed = Arc::new(AtomicBool::new(false));

    macro_rules! build {
        ($ty:ty) => {{
            let collected = collected.clone();
            let failed = failed.clone();
            device.build_input_stream(
                &config,
                move |data: &[$ty], _: &cpal::InputCallbackInfo| {
                    let mut guard = collected.lock().unwrap();
                    for frame in data.chunks(channels.max(1)) {
                        let mut sum = 0f32;
                        for sample in frame {
                            sum += f32::from_sample(*sample);
                        }
                        guard.push(sum / frame.len() as f32);
                    }
                },
                move |err| {
                    eprintln!("采集错误：{err}");
                    failed.store(true, Ordering::SeqCst);
                },
                None,
            )
        }};
    }

    let stream = match format {
        cpal::SampleFormat::F32 => build!(f32),
        cpal::SampleFormat::I16 => build!(i16),
        cpal::SampleFormat::U16 => build!(u16),
        cpal::SampleFormat::I32 => build!(i32),
        other => {
            eprintln!("不支持的采样格式：{other:?}");
            return None;
        }
    }
    .ok()?;

    println!("抓 {seconds:.0} 秒 —— 请对着麦克风说话…");
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs_f32(seconds) && !failed.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(100));
    }
    drop(stream);

    let samples = collected.lock().unwrap().clone();
    Some((samples, rate))
}

/// 一阶 DC blocker（高通）：`y[n] = x[n] - x[n-1] + R·y[n-1]`。
///
/// R 由截止频率决定：R = 1 - 2π·fc/fs。
fn dc_block(samples: &[f32], sample_rate: u32, cutoff_hz: f32) -> Vec<f32> {
    let r = 1.0 - 2.0 * std::f32::consts::PI * cutoff_hz / sample_rate as f32;
    let mut out = Vec::with_capacity(samples.len());
    let mut x1 = 0.0f32;
    let mut y1 = 0.0f32;
    for &x in samples {
        let y = x - x1 + r * y1;
        x1 = x;
        y1 = y;
        out.push(y);
    }
    out
}

/// AGC：**用生产代码**（`voice_notes_lib::audio::Agc`）逐块处理，和 record.rs 一样。
///
/// 探针自己写一份 AGC 就失去意义了 —— 得测真正上线的那份。
fn agc(samples: &[f32]) -> Vec<f32> {
    let mut agc = voice_notes_lib::audio::Agc::new();
    let mut out = samples.to_vec();
    for chunk in out.chunks_mut(512) {
        agc.process(chunk);
    }
    out
}

/// AGC：峰值归一化 + 软限幅：目标峰值 `target`，超出部分用 tanh 压回来。
///
/// 这才是能用的 AGC 形态 —— 之前那个无上限的 `peak_normalize` 会把峰值推到
/// 3.5（远超 ±1.0），模型看到的是削波信号，识别反而变差（实测 91.3% → 85.8%）。
fn peak_normalize_limited(samples: &[f32], target: f32) -> Vec<f32> {
    let mut abs: Vec<f32> = samples.iter().map(|s| s.abs()).collect();
    abs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p99 = abs.get(abs.len() * 99 / 100).copied().unwrap_or(0.0);
    if p99 <= 1e-9 {
        return samples.to_vec();
    }
    let gain = target / p99;
    samples
        .iter()
        .map(|s| {
            let x = s * gain;
            if x.abs() <= target {
                x
            } else {
                target * x.tanh()
            }
        })
        .collect()
}

fn stats(samples: &[f32]) -> (f32, f32, f32) {
    let peak = samples.iter().fold(0.0f32, |a, b| a.max(b.abs()));
    let rms = (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt();
    let mean = samples.iter().sum::<f32>() / samples.len() as f32;
    (peak, rms, mean)
}

/// VAD 判成语音的采样比例。
fn vad_speech_ratio(
    config: &sherpa_onnx::VadModelConfig,
    buffer_seconds: f32,
    samples: &[f32],
) -> f32 {
    let Some(detector) = sherpa_onnx::VoiceActivityDetector::create(config, buffer_seconds) else {
        return 0.0;
    };
    let mut out = 0usize;
    for chunk in samples.chunks(512) {
        detector.accept_waveform(chunk);
        while let Some(segment) = detector.front() {
            out += segment.samples().len();
            detector.pop();
        }
    }
    detector.flush();
    while let Some(segment) = detector.front() {
        out += segment.samples().len();
        detector.pop();
    }
    out as f32 / samples.len() as f32 * 100.0
}

/// 找出「句子之间的空档」，量每段空档的时长和峰值。
///
/// 这就是「丢音频」问题的答案形式：空档里有声音（峰值多少）还是彻底安静。
/// 空档里**有声音但没句子** = VAD 判它不是语音；空档里**彻底安静** = 本来就没声音。
fn report_gaps(samples: &[f32], segments: &[(f32, f32, String)]) {
    let mut cursor = 0.0f32;
    let mut total_gap = 0.0f32;
    let mut loud_gaps = 0usize;
    for (start, end, _) in segments {
        if *start - cursor < 1.0 {
            cursor = cursor.max(*end);
            continue;
        }
        let from = (cursor * 16_000.0) as usize;
        let to = ((*start) * 16_000.0) as usize;
        let slice = &samples[from.min(samples.len())..to.min(samples.len())];
        let (peak, rms, _) = stats(slice);
        let seconds = *start - cursor;
        total_gap += seconds;
        let has_sound = peak >= 0.01;
        if has_sound {
            loud_gaps += 1;
        }
        println!(
            "  空档 {:.1}s → {:.1}s（{seconds:>6.1}s）峰值 {peak:.4}｜RMS {rms:.4} —— {}",
            cursor,
            *start,
            if has_sound {
                "**有声音但没识别出句子**"
            } else {
                "彻底安静"
            }
        );
        cursor = *end;
    }
    println!(
        "  合计空档 {total_gap:.1}s（其中 {loud_gaps} 段有声音）"
    );
}

/// 跑完整会话：句子列表、覆盖秒数、字数。
fn run_session(
    engine: &std::sync::Arc<Engine>,
    vad: &sherpa_onnx::VadModelConfig,
    samples: &[f32],
) -> (Vec<(f32, f32, String)>, f32, usize) {
    let mut session = OfflineSession::new(engine.clone(), vad.clone());
    let mut segments: Vec<(f32, f32, String)> = Vec::new();
    for chunk in samples.chunks(512) {
        for event in session.push(chunk) {
            let OfflineEvent::Segment(segment) = event;
            segments.push((segment.start, segment.end, segment.text.clone()));
        }
    }
    for event in session.finish() {
        let OfflineEvent::Segment(segment) = event;
        segments.push((segment.start, segment.end, segment.text.clone()));
    }
    let covered: f32 = segments.iter().map(|(_, end, _)| *end).sum::<f32>()
        - segments.iter().map(|(start, _, _)| *start).sum::<f32>();
    let chars: usize = segments.iter().map(|(_, _, t)| t.chars().count()).sum();
    (segments, covered, chars)
}

#[test]
fn probe_real_mic() {
    let seconds: f32 = std::env::var("VOICE_NOTES_MIC_SECONDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(15.0);
    let out_prefix = std::env::var("VOICE_NOTES_MIC_OUT")
        .unwrap_or_else(|_| "/tmp/voice-notes-mic".into());

    // 离线模式：分析已有的 WAV（抓过一次之后反复迭代用，不用重新录）
    let mono16: Vec<f32> = if let Ok(path) = std::env::var("VOICE_NOTES_MIC_FILE") {
        let wave = sherpa_onnx::Wave::read(&path).expect("读取 WAV");
        println!("离线分析：{path}（{} 采样）", wave.num_samples());
        wave.samples().to_vec()
    } else {
        let Some((raw, rate)) = capture(seconds) else {
            eprintln!("跳过：抓不到麦克风");
            return;
        };
        let seconds_captured = raw.len() as f32 / rate as f32;
        println!("抓到 {:.1}s（{rate} Hz，{} 采样）", seconds_captured, raw.len());

        // 落盘：原始 48k + 重采样后的 16k（和 record.rs 走同一个 LinearResampler）
        let raw_path = format!("{out_prefix}-raw.wav");
        sherpa_onnx::write(&raw_path, &raw, rate as i32);
        let resampler = sherpa_onnx::LinearResampler::create(rate as i32, 16_000);
        let mono16: Vec<f32> = match &resampler {
            Some(r) => {
                let mut out = r.resample(&raw, false);
                out.extend(r.resample(&[], true));
                out
            }
            None => raw.clone(),
        };
        let path16 = format!("{out_prefix}-16k.wav");
        sherpa_onnx::write(&path16, &mono16, 16_000);
        println!("落盘：{raw_path}\n      {path16}");
        mono16
    };

    let (peak, rms, mean) = stats(&mono16);
    println!(
        "\n16k 信号：峰值 {peak:.4}（{:.1} dBFS），RMS {rms:.4}（{:.1} dBFS），**直流均值 {mean:.5}**",
        20.0 * peak.max(1e-9).log10(),
        20.0 * rms.max(1e-9).log10()
    );
    if mean.abs() > 0.2 * peak.max(1e-9) {
        println!("  ⚠️ 直流分量占了峰值的 {:.0}% —— 波形看着有起伏，其实是人声被直流顶掉了", mean.abs() / peak.max(1e-9) * 100.0);
    }

    let Some(paths) = model_paths() else {
        eprintln!("跳过：找不到 SenseVoice 模型（只做了音频分析）");
        return;
    };
    let engine = std::sync::Arc::new(Engine::new(&paths, "zh", true).expect("加载模型"));
    let vad = engine.vad_config().expect("需要 VAD 模型");

    let variants: Vec<(&str, Vec<f32>)> = vec![
        ("原样", mono16.clone()),
        ("去直流 30Hz", dc_block(&mono16, 16_000, 30.0)),
        ("AGC(生产代码)", agc(&mono16)),
        ("归一化+限幅 0.7", peak_normalize_limited(&mono16, 0.7)),
    ];

    println!("\n{:<16} {:>10} {:>10} {:>8} {:>10} {:>6}", "处理", "峰值", "RMS", "VAD语音", "覆盖", "字数");
    for (label, audio) in &variants {
        let (peak, rms, _) = stats(audio);
        let ratio = vad_speech_ratio(&vad, 60.0, audio);
        let (_segments, covered, chars) = run_session(&engine, &vad, audio);
        println!(
            "{:<16} {:>10.4} {:>10.4} {:>7.1}% {:>9.1}s {:>6}",
            label, peak, rms, ratio, covered, chars
        );
    }
    println!("\n（素材 {:.1}s）", mono16.len() as f32 / 16_000.0);

    // 缺口分析：每个空档里到底有没有声音、峰值多少 —— 「丢音频」问题的答案形式
    let (segments, _, _) = run_session(&engine, &vad, &mono16);
    println!("\n════ 句子时间线（{} 句）════", segments.len());
    for (start, end, text) in &segments {
        println!("  {:.1}s → {:.1}s  {}", start, end, text);
    }
    println!("\n════ 空档分析 ════");
    report_gaps(&mono16, &segments);
}
