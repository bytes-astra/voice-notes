//! 输入电平探针：增益过低会不会让 VAD 不触发？削波会不会让识别掉字？
//!
//! 只打印、不断言。目的是回答「P2-5（AGC / 削波提示）」到底要不要做：
//! - 如果把输入压到某个电平以下 VAD 就不切句了 → 「增益过低」提示是刚需；
//! - 如果稍微一放大就削波掉字 → 「削波」提示是刚需；
//! - 如果这两条都不成立 → 那它只是个锦上添花的功能，可以不做。
//!
//! 用法：
//! ```bash
//! cd src-tauri
//! VOICE_NOTES_MODEL_DIR=... cargo test --test level_probe -- --nocapture --test-threads=1
//! ```

use std::path::{Path, PathBuf};

use sherpa_onnx::{SileroVadModelConfig, VadModelConfig, VoiceActivityDetector};
use voice_notes_lib::engine::Engine;
use voice_notes_lib::models::ModelPaths;

mod common;
use common::edit_distance;

fn model_dir() -> Option<PathBuf> {
    std::env::var("VOICE_NOTES_MODEL_DIR").ok().map(PathBuf::from)
}

fn paths(dir: &Path) -> ModelPaths {
    let model = [dir.join("model.int8.onnx"), dir.join("model.onnx")]
        .into_iter()
        .find(|p| p.is_file())
        .expect("找不到 SenseVoice 模型");
    ModelPaths {
        dir: dir.display().to_string(),
        model: model.display().to_string(),
        tokens: dir.join("tokens.txt").display().to_string(),
        vad: Some(dir.join("silero_vad.onnx").display().to_string()),
        source: "probe".into(),
    }
}

/// 真人语料（minds14 电话录音，带听写文本可算 CER）
fn real_corpus() -> Vec<(String, PathBuf)> {
    let dir = Path::new("tests/fixtures/quality");
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("wav"))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("real-"))
                .unwrap_or(false)
        })
        .collect();
    files.sort();
    files
        .into_iter()
        .map(|p| {
            let name = p.file_stem().unwrap().to_string_lossy().to_string();
            (name, p)
        })
        .collect()
}

