//! 流式识别冒烟测试：把测试音频按 100ms 切片喂进去，模拟实时麦克风。
//!
//!   VOICE_NOTES_STREAMING_MODEL_DIR=/path/to/streaming cargo test --test streaming_smoke -- --nocapture

use std::path::PathBuf;
use std::sync::Arc;

use voice_notes_lib::asr::{StreamEvent, StreamingSession};
use voice_notes_lib::engine::StreamingEngine;
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

    let engine = Arc::new(StreamingEngine::new(&paths, &[]).expect("加载流式模型"));
    let wave = sherpa_onnx::Wave::read("tests/fixtures/zh.wav").expect("读取测试音频");
    let samples = wave.samples();

    let mut session = StreamingSession::new(engine, None);
    let mut partials = 0;
    let mut segments = 0;

    // 100ms 一片，模拟 16 kHz 实时输入。
    for chunk in samples.chunks(1_600) {
        session.advance_samples(chunk.len());
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

/// 收尾必须把最后一个字冲出来：解码器需要一点尾部上下文，
/// 否则句尾会被丢掉（“产品评审” → “产品评审会”少一个字）。
#[test]
fn flushes_trailing_words_on_finish() {
    let Some(paths) = model_paths() else {
        eprintln!("跳过：找不到流式模型（VOICE_NOTES_STREAMING_MODEL_DIR）");
        return;
    };

    for (fixture, tail) in [
        ("tests/fixtures/zh-tail.wav", "评审会"),
        ("tests/fixtures/zh-tail2.wav", "确认一下"),
    ] {
        let engine = Arc::new(StreamingEngine::new(&paths, &[]).expect("加载流式模型"));
        let wave = sherpa_onnx::Wave::read(fixture).expect("读取测试音频");

        let mut session = StreamingSession::new(engine, None);
        for chunk in wave.samples().chunks(3_200) {
            session.advance_samples(chunk.len());
            session.push(chunk);
        }
        session.finish();

        let transcript = session.transcript();
        println!("{} → {}", fixture, transcript.text);
        assert!(
            transcript.text.contains(tail),
            "{fixture} 的句尾应完整包含“{tail}”，实际：{}",
            transcript.text
        );
    }
}

/// 门控收尾也要冲出句尾，不能把最后的字留在被重置的识别器里。
#[test]
fn force_end_flushes_trailing_words() {
    let Some(paths) = model_paths() else {
        eprintln!("跳过：找不到流式模型（VOICE_NOTES_STREAMING_MODEL_DIR）");
        return;
    };
    for (fixture, tail) in [
        ("tests/fixtures/zh-tail.wav", "评审会"),
        ("tests/fixtures/zh-tail2.wav", "确认一下"),
    ] {
        let engine = Arc::new(StreamingEngine::new(&paths, &[]).expect("加载流式模型"));
        let wave = sherpa_onnx::Wave::read(fixture).expect("读取测试音频");
        let mut session = StreamingSession::new(engine, None);
        for chunk in wave.samples().chunks(1_600) {
            session.advance_samples(chunk.len());
            session.push(chunk);
        }
        session.force_end();
        let transcript = session.transcript();
        assert!(
            transcript.text.contains(tail),
            "{fixture} → {}",
            transcript.text
        );
    }
}

/// 一次门控收尾后，重置的识别器仍要能识别下一句。
#[test]
fn streaming_continues_after_force_end() {
    let Some(paths) = model_paths() else {
        eprintln!("跳过：找不到流式模型（VOICE_NOTES_STREAMING_MODEL_DIR）");
        return;
    };
    let engine = Arc::new(StreamingEngine::new(&paths, &[]).expect("加载流式模型"));
    let wave = sherpa_onnx::Wave::read("tests/fixtures/zh.wav").expect("读取测试音频");
    let mut session = StreamingSession::new(engine, None);
    let mut events = Vec::new();
    // 包含一次不足 200ms 的零长语音收尾，再重复两次真实语音。
    session.advance_samples(1_600);
    events.extend(session.push(&vec![0.0; 1_600]));
    events.extend(session.force_end());
    for _ in 0..2 {
        for chunk in wave.samples().chunks(1_600) {
            session.advance_samples(chunk.len());
            events.extend(session.push(chunk));
        }
        session.advance_samples(1_600);
        events.extend(session.force_end());
    }
    let transcript = session.transcript();
    let segments: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            StreamEvent::Segment(segment) => Some(segment),
            _ => None,
        })
        .collect();
    assert_eq!(segments.len(), 2, "两轮都应识别出一句");
    assert!(segments.iter().all(|segment| !segment.text.is_empty()));
    assert!(segments[0].end <= segments[1].start);
    assert!(segments.iter().all(|segment| segment.start < segment.end));
    assert_eq!(segments.len(), transcript.segments.len());
    for (event, final_segment) in segments.iter().zip(&transcript.segments) {
        assert_eq!(
            (event.start, event.end),
            (final_segment.start, final_segment.end)
        );
    }
}

