//! 探针：**背景噪声 / 背景人声**对整段会话的影响。
//!
//! 背景：2026-10-28 线上排查 —— 整段模式有一大段音频没变成文字（997 秒录音里
//! 只有 168 秒被识别成语音）。已排除：
//!   * 采集缓冲丢样（`record.rs` 没有「识别跟不上」告警）、时钟漂移、采样率不一致
//!   * VAD 吞长连续语音（`vad_long_probe`：140 秒连续语音只被吞 0.4 秒）
//!   * 低增益（`gain_probe`：-56.5 dBFS 仍认出 99.2% 语音）
//!
//! 剩下的怀疑：**背景噪声让 Silero 的概率一直高于 `neg_threshold`（0.35），
//! VAD 永远找不到静音** → 句子无限增长 → 走「>90 秒切 30 秒窗」那条路径 ——
//! 而那条路径在项目里从没被真实数据验证过（`docs/research/2026-10-23-audio-vad.md` §5.3
//! 说「>90 s 的单句极少见，优先级低」）。
//!
//! 这个探针给语音加背景噪声 / 背景人声，量：
//!   1. 切出来的句数、最长的一句有多长（收紧态有没有兜住）
//!   2. 覆盖了多少秒、有多少秒没变成文字（这才是用户看到的症状）
//!   3. 字数
//!
//! 运行：
//!   VOICE_NOTES_MODEL_DIR=... cargo test --test noise_probe -- --nocapture

use voice_notes_lib::asr::{OfflineEvent, OfflineSession};
use voice_notes_lib::engine::Engine;

mod common;
use common::{model_paths, trim_silence};

/// 可复现的白噪声（xorshift，不用 rand，探针结果才稳定）。
fn white_noise(len: usize, seed: u64) -> Vec<f32> {
    let mut state = seed | 1;
    let mut out = Vec::with_capacity(len);
    for _ in 0..len {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        // 转成 [-1, 1) 的均匀分布
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
    let s_rms = rms(signal).max(1e-9);
    let n_rms = rms(noise).max(1e-9);
    let scale = s_rms / n_rms / 10f32.powf(snr_db / 20.0);
    let mut out = Vec::with_capacity(signal.len());
    for i in 0..signal.len() {
        out.push(signal[i] + noise[i % noise.len()] * scale);
    }
    out
}

/// 跑完整会话，返回（句数，最长句秒数，覆盖秒数，字数）。
fn run(engine: &std::sync::Arc<Engine>, vad: &sherpa_onnx::VadModelConfig, audio: &[f32]) -> (usize, f32, f32, usize) {
    let mut session = OfflineSession::new(engine.clone(), vad.clone());
    let mut segments: Vec<(f32, f32, String)> = Vec::new();
    for chunk in audio.chunks(512) {
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
    let longest = segments.iter().map(|(s, e, _)| e - s).fold(0.0f32, f32::max);
    let chars: usize = segments.iter().map(|(_, _, t)| t.chars().count()).sum();
    (segments.len(), longest, covered, chars)
}

#[test]
fn probe_noisy_background() {
    let Some(paths) = model_paths() else {
        eprintln!("跳过：找不到 SenseVoice 模型");
        return;
    };
    let engine = std::sync::Arc::new(Engine::new(&paths, "zh", true).expect("加载模型"));
    let vad = engine.vad_config().expect("需要 VAD 模型");

    let wave = sherpa_onnx::Wave::read("tests/fixtures/zh.wav").expect("读取音频");
    let speech = trim_silence(wave.samples(), 0.02);
    // 背景人声：另一段真人语音
    let babble_wave = sherpa_onnx::Wave::read("tests/fixtures/zh-tail.wav").expect("读取音频");
    let babble = trim_silence(babble_wave.samples(), 0.02);

    // 主体：8 段连续语音（≈35 秒）
    let mut signal = Vec::new();
    for _ in 0..8 {
        signal.extend_from_slice(&speech);
    }
    let seconds = signal.len() as f32 / 16000.0;

    println!("主体：{seconds:.1}s 连续语音（{} 段）", 8);
    println!("\n{:<22} {:>6} {:>10} {:>10} {:>6}", "场景", "句数", "最长句", "覆盖", "字数");

    let report = |label: &str, audio: &[f32]| {
        let (segments, longest, covered, chars) = run(&engine, &vad, audio);
        println!(
            "{:<22} {:>6} {:>9.1}s {:>9.1}s {:>6}",
            label,
            segments,
            longest,
            covered,
            chars
        );
    };

    report("干净（无噪声）", &signal);

    for snr in [30.0f32, 20.0, 10.0, 5.0, 0.0, -5.0] {
        let noise = white_noise(signal.len(), 0x9E3779B97F4A7C15);
        report(&format!("白噪声 SNR {snr:.0}dB"), &mix(&signal, &noise, snr));
    }

    for snr in [20.0f32, 10.0, 5.0, 0.0, -5.0] {
        report(&format!("背景人声 SNR {snr:.0}dB"), &mix(&signal, &babble, snr));
    }

    // ── 超长单句：>90 秒才会走「切 30 秒窗」那条路径 ──
    // 背景人声压到 SNR ≤5 dB 时 VAD 完全切不开，整段变成一个句子。
    // 把它拉长到 260 秒，那一句就超过了 MAX_SINGLE_SHOT_SECONDS(90)，
    // 必须走 recognize_segment() 的切窗 + merge_transcripts 路径 ——
    // 而那条路径在项目里从没被真实数据验证过（docs/research/2026-10-23-audio-vad.md §5.3
    // 说「>90 s 的单句极少见，优先级低」）。
    println!("\n════ 超长单句（走切窗路径）════");
    let mut long_signal = Vec::new();
    for _ in 0..60 {
        long_signal.extend_from_slice(&speech);
    }
    let long_seconds = long_signal.len() as f32 / 16000.0;
    println!("主体：{long_seconds:.1}s");
    println!("\n{:<22} {:>6} {:>10} {:>10} {:>6}", "场景", "句数", "最长句", "覆盖", "字数");
    report("干净 260s", &long_signal);
    for snr in [0.0f32, -5.0, -10.0] {
        let noise = white_noise(long_signal.len(), 0x9E3779B97F4A7C15);
        report(&format!("白噪声 SNR {snr:.0}dB"), &mix(&long_signal, &noise, snr));
    }
    for snr in [0.0f32, -5.0, -10.0] {
        report(&format!("背景人声 SNR {snr:.0}dB"), &mix(&long_signal, &babble, snr));
    }

    // ── 有能量但不是语音：告警必须响 ──
    // 20 Hz 嗡鸣：峰值够高（有能量），但 Silero 判它不是语音 → 音频会被
    // sherpa 的 VAD 直接丢掉。这正是「静默丢音频」那条告警要抓的场景。
    println!("\n════ 有能量但不是语音（告警应该响）════");
    for level in [0.5f32, 0.05] {
        let hum: Vec<f32> = (0..long_signal.len())
            .map(|i| level * (2.0 * std::f32::consts::PI * 20.0 * i as f32 / 16000.0).sin())
            .collect();
        report(&format!("20Hz 嗡鸣 峰值 {level}"), &hum);
    }
}
