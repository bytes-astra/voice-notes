//! `asr` 模块的单元测试（从 `asr.rs` 拆出）。

use super::*;
use crate::models;
use std::path::PathBuf;

#[test]
fn externally_advanced_audio_is_not_counted_again() {
    let Some(dir) = std::env::var("VOICE_NOTES_STREAMING_MODEL_DIR")
        .ok()
        .map(PathBuf::from)
    else {
        eprintln!("跳过：未设置 VOICE_NOTES_STREAMING_MODEL_DIR");
        return;
    };
    let paths = models::StreamingPaths {
        dir: dir.display().to_string(),
        encoder: dir.join("encoder.int8.onnx").display().to_string(),
        decoder: dir.join("decoder.onnx").display().to_string(),
        joiner: dir.join("joiner.int8.onnx").display().to_string(),
        tokens: dir.join("tokens.txt").display().to_string(),
        vad: None,
    };
    let engine = Arc::new(StreamingEngine::new(&paths, &[]).expect("加载流式模型"));
    let mut session = StreamingSession::new(engine, None);
    let silence = vec![0.0f32; STREAM_CHUNK];
    session.advance_time(&silence);
    session.push(&silence);
    session.advance_time(&silence);
    session.finish();
    let transcript = session.transcript();
    assert_eq!(transcript.duration_seconds, 0.4);
}

#[test]
fn direct_streaming_time_excludes_only_synthetic_silence() {
    let Some(dir) = std::env::var("VOICE_NOTES_STREAMING_MODEL_DIR")
        .ok()
        .map(PathBuf::from)
    else {
        eprintln!("跳过：未设置 VOICE_NOTES_STREAMING_MODEL_DIR");
        return;
    };
    let paths = models::StreamingPaths {
        dir: dir.display().to_string(),
        encoder: dir.join("encoder.int8.onnx").display().to_string(),
        decoder: dir.join("decoder.onnx").display().to_string(),
        joiner: dir.join("joiner.int8.onnx").display().to_string(),
        tokens: dir.join("tokens.txt").display().to_string(),
        vad: None,
    };
    let engine = Arc::new(StreamingEngine::new(&paths, &[]).expect("加载流式模型"));
    let mut session = StreamingSession::new(engine, None);
    session.push(&vec![0.0f32; STREAM_CHUNK + 1]);
    session.finish();
    assert_eq!(
        session.transcript().duration_seconds,
        (STREAM_CHUNK + 1) as f32 / SAMPLE_RATE as f32
    );
}
