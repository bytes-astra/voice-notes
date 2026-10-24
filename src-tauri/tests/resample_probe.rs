//! 探针：**48 kHz → 16 kHz 的重采样**对识别的影响。
//!
//! 背景：应用从 48 kHz 设备采集，用 `sherpa_onnx::LinearResampler` 线性插值降到
//! 16 kHz —— 线性插值**没有抗混叠低通**，8 kHz 以上的内容会混叠折回语音频段。
//! 而现有的质量探针用的是 16 kHz 原始音频，**整条重采样路径没被量过**。
//!
//! 这里量：把 16 kHz 素材升采样到 48 kHz（模拟采集），再按应用的方式降回来，
//! 比较「原始」和「走了一圈重采样」的识别结果。
//!
//! 运行：
//!   VOICE_NOTES_MODEL_DIR=... cargo test --test resample_probe -- --nocapture

use std::path::PathBuf;

use voice_notes_lib::engine::Engine;
use voice_notes_lib::models::ModelPaths;

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

/// 16 kHz → 48 kHz：3 倍线性插值（模拟 48 kHz 设备采到的信号）
///
/// 注意：比例必须是 3 倍（48/16），写错会让音频变慢，识别结果就没法比了。
fn upsample_16k_to_48k(samples: &[f32]) -> Vec<f32> {
    let mut out = Vec::with_capacity(samples.len() * 3);
    for i in 0..samples.len() {
        let a = samples[i];
        let b = samples.get(i + 1).copied().unwrap_or(a);
        out.push(a);
        out.push((2.0 * a + b) / 3.0);
        out.push((a + 2.0 * b) / 3.0);
    }
    out
}

/// 字符错误率（按字对齐，纯 Levenshtein）
fn cer(reference: &str, hypothesis: &str) -> f32 {
    let r: Vec<char> = reference.chars().filter(|c| !c.is_whitespace()).collect();
    let h: Vec<char> = hypothesis.chars().filter(|c| !c.is_whitespace()).collect();
    if r.is_empty() {
        return 0.0;
    }
    let mut prev: Vec<usize> = (0..=h.len()).collect();
    let mut cur = vec![0usize; h.len() + 1];
    for i in 1..=r.len() {
        cur[0] = i;
        for j in 1..=h.len() {
            let cost = usize::from(r[i - 1] != h[j - 1]);
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[h.len()] as f32 / r.len() as f32
}

#[test]
fn probe_resample_quality() {
    let Some(paths) = model_paths() else {
        eprintln!("跳过：找不到 SenseVoice 模型");
        return;
    };
    let engine = Engine::new(&paths, "zh", true).expect("加载模型");

    for fixture in ["zh.wav", "zh-tail.wav", "zh-tail2.wav"] {
        let path = std::path::Path::new("tests/fixtures").join(fixture);
        if !path.exists() {
            continue;
        }
        let wave = sherpa_onnx::Wave::read(path.to_str().unwrap()).expect("读取音频");
        let original = wave.samples();

        // 模拟：48 kHz 采集 → 线性插值降到 16 kHz（应用走的路）
        let at_48k = upsample_16k_to_48k(original);
        let resampler = sherpa_onnx::LinearResampler::create(48_000, 16_000).expect("重采样器");
        let round_trip = resampler.resample(&at_48k, true);

        let direct = engine.transcribe(original);
        let resampled = engine.transcribe(&round_trip);
        println!(
            "  长度检查：升采样 {} → 降采样 {}（原 {}，比例应为 {:.2}）",
            at_48k.len(),
            round_trip.len(),
            original.len(),
            round_trip.len() as f32 / original.len() as f32
        );

        println!("\n════ {fixture} ════");
        println!("  原始（16 kHz 直喂）：{}", direct.text);
        println!(
            "  重采样一圈（48k→16k）：{}",
            resampled.text
        );
        println!(
            "  重采样后长度 {}/{} 采样（{:.2}s vs {:.2}s）",
            round_trip.len(),
            original.len(),
            round_trip.len() as f32 / 16000.0,
            original.len() as f32 / 16000.0
        );
        // 拿原始的识别结果当参考（不是标准答案，但能看出退化）
        println!(
            "  两者 CER（以原始识别为参考）：{:.1}% / {:.1}%",
            cer(&direct.text, &resampled.text) * 100.0,
            0.0
        );
    }
}
