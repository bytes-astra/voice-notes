//! `record` 模块的单元测试（从 `record.rs` 拆出）。

use super::*;
use crate::models;
use std::path::PathBuf;

/// 测试用的整段引擎 + VAD 配置（没有模型就跳过）。
fn offline_engine() -> Option<(Arc<Engine>, sherpa_onnx::VadModelConfig)> {
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
    let mut vad_candidates = vec![dir.join("silero_vad.onnx")];
    if let Some(parent) = dir.parent() {
        vad_candidates.push(parent.join("silero_vad.onnx"));
    }
    let vad_model = vad_candidates.into_iter().find(|path| path.is_file())?;
    let paths = models::ModelPaths {
        dir: dir.display().to_string(),
        model: model.display().to_string(),
        tokens: tokens.display().to_string(),
        vad: Some(vad_model.display().to_string()),
        source: "test".into(),
    };
    let engine = Arc::new(Engine::new(&paths, "zh", true).expect("加载模型"));
    let vad = engine.vad_config().expect("VAD 配置");
    Some((engine, vad))
}

fn streaming_engine() -> Option<Arc<StreamingEngine>> {
    let dir = std::env::var("VOICE_NOTES_STREAMING_MODEL_DIR")
        .ok()
        .map(PathBuf::from)?;
    let paths = models::StreamingPaths {
        dir: dir.display().to_string(),
        encoder: dir.join("encoder.int8.onnx").display().to_string(),
        decoder: dir.join("decoder.onnx").display().to_string(),
        joiner: dir.join("joiner.int8.onnx").display().to_string(),
        tokens: dir.join("tokens.txt").display().to_string(),
        vad: [dir.join("silero_vad.onnx")]
            .into_iter()
            .find(|path| path.is_file())
            .map(|path| path.display().to_string()),
    };
    Some(Arc::new(StreamingEngine::new(&paths, &[]).expect("加载流式模型")))
}

#[test]
fn streaming_duration_includes_leading_silence() {
    let Some(engine) = streaming_engine() else {
        eprintln!("跳过：未设置 VOICE_NOTES_STREAMING_MODEL_DIR");
        return;
    };
    let queue = audio::SampleQueue::new(16_000);
    let wave = sherpa_onnx::Wave::read("tests/fixtures/zh.wav").expect("读取测试音频");
    let mut audio = vec![0.0f32; 19_200];
    audio.extend_from_slice(&wave.samples()[..76_800]);
    queue.push(&audio);
    let stop = Arc::new(AtomicBool::new(false));
    let stopper = stop.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(2_000));
        stopper.store(true, Ordering::Relaxed);
    });
    let transcript = run_streaming_core(
        engine,
        None,
        queue,
        16_000,
        stop,
        Arc::new(AtomicBool::new(false)),
        |_| {},
    );
    assert_eq!(
        transcript.duration_seconds,
        (audio.len() / asr::SAMPLE_RATE as usize) as f32
    );
}

#[test]
fn streaming_duration_includes_audio_arriving_after_stop() {
    let Some(engine) = streaming_engine() else {
        eprintln!("跳过：未设置 VOICE_NOTES_STREAMING_MODEL_DIR");
        return;
    };
    let queue = audio::SampleQueue::new(16_000);
    queue.push(&vec![0.0f32; 3_200]);
    let transcript = run_streaming_core(
        engine,
        None,
        queue,
        16_000,
        Arc::new(AtomicBool::new(true)),
        Arc::new(AtomicBool::new(false)),
        |_| {},
    );
    assert_eq!(transcript.duration_seconds, 0.2);
}

#[test]
fn streaming_dropped_audio_advances_recording_clock() {
    let Some(engine) = streaming_engine() else {
        eprintln!("跳过：未设置 VOICE_NOTES_STREAMING_MODEL_DIR");
        return;
    };
    let queue = audio::SampleQueue::with_limit(Some(1_000));
    queue.push(&vec![0.0f32; 17_000]);
    let transcript = run_streaming_core(
        engine,
        None,
        queue,
        16_000,
        Arc::new(AtomicBool::new(true)),
        Arc::new(AtomicBool::new(false)),
        |_| {},
    );
    assert_eq!(transcript.duration_seconds, 17_000.0 / 16_000.0);
}

