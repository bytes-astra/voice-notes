//! 探针：**低电平录音走完整管线**还能不能出字（`record.rs` → AGC → VAD → 识别）。
//!
//! 背景：2026-10-28 的线上问题「整段模式整段没转写」，根因是**输入电平太低** ——
//! Silero VAD 要语音概率**连续**高于阈值满 `min_speech_duration` 才触发，
//! 低电平时概率在阈值附近抖 → 永远攒不满 → `voice-activity-detector.cc` 的
//! 非语音分支只 `buffer_.Pop(...)` → 音频静默消失。
//!
//! 单测只测 `audio::Agc` 自己，**守不住「AGC 还接在 `record.rs` 里」** ——
//! 有人把它删掉，单测照样绿。这个探针走**真实的 `run_offline` 管线**，两条路对照：
//!
//!   * 不经过 AGC（直接喂 `OfflineSession`）→ 低电平下几乎没字
//!   * 经过生产管线（`run_offline`）→ 低电平下字回来了
//!
//! 运行：
//!   VOICE_NOTES_MODEL_DIR=... cargo test --test quiet_pipeline_probe -- --nocapture

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use voice_notes_lib::asr::{OfflineEvent, OfflineSession};
use voice_notes_lib::engine::Engine;

mod common;
use common::{model_paths, trim_silence};

/// 直接喂会话（**不经过 AGC**）：返回字数。
fn chars_without_agc(engine: &Arc<Engine>, vad: &sherpa_onnx::VadModelConfig, audio: &[f32]) -> usize {
    let mut session = OfflineSession::new(engine.clone(), vad.clone());
    for chunk in audio.chunks(512) {
        session.push(chunk);
    }
    session.finish();
    session.segments().iter().map(|s| s.text.chars().count()).sum()
}

/// 走生产管线（`record::run_offline`）：返回字数。
fn chars_through_pipeline(
    engine: &Arc<Engine>,
    vad: &sherpa_onnx::VadModelConfig,
    audio: &[f32],
) -> usize {
    let queue = voice_notes_lib::audio::SampleQueue::new(16_000);
    queue.push(audio);
    let stop = Arc::new(AtomicBool::new(true));
    let discard = Arc::new(AtomicBool::new(false));
    let transcript = voice_notes_lib::record::run_offline(
        engine.clone(),
        vad.clone(),
        queue,
        16_000,
        stop,
        discard,
        |_event: OfflineEvent| {},
    );
    transcript.text.chars().count()
}

/// 可复现的白噪声（xorshift，不用 rand，探针结果才稳定）。
fn white_noise(len: usize, seed: u64) -> Vec<f32> {
    let mut state = seed | 1;
    let mut out = Vec::with_capacity(len);
    for _ in 0..len {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        out.push((state >> 11) as f32 / (1u64 << 52) as f32 * 2.0 - 1.0);
    }
    out
}

fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

/// 把噪声按目标信噪比混进语音。
fn mix(signal: &[f32], noise: &[f32], snr_db: f32) -> Vec<f32> {
    let scale = rms(signal).max(1e-9) / rms(noise).max(1e-9) / 10f32.powf(snr_db / 20.0);
    signal
        .iter()
        .enumerate()
        .map(|(i, s)| s + noise[i % noise.len()] * scale)
        .collect()
}

#[test]
fn probe_quiet_recording_still_gets_transcribed() {
    let Some(paths) = model_paths() else {
        eprintln!("跳过：找不到 SenseVoice 模型");
        return;
    };
    let engine = Arc::new(Engine::new(&paths, "zh", true).expect("加载模型"));
    let vad = engine.vad_config().expect("需要 VAD 模型");

    let wave = sherpa_onnx::Wave::read("tests/fixtures/zh.wav").expect("读取音频");
    let speech = trim_silence(wave.samples(), 0.02);
    // 重复 8 次 ≈ 35 秒连续说话
    let mut signal = Vec::new();
    for _ in 0..8 {
        signal.extend_from_slice(&speech);
    }
    let noise = white_noise(signal.len(), 0x9E3779B97F4A7C15);

    println!("{:<18} {:>9} {:>11} {:>11}", "信号", "增益", "不经AGC", "经生产管线");
    for (label, snr) in [("干净人声", f32::INFINITY), ("人声+噪声 10dB", 10.0)] {
        let base = if snr.is_finite() {
            mix(&signal, &noise, snr)
        } else {
            signal.clone()
        };
        for gain in [1.0f32, 0.0316, 0.003, 0.001, 0.0005] {
            let quiet: Vec<f32> = base.iter().map(|s| s * gain).collect();
            let without = chars_without_agc(&engine, &vad, &quiet);
            let with = chars_through_pipeline(&engine, &vad, &quiet);
            println!(
                "{:<18} {:>9} {:>11} {:>11}",
                label,
                format!("×{gain}"),
                without,
                with
            );
        }
    }

    // ── 回归断言 ──
    // ① 正常电平：AGC 不能改变结果（电平够就一个样本都不动）。
    let normal: Vec<f32> = signal.clone();
    let without = chars_without_agc(&engine, &vad, &normal);
    let with = chars_through_pipeline(&engine, &vad, &normal);
    assert!(without > 50, "正常电平本身就该有字：{without}");
    assert!(
        with.abs_diff(without) <= 5,
        "正常电平不该被 AGC 改变：不经 AGC {without} 字 vs 经管线 {with} 字"
    );

    // ② 低电平：不经过 AGC 会整段丢字，经过生产管线必须把字找回来。
    //    这一条守住「AGC 还接在 record.rs 里」—— 单测只测 Agc 自己，
    //    有人把它从管线里摘掉，单测照样绿。
    let quiet: Vec<f32> = signal.iter().map(|s| s * 0.001).collect();
    let without = chars_without_agc(&engine, &vad, &quiet);
    let with = chars_through_pipeline(&engine, &vad, &quiet);
    println!("\n低电平（×0.001）：不经 AGC {without} 字 vs 经生产管线 {with} 字");
    assert!(
        with > 50,
        "低电平录音经过生产管线应该出字，实际 {with} 字（不经 AGC 是 {without} 字）"
    );
    assert!(
        with > without + 50,
        "低电平录音经过生产管线应该明显出字：不经 AGC {without} vs 经管线 {with}"
    );
}
