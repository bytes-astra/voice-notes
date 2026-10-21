//! 流式识别冒烟测试：把测试音频按 100ms 切片喂进去，模拟实时麦克风。
//!
//!   VOICE_NOTES_STREAMING_MODEL_DIR=/path/to/streaming cargo test --test streaming_smoke -- --nocapture

use std::path::PathBuf;
use std::sync::Arc;

use voice_notes_lib::asr::{StreamEvent, StreamingEngine, StreamingSession};
use voice_notes_lib::models::StreamingPaths;

fn model_paths() -> Option<StreamingPaths> {
    let dir = std::env::var("VOICE_NOTES_STREAMING_MODEL_DIR")
        .ok()
        .map(PathBuf::from)?;
    let encoder = dir.join("encoder.int8.onnx");
    let decoder = dir.join("decoder.onnx");
    let joiner = dir.join("joiner.int8.onnx");
    let tokens = dir.join("tokens.txt");
    if !(encoder.is_file() && decoder.is_file() && joiner.is_file() && tokens.is_file()) {
        return None;
    }
    Some(StreamingPaths {
        dir: dir.display().to_string(),
        encoder: encoder.display().to_string(),
        decoder: decoder.display().to_string(),
        joiner: joiner.display().to_string(),
        tokens: tokens.display().to_string(),
        vad: [dir.join("silero_vad.onnx")]
            .into_iter()
            .find(|path| path.is_file())
            .map(|path| path.display().to_string()),
    })
}

#[test]
fn streams_chinese_fixture() {
    let Some(paths) = model_paths() else {
        eprintln!("跳过：找不到流式模型（VOICE_NOTES_STREAMING_MODEL_DIR）");
        return;
    };

    let engine = Arc::new(StreamingEngine::new(&paths).expect("加载流式模型"));
    let wave = sherpa_onnx::Wave::read("tests/fixtures/zh.wav").expect("读取测试音频");
    let samples = wave.samples();

    let mut session = StreamingSession::new(engine, None);
    let mut partials = 0;
    let mut segments = 0;

    // 100ms 一片，模拟 16 kHz 实时输入。
    for chunk in samples.chunks(1_600) {
        for event in session.push(chunk) {
            match event {
                StreamEvent::Partial(text) => {
                    partials += 1;
                    println!("partial: {text}");
                }
                StreamEvent::Segment(segment) => {
                    segments += 1;
                    println!("segment: {}", segment.text);
                }
            }
        }
    }
    for event in session.finish() {
        if let StreamEvent::Segment(segment) = event {
            segments += 1;
            println!("final segment: {}", segment.text);
        }
    }

    let transcript = session.transcript();
    println!(
        "结果：{}（{} 段，{} 次中间结果，音频 {:.2}s）",
        transcript.text, segments, partials, transcript.duration_seconds
    );

    assert!(!transcript.text.is_empty(), "流式结果为空");
    assert!(partials > 0, "没有产生中间结果");
    assert!(transcript.duration_seconds > 0.0);
}
