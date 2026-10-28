//! 探针：**长段连续语音**经过 VAD 后，有多少音频被吞掉。
//!
//! 背景：用户反馈「一长段转写后，后面的录音会有丢失」。日志显示 162 秒的录音
//! 全部喂进了识别会话（`recorded_samples` 计数对得上），但只出了 20 句 ——
//! 所以音频是在 VAD 这一环丢的，不是队列丢的（队列丢会有「识别跟不上」的告警，
//! 日志里没有）。
//!
//! 这个探针只打印不断言：把一段真人语音重复成连续的长语音（中间不留静音），
//! 喂给 VAD，看**喂进去多少采样、切出来的句子一共多少采样**。
//! 差值就是被 VAD 吞掉的音频。
//!
//! 运行：
//!   VOICE_NOTES_MODEL_DIR=... cargo test --test vad_long_probe -- --nocapture

use std::path::PathBuf;

mod common;
use common::trim_silence;

/// 模型目录（同其他冒烟测试）
fn model_dir() -> Option<PathBuf> {
    let dir = std::env::var("VOICE_NOTES_MODEL_DIR").ok().map(PathBuf::from)?;
    dir.join("silero_vad.onnx").is_file().then_some(dir)
}

fn fixture(name: &str) -> Option<PathBuf> {
    let path = std::path::Path::new("tests/fixtures").join(name);
    path.exists().then_some(path)
}

/// VAD 配置：跟应用里 engine.rs 的完全一样（阈值 0.5 / 静音 0.5s / 最长语音 30s）
///
/// 这个探针只加载 silero_vad.onnx（不加载 161 MB 的 SenseVoice），
/// 所以拿不到 `Engine::vad_config()` —— 只能自己抄一份。**改了 engine.rs 要回来同步**。
fn vad_config(vad_path: &std::path::Path) -> sherpa_onnx::VadModelConfig {
    sherpa_onnx::VadModelConfig {
        silero_vad: sherpa_onnx::SileroVadModelConfig {
            model: Some(vad_path.display().to_string()),
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
    }
}

/// 喂一段长语音，统计 VAD 吐出来的音频总量
fn measure(
    config: &sherpa_onnx::VadModelConfig,
    buffer_seconds: f32,
    speech: &[f32],
) -> (usize, usize, usize) {
    let Some(detector) = sherpa_onnx::VoiceActivityDetector::create(config, buffer_seconds) else {
        return (0, 0, 0);
    };
    let mut fed = 0usize;
    let mut in_segments = 0usize;
    let mut segments = 0usize;
    // 和应用一样：喂一段就取一次已完成的句子
    for chunk in speech.chunks(512) {
        fed += chunk.len();
        detector.accept_waveform(chunk);
        while let Some(segment) = detector.front() {
            in_segments += segment.samples().len();
            segments += 1;
            detector.pop();
        }
    }
    // 收尾：把还没成句的冲出来
    detector.flush();
    while let Some(segment) = detector.front() {
        in_segments += segment.samples().len();
        segments += 1;
        detector.pop();
    }
    (fed, in_segments, segments)
}

#[test]
fn probe_long_continuous_speech() {
    let Some(dir) = model_dir() else {
        eprintln!("跳过：未设置 VOICE_NOTES_MODEL_DIR");
        return;
    };
    let vad_path = dir.join("silero_vad.onnx");
    let Some(fixture) = fixture("zh.wav") else {
        eprintln!("跳过：缺少 zh.wav");
        return;
    };
    let wave = sherpa_onnx::Wave::read(fixture.to_str().unwrap()).expect("读取音频");
    let speech = trim_silence(wave.samples(), 0.02);
    println!(
        "素材：{} 采样（{:.2}s），去掉首尾静音后 {} 采样（{:.2}s）",
        wave.samples().len(),
        wave.samples().len() as f32 / 16000.0,
        speech.len(),
        speech.len() as f32 / 16000.0
    );

    let config = vad_config(&vad_path);

    // 场景 A：连续语音（段与段之间不留静音）
    // 场景 B：说话 → 静音 → 说话 → 静音 …（用户反馈的丢字场景）
    // gap 是采样数，按它插静音 —— 别只当布尔用，否则改标签会静默量同一段音频。
    //
    // 连续语音要测**多个长度**：sherpa 的「收紧态」（threshold 0.5→0.9、min_silence
    // 0.5→0.1）在 buffer 超过 sample_rate × max_speech_duration（30 s）时才触发，
    // 只测十几秒根本碰不到它 —— 而用户丢的正是几分钟的连续说话。
    for (label, gap, repeat) in [
        ("连续语音 ×4", 0usize, 4usize),
        ("连续语音 ×8", 0usize, 8usize),
        ("连续语音 ×16", 0usize, 16usize),
        ("连续语音 ×32", 0usize, 32usize),
        ("说话+2秒静音+说话", 16_000 * 2, 4usize),
    ] {
        let mut long = Vec::new();
        for i in 0..repeat {
            long.extend_from_slice(&speech);
            if gap > 0 && i + 1 < repeat {
                long.extend(std::iter::repeat(0.0f32).take(gap));
            }
        }
        let seconds = long.len() as f32 / 16000.0;
        println!("\n════ {label}：{seconds:.1}s（{repeat} 段）════");
        let buffer = 60.0f32;
        let (fed, out, segments) = measure(&config, buffer, &long);
        let lost = fed.saturating_sub(out);
        println!(
            "  VAD 缓冲 {buffer:>5.0}s → 喂进 {fed:>8} 采样，切出 {out:>8} 采样（{segments:>3} 句），**被吞 {lost:>8} 采样 = {:.1}s**",
            lost as f32 / 16000.0
        );
    }
}
