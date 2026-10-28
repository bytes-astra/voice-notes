//! `asr` 模块的单元测试（从 `asr.rs` 拆出）。

use super::*;
use crate::models;
use std::path::PathBuf;

#[test]
fn pre_roll_only_for_speech_like_prefix() {
    let speech = vec![0.1f32; 32_000];
    let segment = vec![0.1f32; 16_000];
    // 0.2s 处起句，段前是语音级能量 → 回扩
    assert!(should_pre_roll(3_200, &speech, &segment));
    // 段前是静音/底噪 → 不回扩（否则会把噪声塞进结果）
    assert!(!should_pre_roll(3_200, &vec![0.001f32; 32_000], &segment));
    // 从 0 开始没有可回扩的音频
    assert!(!should_pre_roll(0, &speech, &segment));
    // 2s 处起句：前面有真实音频，sherpa 的 padding 够用
    assert!(!should_pre_roll(32_000, &speech, &segment));
}

#[test]
fn punctuates_sentences() {
    let Some(model) = std::env::var("VOICE_NOTES_PUNCT_MODEL").ok() else {
        eprintln!("跳过：未设置 VOICE_NOTES_PUNCT_MODEL");
        return;
    };
    let engine = PunctuationEngine::new(&model).expect("加载标点模型");
    let out = engine.apply("开放时间早上九点至下午五点");
    println!("标点结果：{out}");
    assert!(
        out.contains('。') || out.contains('，'),
        "没有补出标点: {out}"
    );
}

#[test]
fn streams_with_punctuation() {
    let (Some(model_dir), Some(punct_model)) = (
        std::env::var("VOICE_NOTES_STREAMING_MODEL_DIR").ok(),
        std::env::var("VOICE_NOTES_PUNCT_MODEL").ok(),
    ) else {
        eprintln!("跳过：缺少流式模型或标点模型环境变量");
        return;
    };
    let dir = PathBuf::from(model_dir);
    let paths = models::StreamingPaths {
        dir: dir.display().to_string(),
        encoder: dir.join("encoder.int8.onnx").display().to_string(),
        decoder: dir.join("decoder.onnx").display().to_string(),
        joiner: dir.join("joiner.int8.onnx").display().to_string(),
        tokens: dir.join("tokens.txt").display().to_string(),
        vad: None,
    };
    let engine = Arc::new(StreamingEngine::new(&paths, &[]).expect("加载流式模型"));
    let punctuator = Arc::new(PunctuationEngine::new(&punct_model).expect("加载标点模型"));
    let wave = sherpa_onnx::Wave::read("tests/fixtures/zh.wav").expect("读取测试音频");
    let mut session = StreamingSession::new(engine, Some(punctuator));
    for chunk in wave.samples().chunks(1_600) {
        session.push(chunk);
    }
    session.finish();
    let transcript = session.transcript();
    println!("带标点结果：{}", transcript.text);
    assert!(
        transcript.text.contains('。') || transcript.text.contains('，'),
        "流式结果没有标点: {}",
        transcript.text
    );
}
