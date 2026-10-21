//! 端到端冒烟测试：用真实模型转写一段中文语音。
//!
//! 模型不存在时自动跳过（CI/新机器上不阻塞）：
//!   VOICE_NOTES_MODEL_DIR=/path/to/models cargo test --test asr_smoke -- --nocapture

use std::path::PathBuf;

use voice_notes_lib::asr::Engine;
use voice_notes_lib::models::ModelPaths;

fn model_paths() -> Option<ModelPaths> {
    let dir = std::env::var("VOICE_NOTES_MODEL_DIR")
        .ok()
        .map(PathBuf::from)
        .or_else(|| {
            let home = std::env::var("HOME").ok()?;
            Some(PathBuf::from(home).join(".dsh/speech-to-text/sensevoice/models/sensevoice-onnx"))
        })?;

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

#[test]
fn transcribes_chinese_speech() {
    let Some(paths) = model_paths() else {
        eprintln!("跳过：找不到 SenseVoice 模型");
        return;
    };

    let engine = Engine::new(&paths, "zh", true).expect("加载模型");
    let wave = sherpa_onnx::Wave::read("tests/fixtures/zh.wav").expect("读取测试音频");
    assert_eq!(wave.sample_rate(), 16_000);

    let transcript = engine.transcribe(wave.samples());
    println!("识别结果：{}", transcript.text);
    println!(
        "音频 {:.2}s，耗时 {}ms，分句 {} 段",
        transcript.duration_seconds,
        transcript.elapsed_ms,
        transcript.segments.len()
    );

    assert!(!transcript.text.is_empty(), "转写结果为空");
    assert!(!transcript.segments.is_empty(), "VAD 没有切出语音段");
    assert!(transcript.duration_seconds > 0.0);
}

#[test]
fn joins_sensevoice_segments() {
    // 纯逻辑回归，不需要模型。
    assert_eq!(
        voice_notes_lib::asr::join_segments(["今天天气不错", "我们出去走走吧"].into_iter()),
        "今天天气不错我们出去走走吧"
    );
}