#[test]
fn streams_from_buffer_with_resampling() {
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
        vad: [dir.join("silero_vad.onnx")]
            .into_iter()
            .find(|path| path.is_file())
            .map(|path| path.display().to_string()),
    };
    let engine = Arc::new(StreamingEngine::new(&paths, &[]).expect("加载流式模型"));
    let wave = sherpa_onnx::Wave::read("tests/fixtures/zh.wav").expect("读取测试音频");

    // 模拟 48 kHz 设备：每个 16 kHz 样本重复三次。
    let mut upsampled = Vec::with_capacity(wave.samples().len() * 3);
    for sample in wave.samples() {
        upsampled.push(*sample);
        upsampled.push(*sample);
        upsampled.push(*sample);
    }
    let queue = audio::SampleQueue::new(48_000);
    queue.push(&upsampled);
    let stop = Arc::new(AtomicBool::new(false));
    let stopper = stop.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(400));
        stopper.store(true, Ordering::SeqCst);
    });

    let mut partials = 0usize;
    let transcript = run_streaming_core(
        engine,
        None,
        queue,
        48_000,
        stop,
        Arc::new(AtomicBool::new(false)),
        |events| {
            for event in events {
                if let StreamEvent::Partial(_) = event {
                    partials += 1;
                }
            }
        },
    );
    println!("流式结果：{}", transcript.text);
    assert!(!transcript.text.is_empty(), "流式结果为空");
    assert!(partials > 0, "没有中间结果");
    assert!(transcript.duration_seconds > 5.0);
}

/// 识别跟不上、丢掉一段音频时，结果里必须留一个可见的缺口标记。
///
/// 回归测试：之前流式路径完全忽略了 `dropped`，音频被静默丢弃。
#[test]
fn streaming_gap_marker_reaches_transcript() {
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
        vad: [dir.join("silero_vad.onnx")]
            .into_iter()
            .find(|path| path.is_file())
            .map(|path| path.display().to_string()),
    };
    let engine = Arc::new(StreamingEngine::new(&paths, &[]).expect("加载流式模型"));
    let wave = sherpa_onnx::Wave::read("tests/fixtures/zh.wav").expect("读取测试音频");
    let mut upsampled = Vec::with_capacity(wave.samples().len() * 3);
    for sample in wave.samples() {
        upsampled.push(*sample);
        upsampled.push(*sample);
        upsampled.push(*sample);
    }
    // 上限调到 1000 个采样：音频远超上限 → 一定会触发丢弃
    let queue = audio::SampleQueue::with_limit(Some(1_000));
    queue.push(&upsampled);
    let stop = Arc::new(AtomicBool::new(false));
    let stopper = stop.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(400));
        stopper.store(true, Ordering::SeqCst);
    });

    let transcript = run_streaming_core(
        engine,
        None,
        queue,
        48_000,
        stop,
        Arc::new(AtomicBool::new(false)),
        |_| {},
    );
    println!("带缺口的流式结果：{}", transcript.text);
    assert!(
        transcript.text.contains("[[gap:"),
        "丢掉音频后结果里应该有缺口标记: {}",
        transcript.text
    );
    assert!(
        transcript
            .segments
            .iter()
            .any(|s| s.text.contains("[[gap:")),
        "segments 里也该有缺口标记"
    );
}

/// 整段模式：丢掉音频后，缺口标记必须出现在最终文本里。
///
/// 回归测试：之前标记只 push 进了本地 parts，最终文本却从 session.segments()
/// 拼，标记被丢掉了。
#[test]
fn offline_gap_marker_reaches_transcript() {
    let Some((engine, vad)) = offline_engine() else {
        eprintln!("跳过：找不到 SenseVoice 模型");
        return;
    };
    let wave = sherpa_onnx::Wave::read("tests/fixtures/zh.wav").expect("读取测试音频");

    // 上限调到 1000 个采样：音频远超上限 → 一定会触发丢弃
    let queue = audio::SampleQueue::with_limit(Some(1_000));
    queue.push(wave.samples());
    let stop = Arc::new(AtomicBool::new(true));

    let mut events = Vec::new();
    let transcript = run_offline(
        engine,
        vad,
        queue,
        16_000,
        stop,
        Arc::new(AtomicBool::new(false)),
        |event| events.push(event),
    );
    println!("带缺口的整段结果：{}", transcript.text);
    assert!(
        transcript.text.contains("[[gap:"),
        "丢掉音频后文本里应该有缺口标记: {}",
        transcript.text
    );
    assert!(
        events.iter().any(|e| match e {
            OfflineEvent::Segment(s) => s.text.contains("[[gap:"),
        }),
        "也应该实时发一个缺口事件"
    );
    assert!(
        transcript
            .segments
            .iter()
            .any(|s| s.text.contains("[[gap:")),
        "segments 里也该有缺口标记（流式路径就是这样的）"
    );
}

