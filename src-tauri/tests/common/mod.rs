//! 探针共用的辅助函数。
//!
//! Rust 的集成测试每个文件都是独立 crate，没法直接互相 import —— 唯一的共享办法是
//! `tests/common/mod.rs` + 各测试里写 `mod common;`。这里放被多个探针抄来抄去的那几份。
//!
//! 每个测试只用到其中一部分，没用到的会报 `dead_code` —— 所以整块 allow。
//! （`check:rust` 带 `-D warnings`，删掉这行会让五个测试 crate 一起报错。）
#![allow(dead_code)]

use std::path::{Path, PathBuf};

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

/// 质量语料目录（不入库，本机生成或抓取）。
pub const QUALITY_DIR: &str = "tests/fixtures/quality";

/// 语料里某个前缀的文件（`tts-` / `real-` / `noisy-` / `num-` / `long-`）。
pub fn corpus(prefix: &str) -> Vec<(String, PathBuf)> {
    let dir = Path::new(QUALITY_DIR);
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
                .map(|n| n.starts_with(prefix))
                .unwrap_or(false)
        })
        .collect();
    files.sort();
    files
        .into_iter()
        .map(|p| {
            let name = p.file_name().unwrap().to_string_lossy().to_string();
            (name, p)
        })
        .collect()
}

/// 全部语料（按类别顺序）。
pub fn all_corpus() -> Vec<(String, PathBuf)> {
    corpus("real-")
        .into_iter()
        .chain(corpus("tts-"))
        .chain(corpus("noisy-"))
        .chain(corpus("long-"))
        .chain(corpus("num-"))
        .collect()
}

/// 归一化文本：去掉空白与标点，只留可比对的字符。
pub fn norm(text: &str) -> String {
    text.chars()
        .filter(|c| {
            !c.is_whitespace()
                && !c.is_ascii_punctuation()
                && !"，。！？、；：“”‘’（）《》…—～·「」".contains(*c)
        })
        .collect()
}

/// 语料的参考文本（同名 `.txt`）。没有就返回 None（只观察输出，不算 CER）。
pub fn reference_for(file: &Path) -> Option<String> {
    let txt = file.with_extension("txt");
    std::fs::read_to_string(txt)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// 语料类别（按文件名前缀）。
pub fn category(name: &str) -> &str {
    if name.starts_with("noisy-") {
        "noisy"
    } else if name.starts_with("real-") {
        "real"
    } else if name.starts_with("tts-") {
        "tts"
    } else if name.starts_with("long-") {
        "long"
    } else if name.starts_with("num-") {
        "num"
    } else {
        "legacy"
    }
}

/// CER（字符错误率）：编辑距离 / 参考文本长度。
pub fn cer(hypothesis: &str, reference: &str) -> (usize, usize) {
    let hyp: Vec<char> = norm(hypothesis).chars().collect();
    let refc: Vec<char> = norm(reference).chars().collect();
    (edit_distance(&hyp, &refc), refc.len())
}

/// 读环境变量里的浮点数（拿不到就用默认值）。
pub fn env_f64(name: &str, default: f64) -> f64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse::<f64>().ok())
        .unwrap_or(default)
}

