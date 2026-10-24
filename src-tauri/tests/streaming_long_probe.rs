//! 探针：**流式**模式下的长录音，看有多少音频没变成文字。
//!
//! 直接驱动应用自己的编排函数 `record::run_streaming_core`（不复制逻辑），
//! 用一个后台线程模拟麦克风往队列里灌音频。
//!
//! 量三件事：
//!   1. 灌进队列的采样数
//!   2. 切出来的句子覆盖了哪些时间段
//!   3. 没被覆盖的时间 = 音频到不了文字的地方
//!
//! 运行：
//!   VOICE_NOTES_STREAMING_MODEL_DIR=... cargo test --test streaming_long_probe -- --nocapture

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use voice_notes_lib::asr::StreamEvent;
use voice_notes_lib::audio::SampleQueue;
use voice_notes_lib::engine::StreamingEngine;
use voice_notes_lib::models::StreamingPaths;
use voice_notes_lib::record;

mod common;
use common::trim_silence;

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
fn probe_streaming_long_recording() {
    let Some(paths) = model_paths() else {
        eprintln!("跳过：找不到流式模型（VOICE_NOTES_STREAMING_MODEL_DIR）");
        return;
    };
    let engine = Arc::new(StreamingEngine::new(&paths, &[]).expect("加载流式模型"));
    let wave = sherpa_onnx::Wave::read("tests/fixtures/zh.wav").expect("读取音频");
    let speech = trim_silence(wave.samples(), 0.02);

    for repeat in [1usize, 8, 20] {
        let mut long = Vec::with_capacity(speech.len() * repeat);
        for _ in 0..repeat {
            long.extend_from_slice(&speech);
        }
        let seconds = long.len() as f32 / 16000.0;

        let queue = SampleQueue::with_limit(None); // 不限：让探针只看识别本身丢不丢
        let stop = Arc::new(AtomicBool::new(false));
        let discard = Arc::new(AtomicBool::new(false));

        // 后台线程模拟麦克风：按 48 kHz 的节奏灌（这里直接快进灌完）
        let producer = {
            let queue = queue.clone();
            let stop = stop.clone();
            std::thread::spawn(move || {
                for chunk in long.chunks(4800) {
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    queue.push(chunk);
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                stop.store(true, Ordering::Relaxed);
            })
        };

        let mut segments: Vec<(f32, f32, String)> = Vec::new();
        let transcript = record::run_streaming_core(
            engine.clone(),
            None,
            queue,
            16_000,
            stop.clone(),
            discard,
            |events| {
                for event in events {
                    if let StreamEvent::Segment(segment) = event {
                        segments.push((segment.start, segment.end, segment.text.clone()));
                    }
                }
            },
        );
        producer.join().ok();

        let fed = transcript.duration_seconds;
        let covered: f32 = segments.iter().map(|(_, end, _)| *end).sum::<f32>()
            - segments.iter().map(|(start, _, _)| *start).sum::<f32>();
        let chars: usize = segments.iter().map(|(_, _, t)| t.chars().count()).sum();

        println!("\n════ 连续语音 {seconds:.1}s（流式）════");
        println!(
            "  喂进 {fed:.1}s → 切出 {} 段，覆盖 {covered:.1}s，**没覆盖 {:.1}s**",
            segments.len(),
            fed - covered
        );
        println!("  文字共 {chars} 字");
        for (start, end, text) in segments.iter().take(10) {
            println!("    [{start:>7.1}s → {end:>7.1}s] {text}");
        }
        if segments.len() > 10 {
            println!("    …（还有 {} 段）", segments.len() - 10);
        }
    }
}
