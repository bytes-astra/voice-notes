//! `asr` 模块的单元测试（从 `asr.rs` 拆出）。

use super::*;

#[test]
fn externally_advanced_audio_is_not_counted_again() {
    let Some(engine) = crate::engine::streaming_engine_for_test() else {
        eprintln!("跳过：未设置 VOICE_NOTES_STREAMING_MODEL_DIR");
        return;
    };
    let mut session = StreamingSession::new(engine, None);
    let silence = vec![0.0f32; STREAM_CHUNK];
    session.advance_samples(STREAM_CHUNK);
    session.push(&silence);
    session.advance_samples(STREAM_CHUNK);
    session.finish();
    let transcript = session.transcript();
    assert_eq!(transcript.duration_seconds, 0.4);
}

/// 没有识别文本的收尾不能把上一句终点推进。
#[test]
fn empty_force_end_does_not_advance_last_sentence_end() {
    let Some(engine) = crate::engine::streaming_engine_for_test() else {
        eprintln!("跳过：未设置 VOICE_NOTES_STREAMING_MODEL_DIR");
        return;
    };
    let mut session = StreamingSession::new(engine, None);
    session.advance_samples(STREAM_CHUNK);
    session.push(&vec![0.0; STREAM_CHUNK]);
    assert!(session.partial.is_empty());
    session.force_end();
    assert_eq!(session.timeline.last_end, 0, "没有句子就不能推进上一句终点");
}

/// 缺口发生在预录之后时，句子时间不能退回缺口之前。
#[test]
fn preroll_before_a_gap_cannot_cross_the_gap() {
    let Some(engine) = crate::engine::streaming_engine_for_test() else {
        eprintln!("跳过：未设置 VOICE_NOTES_STREAMING_MODEL_DIR");
        return;
    };
    let wave = sherpa_onnx::Wave::read("tests/fixtures/zh.wav").expect("读取中文 fixture");
    let mut session = StreamingSession::new(engine, None);
    session.advance_samples(STREAM_CHUNK * 2);
    let preroll_end = session.timeline.recorded;
    session.advance_dropped(SAMPLE_RATE as usize);
    let gap_end = session.timeline.recorded;
    session.advance_samples(wave.samples().len());
    session.push_at(&vec![0.0; STREAM_CHUNK * 2], preroll_end);
    session.push(wave.samples());
    session.finish();
    let transcript = session.transcript();
    assert!(!transcript.segments.is_empty());
    assert!(transcript.segments[0].start >= gap_end as f32 / SAMPLE_RATE as f32);
}

/// 门控收尾时应消费已接收的余量，并以真实输入终点而不是门控静音终点定稿。
#[test]
fn force_end_consumes_pending_before_reset_and_uses_input_end() {
    let Some(engine) = crate::engine::streaming_engine_for_test() else {
        eprintln!("跳过：未设置 VOICE_NOTES_STREAMING_MODEL_DIR");
        return;
    };
    let mut session = StreamingSession::new(engine, None);
    let wave = sherpa_onnx::Wave::read("tests/fixtures/zh.wav").expect("读取中文 fixture");
    let mut samples = wave.samples().to_vec();
    let padding = (1_600 + STREAM_CHUNK - samples.len() % STREAM_CHUNK) % STREAM_CHUNK;
    samples.resize(samples.len() + padding, 0.0);
    session.advance_samples(samples.len());
    session.push(&samples);
    assert_eq!(session.input.pending.len(), 1_600);
    assert!(
        !session.partial.is_empty(),
        "fixture 必须在门控收尾前产生文本"
    );
    let audio_end = session.input.end;
    // 这段静音被门控，只推进真实录音时钟，没有喂给识别器。
    session.advance_samples(STREAM_CHUNK);
    let events = session.force_end();
    assert!(
        session.input.pending.is_empty(),
        "余量不能留到下一句的识别器"
    );
    let emitted: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            StreamEvent::Segment(segment) => Some(segment),
            _ => None,
        })
        .collect();
    assert!(!emitted.is_empty(), "必须保留收尾时的识别文本");
    assert_eq!(
        session.timeline.last_end, audio_end,
        "不能用未喂入的门控静音定稿"
    );
    for segment in emitted {
        assert!(segment.start < segment.end);
        assert!(segment.end <= audio_end as f32 / SAMPLE_RATE as f32);
    }
}

/// 停止前的门控静音不能让 finish 把结果压成零时长。
#[test]
fn force_end_then_finish_preserves_real_segment_time() {
    let Some(engine) = crate::engine::streaming_engine_for_test() else {
        eprintln!("跳过：未设置 VOICE_NOTES_STREAMING_MODEL_DIR");
        return;
    };
    let wave = sherpa_onnx::Wave::read("tests/fixtures/zh.wav").expect("读取中文 fixture");
    let mut session = StreamingSession::new(engine, None);
    session.advance_samples(wave.samples().len());
    session.push(wave.samples());
    session.advance_samples(STREAM_CHUNK);
    let mut events = session.force_end();
    events.extend(session.finish());
    let transcript = session.transcript();
    let emitted: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            StreamEvent::Segment(segment) => Some(segment),
            _ => None,
        })
        .collect();
    assert!(!emitted.is_empty());
    assert_eq!(emitted.len(), transcript.segments.len());
    for (event, final_segment) in emitted.iter().zip(&transcript.segments) {
        assert!(event.start < event.end);
        assert!(event.end <= wave.samples().len() as f32 / SAMPLE_RATE as f32);
        assert_eq!(
            (event.start, event.end),
            (final_segment.start, final_segment.end)
        );
    }
}

/// 时钟只认调用方推进的采样：喂进去的音频不再自己计数，
/// 收尾补的合成静音也不算录音时长。
#[test]
fn duration_counts_only_advanced_audio() {
    let Some(engine) = crate::engine::streaming_engine_for_test() else {
        eprintln!("跳过：未设置 VOICE_NOTES_STREAMING_MODEL_DIR");
        return;
    };
    let mut session = StreamingSession::new(engine.clone(), None);
    session.push(&vec![0.0f32; STREAM_CHUNK + 1]);
    session.finish();
    assert_eq!(session.transcript().duration_seconds, 0.0);

    let mut session = StreamingSession::new(engine.clone(), None);
    session.advance_samples(STREAM_CHUNK + 1);
    session.push(&vec![0.0f32; STREAM_CHUNK + 1]);
    session.finish();
    assert_eq!(
        session.transcript().duration_seconds,
        (STREAM_CHUNK + 1) as f32 / SAMPLE_RATE as f32
    );
}
