//! 探针：**输入音量**对 VAD 与识别器的双重影响。
//!
//! 背景：2026-10-28 线上排查 —— 整段模式有一大段音频没变成文字（997 秒录音里
//! 只有 168 秒被识别成语音）。已排除：采集缓冲丢样（`record.rs` 没有「识别跟不上」
//! 的告警）、时钟漂移（音频时钟与墙钟同步）、采样率/重采样不一致。
//! `vad_long_probe` 证明 **VAD 不吞正常音量的长连续语音**（140 秒只被吞 0.4 秒）。
//!
//! 剩下的怀疑：`docs/research/2026-10-23-audio-vad.md` §3.4 写的
//! 「**增益过低** → 语音概率上不去，VAD 判不出语音（整句丢失）；SenseVoice 也会
//! 输出空串」。而 sherpa 的 VAD 对判为非语音的音频是**直接丢弃**的
//! （`voice-activity-detector.cc` 的 else 分支只 `buffer_.Pop`），零痕迹 ——
//! 所以低增益时段的话会整段人间蒸发，日志、转写、界面上都看不出来。
//!
//! 这个探针把同一段真人语音按不同增益喂进去，分别量：
//!   1. VAD 判成语音的采样数（Silero 的概率有没有过 threshold 0.5）
//!   2. 完整会话切出的句子数与字数（识别器有没有输出）
//!
//! 运行：
//!   VOICE_NOTES_MODEL_DIR=... cargo test --test gain_probe -- --nocapture

use voice_notes_lib::asr::{OfflineEvent, OfflineSession};
use voice_notes_lib::engine::Engine;

mod common;
use common::{model_paths, trim_silence};

/// 只跑 VAD：喂进去多少采样、切出来多少采样（判成语音的部分）。
fn vad_speech_samples(
    config: &sherpa_onnx::VadModelConfig,
    buffer_seconds: f32,
    speech: &[f32],
) -> (usize, usize) {
    let Some(detector) = sherpa_onnx::VoiceActivityDetector::create(config, buffer_seconds) else {
        return (0, 0);
    };
    let mut fed = 0usize;
    let mut out = 0usize;
    for chunk in speech.chunks(512) {
        fed += chunk.len();
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
    (fed, out)
}

/// 跑完整会话：句子数、覆盖的秒数、字数。
fn offline_session(engine: &std::sync::Arc<Engine>, vad: &sherpa_onnx::VadModelConfig, speech: &[f32]) -> (usize, f32, usize) {
    let mut session = OfflineSession::new(engine.clone(), vad.clone());
    let mut segments: Vec<(f32, f32, String)> = Vec::new();
    for chunk in speech.chunks(512) {
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
    (segments.len(), covered, chars)
}

#[test]
fn probe_low_gain() {
    let Some(paths) = model_paths() else {
        eprintln!("跳过：找不到 SenseVoice 模型");
        return;
    };
    let engine = std::sync::Arc::new(Engine::new(&paths, "zh", true).expect("加载模型"));
    let vad = engine.vad_config().expect("需要 VAD 模型");

    let wave = sherpa_onnx::Wave::read("tests/fixtures/zh.wav").expect("读取音频");
    let speech = trim_silence(wave.samples(), 0.02);
    let peak = speech.iter().fold(0.0f32, |a, b| a.max(b.abs()));
    let rms = (speech.iter().map(|s| s * s).sum::<f32>() / speech.len() as f32).sqrt();
    println!(
        "素材：{:.2}s，峰值 {peak:.4}，RMS {rms:.4}（{:.1} dBFS RMS）",
        speech.len() as f32 / 16000.0,
        20.0 * rms.max(1e-9).log10()
    );

    // 同一段语音，重复 8 次 ≈ 35 秒连续说话（够触发 sherpa 的「收紧态」）。
    let mut long = Vec::new();
    for _ in 0..8 {
        long.extend_from_slice(&speech);
    }
    let seconds = long.len() as f32 / 16000.0;

    println!("\n{:<8} {:>10} {:>12} {:>10} {:>8} {:>8}", "增益", "峰值", "dBFS", "VAD语音", "句数", "字数");
    for gain in [1.0f32, 0.5, 0.2, 0.1, 0.05, 0.02, 0.01, 0.003] {
        let scaled: Vec<f32> = long.iter().map(|s| s * gain).collect();
        let peak = scaled.iter().fold(0.0f32, |a, b| a.max(b.abs()));
        let rms = (scaled.iter().map(|s| s * s).sum::<f32>() / scaled.len() as f32).sqrt();
        let (_, vad_out) = vad_speech_samples(&vad, 60.0, &scaled);
        let (segments, _covered, chars) = offline_session(&engine, &vad, &scaled);
        println!(
            "{:<8} {:>10.4} {:>12} {:>9.1}% {:>8} {:>8}",
            format!("×{gain}"),
            peak,
            format!("{:.1}", 20.0 * rms.max(1e-9).log10()),
            vad_out as f32 / scaled.len() as f32 * 100.0,
            segments,
            chars
        );
    }
    println!("（素材 {seconds:.1}s）");
}
