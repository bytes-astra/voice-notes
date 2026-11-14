//! 真模型端到端：同批多个 endpoint 的实时事件与最终 segments 使用相同且非零的时间区间。
//! VOICE_NOTES_STREAMING_MODEL_DIR=... cargo test --test streaming_endpoint_time -- --ignored --nocapture

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use voice_notes_lib::asr::{Segment, StreamEvent, StreamingSession, Transcript};
use voice_notes_lib::audio::SampleQueue;
use voice_notes_lib::engine::StreamingEngine;
use voice_notes_lib::models::StreamingPaths;
use voice_notes_lib::record::run_streaming_core;

const RATE: usize = 16_000;
const REPEATS: usize = 3;
const SILENCE: usize = 3 * RATE;

fn engine() -> Arc<StreamingEngine> {
    let dir = PathBuf::from(
        std::env::var("VOICE_NOTES_STREAMING_MODEL_DIR")
            .expect("必须设置 VOICE_NOTES_STREAMING_MODEL_DIR，不能静默跳过"),
    );
    let paths = StreamingPaths {
        dir: dir.display().to_string(),
        encoder: dir.join("encoder.int8.onnx").display().to_string(),
        decoder: dir.join("decoder.onnx").display().to_string(),
        joiner: dir.join("joiner.int8.onnx").display().to_string(),
        tokens: dir.join("tokens.txt").display().to_string(),
        vad: Some(dir.join("silero_vad.onnx").display().to_string()),
    };
    for file in [
        &paths.encoder,
        &paths.decoder,
        &paths.joiner,
        &paths.tokens,
        paths.vad.as_ref().unwrap(),
    ] {
        assert!(std::path::Path::new(file).is_file(), "缺少模型文件：{file}");
    }
    Arc::new(StreamingEngine::new(&paths, &[]).expect("加载真实 Zipformer 与 VAD"))
}

fn batch() -> (Vec<f32>, usize) {
    let wave = sherpa_onnx::Wave::read("tests/fixtures/zh.wav").expect("读取已有中文 fixture");
    let period = wave.samples().len() + SILENCE;
    let mut samples = Vec::with_capacity(REPEATS * period);
    for _ in 0..REPEATS {
        samples.extend_from_slice(wave.samples());
        samples.resize(samples.len() + SILENCE, 0.0);
    }
    (samples, period)
}

fn collect(events: Vec<StreamEvent>, segments: &mut Vec<Segment>) {
    for event in events {
        if let StreamEvent::Segment(segment) = event {
            segments.push(segment);
        }
    }
}

fn chunked_reference(engine: Arc<StreamingEngine>, samples: &[f32]) -> Transcript {
    let mut session = StreamingSession::new(engine, None);
    for chunk in samples.chunks(3_200) {
        session.advance_samples(chunk.len());
        session.push(chunk);
    }
    session.finish();
    session.transcript()
}

fn assert_matches_chunked(label: &str, actual: &Transcript, expected: &Transcript) {
    assert_eq!(actual.segments.len(), expected.segments.len());
    for (index, (actual, expected)) in actual.segments.iter().zip(&expected.segments).enumerate() {
        println!(
            "{label} comparison {}: batch=[{:.4}, {:.4}), chunked=[{:.4}, {:.4})",
            index + 1,
            actual.start,
            actual.end,
            expected.start,
            expected.end
        );
        assert_eq!(actual.text, expected.text);
        assert_eq!(actual.start, expected.start, "整批与正常分片的起点必须一致");
        assert_eq!(actual.end, expected.end, "整批与正常分片的终点必须一致");
    }
}

fn assert_segments(
    label: &str,
    events: &[Segment],
    transcript: &Transcript,
    period: usize,
    total_samples: usize,
) {
    assert_eq!(events.len(), REPEATS, "{label} 应识别三句：{events:?}");
    assert_eq!(events.len(), transcript.segments.len());
    let duration = total_samples as f32 / RATE as f32;
    assert!((transcript.duration_seconds - duration).abs() < 1e-4);
    let mut previous_end = 0.0;
    for (index, (event, final_segment)) in events.iter().zip(&transcript.segments).enumerate() {
        assert!(!event.text.trim().is_empty());
        assert_eq!(
            event.text, events[0].text,
            "同一 fixture 的三次转写内容应一致"
        );
        assert!(event.start.is_finite() && event.end.is_finite());
        assert!(event.start >= 0.0 && event.end <= duration + 1e-4);
        assert!(
            event.start < event.end,
            "{label} 第 {} 句为零时长：{event:?}",
            index + 1
        );
        assert!(
            event.start >= previous_end,
            "{label} 句子时间不能重叠或倒退"
        );
        // 每句归属独立的音频周期，不能全部挤到批末；不假定起点等于声学起音。
        let period_start = (index * period) as f32 / RATE as f32;
        let period_end = ((index + 1) * period) as f32 / RATE as f32;
        assert!(
            event.start >= period_start && event.start < period_end,
            "{label} 第 {} 句不在对应音频周期：{event:?}",
            index + 1
        );
        assert!(event.end > period_start && event.end <= period_end);
        assert_eq!(event.text, final_segment.text);
        assert_eq!(event.start, final_segment.start);
        assert_eq!(event.end, final_segment.end);
        println!(
            "{label} segment {}: event=[{:.4}, {:.4}), final=[{:.4}, {:.4}), chars={}",
            index + 1,
            event.start,
            event.end,
            final_segment.start,
            final_segment.end,
            event.text.chars().count()
        );
        previous_end = event.end;
    }
    println!(
        "{label}: duration={duration:.4}s, realtime_segments={}, final_segments={}",
        events.len(),
        transcript.segments.len()
    );
}