#[test]
fn capture_failure_does_not_discard_finished_transcript() {
    let Ok(recorder) = audio::start_with(None, None, None) else {
        eprintln!("跳过：没有可用麦克风");
        return;
    };
    recorder.inject_error(crate::code("record.micError"));
    let (tx, rx) = mpsc::channel();
    tx.send(Transcript {
        text: "已经识别的一句".into(),
        segments: Vec::new(),
        duration_seconds: 2.0,
        elapsed_ms: 20,
        language: "zh".into(),
        warning: None,
    })
    .unwrap();
    let run = RecordingRun {
        stop: Arc::new(AtomicBool::new(false)),
        discard: Arc::new(AtomicBool::new(false)),
        rx,
    };
    let stop = run.stop.clone();
    let finished = finish_recording(recorder, run, 1).expect("部分结果应该保留");
    assert_eq!(finished.transcript.text, "已经识别的一句");
    assert!(finished.capture_error.is_some());
    assert!(stop.load(Ordering::SeqCst));
}

#[test]
fn capture_failure_without_transcript_returns_error() {
    let Ok(recorder) = audio::start_with(None, None, None) else {
        eprintln!("跳过：没有可用麦克风");
        return;
    };
    recorder.inject_error(crate::code("record.micError"));
    let (tx, rx) = mpsc::channel();
    tx.send(Transcript {
        text: String::new(),
        segments: Vec::new(),
        duration_seconds: 1.0,
        elapsed_ms: 0,
        language: "zh".into(),
        warning: None,
    })
    .unwrap();
    let run = RecordingRun {
        stop: Arc::new(AtomicBool::new(false)),
        discard: Arc::new(AtomicBool::new(false)),
        rx,
    };
    assert!(
        matches!(finish_recording(recorder, run, 1), Err(error) if error == "record.micError")
    );
}

/// 等待收尾失败时必须报错，不能把半截结果当成成功交出去。
///
/// 这条路径不需要麦克风与模型：识别线程一直不回传结果就触发超时。
#[test]
fn wait_failure_returns_no_partial_result() {
    let (tx, rx) = mpsc::channel::<Transcript>();
    let discard = AtomicBool::new(false);
    assert!(matches!(
        wait_transcript(&rx, &discard, 0),
        Err(error) if error == STOP_FAILED
    ));
    assert!(discard.load(Ordering::SeqCst), "超时要置 discard，别再让旧线程发射");
    drop(tx);
}

/// 停止录音时，识别期间麦克风采到的那段音频不能被丢掉。
///
/// 回归测试：run_offline 的循环是「取音频 → 识别 → 看停止标志」，
/// 停止标志在识别期间被置位时循环直接跳出，那段音频就永远留在队列里
/// （流式路径在循环后有收尾取样，整段原来没有）。
#[test]
fn offline_stop_keeps_the_tail() {
    let Some((engine, vad)) = offline_engine() else {
        eprintln!("跳过：找不到 SenseVoice 模型");
        return;
    };
    let wave = sherpa_onnx::Wave::read("tests/fixtures/zh.wav").expect("读取测试音频");

    // 尾部补 1 秒静音：VAD 要靠静音断句，不然句子要等到 flush 才出来，
    // 循环里就不会有事件了。
    let mut speech = wave.samples().to_vec();
    speech.extend(std::iter::repeat(0.0f32).take(16_000));

    let queue = audio::SampleQueue::with_limit(None);
    queue.push(&speech);

    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = stop.clone();
    let q = queue.clone();
    let tail = vec![0.0f32; 1_600]; // 100ms 的尾巴
    let mut emitted = 0usize;

    // 看门狗：万一没识别出句子，10 秒后也让循环退出（不能把测试挂死）。
    let watchdog = stop.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(10));
        watchdog.store(true, Ordering::SeqCst);
    });

    let _transcript = run_offline(
        engine,
        vad,
        queue.clone(),
        16_000,
        stop,
        Arc::new(AtomicBool::new(false)),
        |event| {
            let OfflineEvent::Segment(_) = event;
            emitted += 1;
            if emitted == 1 {
                // 模拟「识别这句的工夫里麦克风又采到一段，然后用户点了停止」
                q.push(&tail);
                stop2.store(true, Ordering::SeqCst);
            }
        },
    );

    assert!(emitted > 0, "没识别出句子，测不到停止那一刻的收尾");
    assert_eq!(
        queue.pending(),
        0,
        "停止录音后队列里还剩 {} 个采样没被识别 —— 这段音频被静默丢掉了",
        queue.pending()
    );
}