fn reference_for(file: &Path) -> Option<String> {
    std::fs::read_to_string(file.with_extension("txt"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// 去掉空白与标点（与 quality_probe 的 norm 同口径）
fn norm(text: &str) -> String {
    text.chars()
        .filter(|c| {
            !c.is_whitespace()
                && !c.is_ascii_punctuation()
                && !"，。！？、；：“”‘’（）《》…—～·「」".contains(*c)
        })
        .collect()
}

fn vad_config(vad_path: &str) -> VadModelConfig {
    VadModelConfig {
        silero_vad: SileroVadModelConfig {
            model: Some(vad_path.to_string()),
            threshold: 0.5,
            min_silence_duration: 0.3,
            min_speech_duration: 0.25,
            window_size: 512,
            max_speech_duration: 30.0,
        },
        sample_rate: 16_000,
        num_threads: 1,
        provider: Some("cpu".into()),
        debug: false,
        ..Default::default()
    }
}

/// 跑一遍 VAD，返回切出的段（起点采样, 长度采样）
fn vad_segments(vad_path: &str, samples: &[f32]) -> Vec<(usize, usize)> {
    let Some(detector) = VoiceActivityDetector::create(&vad_config(vad_path), 60.0) else {
        return Vec::new();
    };
    detector.reset();
    for chunk in samples.chunks(512) {
        detector.accept_waveform(chunk);
    }
    detector.flush();
    let mut spans = Vec::new();
    while let Some(segment) = detector.front() {
        spans.push((segment.start() as usize, segment.samples().len()));
        detector.pop();
    }
    spans
}

/// 增益过低会不会让 VAD 不触发？
///
/// 衰减 N dB 后看 VAD 还切不切得出语音段。段数掉到 0 或语音秒数大幅缩水，
/// 就说明「增益过低」不是假设，是真会发生的故障。
#[test]
fn probe_vad_gain_sensitivity() {
    let Some(dir) = model_dir() else {
        eprintln!("跳过：未设置 VOICE_NOTES_MODEL_DIR");
        return;
    };
    let paths = paths(&dir);
    let Some(vad_path) = paths.vad.clone() else {
        eprintln!("跳过：没有 VAD 模型");
        return;
    };
    let files = real_corpus();
    if files.is_empty() {
        eprintln!("跳过：没有 real-* 语料");
        return;
    }

    println!("=== 衰减后 VAD 还能切出多少（段数/语音秒）===");
    for (name, file) in files.iter().take(8) {
        let wave = sherpa_onnx::Wave::read(file.to_str().unwrap()).expect("读取音频");
        let mut line = format!("[{name}]");
        for db in [0.0f32, -6.0, -12.0, -18.0, -24.0, -30.0, -40.0] {
            let gain = 10f32.powf(db / 20.0);
            let scaled: Vec<f32> = wave.samples().iter().map(|s| s * gain).collect();
            let spans = vad_segments(&vad_path, &scaled);
            let speech: f32 = spans.iter().map(|(_, len)| *len as f32).sum::<f32>() / 16_000.0;
            line.push_str(&format!(" {db:>4.0}dB={}段/{:.1}s", spans.len(), speech));
        }
        println!("{line}");
    }
}

/// 削波会不会让识别掉字？
///
/// 放大后削到 [-1, 1]，看 CER 变化多少、削波比例多少。
#[test]
fn probe_clipping_cer() {
    let Some(dir) = model_dir() else {
        eprintln!("跳过：未设置 VOICE_NOTES_MODEL_DIR");
        return;
    };
    let paths = paths(&dir);
    let engine = Engine::new(&paths, "zh", true).expect("加载模型");
    let files = real_corpus();
    if files.is_empty() {
        eprintln!("跳过：没有 real-* 语料");
        return;
    }

    println!("=== 放大削波后的 CER（×gain = 削波比例）===");
    for (name, file) in files.iter().take(8) {
        let wave = sherpa_onnx::Wave::read(file.to_str().unwrap()).expect("读取音频");
        let Some(reference) = reference_for(file) else {
            continue;
        };
        let refc: Vec<char> = norm(&reference).chars().collect();
        let mut line = format!("[{name}]");
        for gain in [1.0f32, 1.5, 2.0, 4.0] {
            let clipped: Vec<f32> = wave
                .samples()
                .iter()
                .map(|s| (s * gain).clamp(-1.0, 1.0))
                .collect();
            let clipped_ratio =
                clipped.iter().filter(|s| s.abs() >= 0.999).count() as f32 / clipped.len() as f32;
            let text = engine.recognize(&clipped);
            let hyp: Vec<char> = norm(&text).chars().collect();
            let dist = edit_distance(&hyp, &refc);
            line.push_str(&format!(
                " ×{gain}=CER {:.1}% 削波{:.1}%",
                dist as f64 / refc.len().max(1) as f64 * 100.0,
                clipped_ratio * 100.0
            ));
        }
        println!("{line}");
    }
}

/// 电平表的口径：LevelRing::peak() 是 rms×6，不是真峰值。
///
/// 差距有多大？rms×6 什么时候会明显小于/大于真实峰值？
#[test]
fn probe_level_meter_scale() {
    let files = real_corpus();
    if files.is_empty() {
        eprintln!("跳过：没有 real-* 语料");
        return;
    }

    println!("=== 电平表口径对比（RMS×6 vs 真峰值 max|s|）===");
    let mut max_gap = 0.0f32;
    for (name, file) in files.iter().take(8) {
        let wave = sherpa_onnx::Wave::read(file.to_str().unwrap()).expect("读取音频");
        let samples = wave.samples();
        if samples.is_empty() {
            continue;
        }
        let rms = (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt();
        let peak = samples.iter().fold(0.0f32, |acc, s| acc.max(s.abs()));
        let shown = (rms * 6.0).clamp(0.0, 1.0);
        max_gap = max_gap.max((shown - peak).abs());
        println!(
            "[{name}] RMS={:.4} 表显={:.3} 真峰值={:.3} 差={:+.3}",
            rms,
            shown,
            peak,
            shown - peak
        );
    }
    println!("最大偏差：{max_gap:.3}");
}