#[test]
#[ignore = "requires VOICE_NOTES_STREAMING_MODEL_DIR; run explicitly with --ignored"]
fn one_batch_multiple_endpoints_match_realtime_and_final_segments() {
    let (samples, period) = batch();
    let engine = engine();
    let mut session = StreamingSession::new(engine.clone(), None);
    session.advance_samples(samples.len());
    let mut segments = Vec::new();
    // 只调用一次 push；收尾前的 Segment 只能来自模型 endpoint，不能靠 finish 凑数。
    collect(session.push(&samples), &mut segments);
    assert_eq!(
        segments.len(),
        REPEATS,
        "单次 push 必须实际触发三个 endpoint"
    );
    collect(session.finish(), &mut segments);
    let transcript = session.transcript();
    assert_segments("session", &segments, &transcript, period, samples.len());
    assert_matches_chunked("session", &transcript, &chunked_reference(engine, &samples));
}

/// 门控关闭后立即停止：旧的 pending 不能被 reset 后的识别器识别成零时长的新句。
#[test]
#[ignore = "requires VOICE_NOTES_STREAMING_MODEL_DIR; run explicitly with --ignored"]
fn gated_stop_keeps_pending_in_the_previous_sentence() {
    use std::sync::mpsc;
    use std::time::Duration;

    let engine = engine();
    let wave = sherpa_onnx::Wave::read("tests/fixtures/zh.wav").expect("读取中文 fixture");
    let queue = SampleQueue::new(RATE as u32);
    // 1 个样本留在 pending；接下来一批也在宽限内送入识别器。
    queue.push(&wave.samples()[..48_001]);
    let producer_queue = queue.clone();
    let stop = Arc::new(AtomicBool::new(false));
    let producer_stop = stop.clone();
    let (tx, rx) = mpsc::channel::<()>();
    let producer = std::thread::spawn(move || {
        let mut completed = false;
        if rx.recv_timeout(Duration::from_secs(30)).is_ok() {
            std::thread::sleep(Duration::from_millis(100));
            producer_queue.push(&vec![0.0; 6_400]);
            if rx.recv_timeout(Duration::from_secs(30)).is_ok() {
                std::thread::sleep(Duration::from_millis(900));
                producer_queue.push(&vec![0.0; 6_400]);
                completed = rx.recv_timeout(Duration::from_secs(30)).is_ok();
            }
        }
        producer_stop.store(true, Ordering::SeqCst);
        completed
    });
    let mut segments = Vec::new();
    let transcript = run_streaming_core(
        engine,
        None,
        queue,
        RATE as u32,
        stop,
        Arc::new(AtomicBool::new(false)),
        |events| {
            collect(events, &mut segments);
            let _ = tx.send(());
        },
    );
    assert!(
        producer.join().expect("采样线程应正常退出"),
        "必须处理三批采样"
    );
    assert!(!segments.is_empty(), "真实语音必须产生定稿");
    assert_eq!(segments.len(), transcript.segments.len());
    let mut previous_end = 0.0;
    for (event, final_segment) in segments.iter().zip(&transcript.segments) {
        assert!(event.start < event.end, "句子不能被压成零时长：{event:?}");
        assert!(event.start >= previous_end, "句子不能重叠：{event:?}");
        assert!(
            event.end <= 54_401.0 / RATE as f32,
            "不能把门控静音当作识别音频"
        );
        assert_eq!(
            (event.start, event.end),
            (final_segment.start, final_segment.end)
        );
        assert_eq!(event.text, final_segment.text);
        previous_end = event.end;
    }
}

#[test]
#[ignore = "requires VOICE_NOTES_STREAMING_MODEL_DIR; run explicitly with --ignored"]
fn one_capture_batch_matches_realtime_and_final_segments() {
    let (samples, period) = batch();
    let engine = engine();
    let queue = SampleQueue::new(RATE as u32);
    queue.push(&samples);
    let stop = Arc::new(AtomicBool::new(false));
    let stopper = stop.clone();
    // 兜底：首批音频若始终没被判为语音就不会 emit，循环在队列取空后会一直空转。
    // 超时主动置 stop，让下面的断言失败，而不是把测试挂死。
    let watchdog = stop.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(120));
        watchdog.store(true, Ordering::SeqCst);
    });
    let mut segments = Vec::new();
    let mut first_batch_segments = None;
    let transcript = run_streaming_core(
        engine.clone(),
        None,
        queue,
        RATE as u32,
        stop,
        Arc::new(AtomicBool::new(false)),
        |events| {
            collect(events, &mut segments);
            first_batch_segments.get_or_insert(segments.len());
            // 一批音频已完整处理后结束循环；没触发足够 endpoint 时断言失败而不是挂起。
            stopper.store(true, Ordering::SeqCst);
        },
    );
    assert_eq!(
        first_batch_segments,
        Some(REPEATS),
        "生产首批回调必须已有三个 endpoint"
    );
    assert_segments("record", &segments, &transcript, period, samples.len());
    assert_matches_chunked("record", &transcript, &chunked_reference(engine, &samples));
}
