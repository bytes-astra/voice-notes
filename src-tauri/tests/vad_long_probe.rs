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

/// 模型目录（同其他冒烟测试）
fn model_dir() -> Option<PathBuf> {
    let dir = std::env::var("VOICE_NOTES_MODEL_DIR").ok().map(PathBuf::from)?;
    dir.join("silero_vad.onnx").is_file().then_some(dir)
}

fn fixture(name: &str) -> Option<PathBuf> {
    let path = std::path::Path::new("tests/fixtures").join(name);
    path.exists().then_some(path)
}

/// 去掉首尾静音，让多段语音拼起来是「连续说话」
fn trim_silence(samples: &[f32], threshold: f32) -> Vec<f32> {
    let loud = |w: &[f32]| w.iter().fold(0.0f32, |a, b| a.max(b.abs())) > threshold;
    let win = 160usize; // 10 ms
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

/// VAD 配置：跟应用里 engine.rs 的完全一样（阈值 0.5 / 静音 0.5s / 最长语音 30s）
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

    // 拼成 N 段连续语音（段与段之间不留静音 → VAD 找不到静音点）
    for repeat in [1usize, 4, 12, 24] {
        let mut long = Vec::with_capacity(speech.len() * repeat);
        for _ in 0..repeat {
            long.extend_from_slice(&speech);
        }
        let seconds = long.len() as f32 / 16000.0;
        println!("\n════ 连续语音 {seconds:.1}s（{repeat} 段拼接）════");
        for buffer in [60.0f32, 120.0, 300.0] {
            let (fed, out, segments) = measure(&config, buffer, &long);
            let lost = fed.saturating_sub(out);
            println!(
                "  VAD 缓冲 {buffer:>5.0}s → 喂进 {fed:>8} 采样，切出 {out:>8} 采样（{segments:>3} 句），**被吞 {lost:>8} 采样 = {:.1}s**",
                lost as f32 / 16000.0
            );
        }
    }
}
