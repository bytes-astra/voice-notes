//! 探针共用的辅助函数。
//!
//! Rust 的集成测试每个文件都是独立 crate，没法直接互相 import —— 唯一的共享办法是
//! `tests/common/mod.rs` + 各测试里写 `mod common;`。这里放被多个探针抄来抄去的那几份。
//!
//! 每个测试只用到其中一部分，没用到的会报 `dead_code` —— 所以整块 allow。
//! （`check:rust` 带 `-D warnings`，删掉这行会让五个测试 crate 一起报错。）
#![allow(dead_code)]

use std::path::PathBuf;

use voice_notes_lib::models::ModelPaths;

/// 整段模型路径（`VOICE_NOTES_MODEL_DIR`）
pub fn model_paths() -> Option<ModelPaths> {
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
pub fn trim_silence(samples: &[f32], threshold: f32) -> Vec<f32> {
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

/// 字符级编辑距离（纯 Levenshtein）
pub fn edit_distance(a: &[char], b: &[char]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for i in 1..=a.len() {
        cur[0] = i;
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}
