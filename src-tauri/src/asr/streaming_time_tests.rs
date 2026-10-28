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
