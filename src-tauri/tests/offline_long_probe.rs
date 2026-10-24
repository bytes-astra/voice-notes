//! 探针：**长段连续语音**跑完整的离线会话，看有多少音频没变成文字。
//!
//! 背景：用户反馈「一长段转写后，后面的录音会有丢失」。日志显示 162 秒的录音
//! 全部喂进了识别会话（`recorded_samples` 对得上），只出 20 句 308 字，
//! 且**没有**「识别跟不上」的告警 —— 所以不是队列丢的。
//! 另一个探针（vad_long_probe）证明 VAD 也不丢（105 秒只被吞 0.3 秒）。
//!
//! 这个探针量三件事：
//!   1. 喂进会话的采样数
//!   2. 切出来的句子覆盖了哪些时间段（段落的 start/end）
//!   3. 没被任何段落覆盖的时间 = 音频到不了文字的地方
//!
//! 运行：
//!   VOICE_NOTES_MODEL_DIR=... cargo test --test offline_long_probe -- --nocapture

use std::path::PathBuf;

use voice_notes_lib::asr::{OfflineEvent, OfflineSession};
use voice_notes_lib::engine::Engine;
use voice_notes_lib::models::ModelPaths;
use sherpa_onnx::{SileroVadModelConfig, VadModelConfig};

fn model_paths() -> Option<ModelPaths> {
    let dir = std::env::var("VOICE_NOTES_MODEL_DIR")
        .ok()
        .map(PathBuf::from)?;
    let model = [dir.join("model.int8.onnx"), dir.join("model.onnx")]
        .into_iter()
        .find(|path| path.is_file())?;
    let tokens = dir.join("tokens.txt");
    if !tokens.is_file() {
        return None;
    }
    let vad = [
        dir.parent().map(|p| p.join("silero/silero_vad.onnx")),
        Some(dir.join("silero_vad.onnx")),
    ]
    .into_iter()
    .flatten()
    .find(|path| path.is_file());

    Some(ModelPaths {
        dir: dir.display().to_string(),
        model: model.display().to_string(),
        tokens: tokens.display().to_string(),
        vad: vad.map(|path| path.display().to_string()),
        source: "test".into(),
    })
}

/// 去掉首尾静音，让多段语音拼起来是「连续说话」
fn trim_silence(samples: &[f32], threshold: f32) -> Vec<f32> {
    let loud = |w: &[f32]| w.iter().fold(0.0f32, |a, b| a.max(b.abs())) > threshold;
    let win = 160usize;
    let first = samples
        .chunks(win)
        .position(loud)
        .map(|i| i * win)
        .unwrap_or(0);
    let last = samples
        .chunks(win)
        .rposition(loud)
        .map(|i| (i + 1) * win)
        .unwrap_or(samples.len());
    samples[first..last.min(samples.len())].to_vec()
}

#[test]
fn probe_long_recording_loss() {
    let Some(paths) = model_paths() else {
        eprintln!("跳过：找不到 SenseVoice 模型");
        return;
    };
    let engine = std::sync::Arc::new(Engine::new(&paths, "zh", true).expect("加载模型"));

    // 和应用一样的 VAD 配置
    let vad_path = paths.vad.clone().expect("需要 VAD 模型");
    let vad = VadModelConfig {
        silero_vad: SileroVadModelConfig {
            model: Some(vad_path),
            threshold: 0.5,
            min_silence_duration: 0.5,
            min_speech_duration: 0.25,
            window_size: 512,
            max_speech_duration: 30.0,
        },
        sample_rate: 16_000,
        num_threads: 1,
        provider: Some("cpu".into()),
        debug: false,
        ..Default::default()
    };

    let wave = sherpa_onnx::Wave::read("tests/fixtures/zh.wav").expect("读取音频");
    let speech = trim_silence(wave.samples(), 0.02);

    // gap 是采样数，按它插静音 —— 只当布尔用的话，换个标签会静默量成同一段音频。
    for (label, gap) in [
        ("连续语音", 0usize),
        ("说话+2秒静音+说话", 16_000 * 2),
        ("说话+10秒静音+说话", 16_000 * 10),
    ] {
        let repeat = 3usize;
        let mut long = Vec::new();
        for i in 0..repeat {
            long.extend_from_slice(&speech);
            if gap > 0 && i + 1 < repeat {
                long.extend(std::iter::repeat(0.0f32).take(gap));
            }
        }
        let seconds = long.len() as f32 / 16000.0;

        let mut session = OfflineSession::new(engine.clone(), vad.clone());
        let mut segments: Vec<(f32, f32, String)> = Vec::new();
        // 和应用一样按 512 一块喂
        for chunk in long.chunks(512) {
            for event in session.push(chunk) {
                let OfflineEvent::Segment(segment) = event;
                segments.push((segment.start, segment.end, segment.text.clone()));
            }
        }
        for event in session.finish() {
            let OfflineEvent::Segment(segment) = event;
            segments.push((segment.start, segment.end, segment.text.clone()));
        }

        let fed = session.recorded_samples() as f32 / 16000.0;
        let covered: f32 = segments.iter().map(|(_, end, _)| *end).sum::<f32>()
            - segments.iter().map(|(start, _, _)| *start).sum::<f32>();
        let chars: usize = segments.iter().map(|(_, _, t)| t.chars().count()).sum();

        println!("\n════ {label}：{seconds:.1}s ════");
        println!(
            "  喂进 {fed:.1}s → 切出 {} 段，覆盖 {covered:.1}s，**没覆盖 {:.1}s**",
            segments.len(),
            fed - covered
        );
        println!("  文字共 {chars} 字");
        for (start, end, text) in segments.iter().take(12) {
            println!(
                "    [{start:>7.1}s → {end:>7.1}s] {}",
                if text.chars().count() > 40 {
                    format!("{}…", text.chars().take(40).collect::<String>())
                } else {
                    text.clone()
                }
            );
        }
        if segments.len() > 12 {
            println!("    …（还有 {} 段）", segments.len() - 12);
        }
    }
}