/// 门控收尾后的流应与新会话独立，greedy 和热词两种配置都要验证。
#[test]
fn force_end_starts_a_fresh_stream_with_or_without_hotwords() {
    let Some(paths) = model_paths() else {
        eprintln!("跳过：找不到流式模型（VOICE_NOTES_STREAMING_MODEL_DIR）");
        return;
    };
    let wave = sherpa_onnx::Wave::read("tests/fixtures/zh.wav").expect("读取测试音频");
    let words = vec!["产品评审会".to_string()];
    for hotwords in [&[][..], words.as_slice()] {
        let engine = Arc::new(StreamingEngine::new(&paths, hotwords).expect("加载流式模型"));
        let mut session = StreamingSession::new(engine.clone(), None);
        let mut previous_count = 0;
        for chunk in wave.samples().chunks(1_600) {
            session.advance_samples(chunk.len());
            previous_count += session
                .push(chunk)
                .iter()
                .filter(|event| matches!(event, StreamEvent::Segment(_)))
                .count();
        }
        previous_count += session
            .force_end()
            .iter()
            .filter(|event| matches!(event, StreamEvent::Segment(_)))
            .count();
        for chunk in wave.samples().chunks(1_600) {
            session.advance_samples(chunk.len());
            session.push(chunk);
        }
        session.force_end();
        let actual = session.transcript();
        let mut fresh = StreamingSession::new(engine, None);
        for chunk in wave.samples().chunks(1_600) {
            fresh.advance_samples(chunk.len());
            fresh.push(chunk);
        }
        fresh.force_end();
        let expected = fresh.transcript();
        assert!(
            previous_count > 0 && !expected.text.is_empty(),
            "两轮均应识别出文本"
        );
        assert!(
            actual.segments.len() > previous_count,
            "收尾后应再次识别出文本"
        );
        let actual_text = actual.segments[previous_count..]
            .iter()
            .map(|s| s.text.as_str())
            .collect::<String>();
        assert_eq!(
            actual_text, expected.text,
            "门控收尾后的识别结果必须与新流一致，hotwords={hotwords:?}"
        );
    }
}

/// 词表外的字（番、茄、腩、苹、莓、酪）不能变成「�」。
///
/// 这个流式模型的词表只有 1426 个汉字，表外的字只能拆成字节 token
/// `<0xE7><0x95><0xAA>` 拼。sherpa 把这些 token 还原成裸字节放进 `text`，
/// 切分点落在半个字上时就是非法 UTF-8，sherpa 自己会固化成 U+FFFD。
/// 修法是从**无损的 token 列表**重建文本（asr::text_from_tokens）。
///
/// `tests/fixtures/zh-oov.wav` 是 macOS TTS 生成的「番茄牛腩。苹果派要苹果。草莓奶酪。」，
/// 正好覆盖词表外的字。
#[test]
fn keeps_out_of_vocab_chars_intact() {
    let Some(paths) = model_paths() else {
        eprintln!("跳过：找不到流式模型（VOICE_NOTES_STREAMING_MODEL_DIR）");
        return;
    };

    let engine = Arc::new(StreamingEngine::new(&paths, &[]).expect("加载流式模型"));
    let wave = sherpa_onnx::Wave::read("tests/fixtures/zh-oov.wav").expect("读取测试音频");

    let mut session = StreamingSession::new(engine, None);
    let mut all = String::new();
    for chunk in wave.samples().chunks(1_600) {
        session.advance_samples(chunk.len());
        for event in session.push(chunk) {
            match event {
                StreamEvent::Partial(text) => {
                    assert!(!text.contains('\u{FFFD}'), "中间结果里出现了乱码：{text}");
                    all.push_str(&text);
                }
                StreamEvent::Segment(segment) => {
                    assert!(
                        !segment.text.contains('\u{FFFD}'),
                        "定稿句里出现了乱码：{}",
                        segment.text
                    );
                    all.push_str(&segment.text);
                }
            }
        }
    }
    for event in session.finish() {
        if let StreamEvent::Segment(segment) = event {
            assert!(
                !segment.text.contains('\u{FFFD}'),
                "收尾句里出现了乱码：{}",
                segment.text
            );
            all.push_str(&segment.text);
        }
    }

    let transcript = session.transcript();
    println!("结果：{}", transcript.text);
    assert!(
        !transcript.text.contains('\u{FFFD}'),
        "转写结果里出现了乱码：{}",
        transcript.text
    );
    // 词表外的字要真的在结果里 —— 不能为了不乱码而把字丢掉
    for ch in ['番', '茄', '腩'] {
        assert!(
            transcript.text.contains(ch),
            "结果里应该有“{ch}”：{}",
            transcript.text
        );
    }
}

#[test]
fn picks_requested_input_device() {
    use voice_notes_lib::audio::pick_device_name;

    let available = vec![
        "MacBook Pro Microphone".to_string(),
        "USB 麦克风".to_string(),
    ];

    assert_eq!(
        pick_device_name(None, &available),
        None,
        "没指定就用系统默认"
    );
    assert_eq!(
        pick_device_name(Some(""), &available),
        None,
        "空名字等于没指定"
    );
    assert_eq!(
        pick_device_name(Some("USB 麦克风"), &available),
        Some("USB 麦克风".to_string()),
        "精确匹配应被选中"
    );
    assert_eq!(
        pick_device_name(Some("不存在的设备"), &available),
        None,
        "找不到就退回系统默认"
    );
}

/// 预热麦克风：提前触发系统权限弹窗，不真正录音。
#[test]
fn probes_input_device() {
    let result = voice_notes_lib::audio::probe_input(None);
    println!("probe_input → {result:?}");
    assert!(result.is_ok(), "打开输入设备失败：{result:?}");
}
