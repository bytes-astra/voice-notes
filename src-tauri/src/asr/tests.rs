//! `asr` 模块的单元测试（从 `asr.rs` 拆出）。

use super::*;

#[test]
fn joins_cjk_without_spaces() {
    assert_eq!(join_segments(["你好", "世界"].into_iter()), "你好世界");
    assert_eq!(join_segments(["hello", "world"].into_iter()), "hello world");
    assert_eq!(join_segments(["你好", "hello"].into_iter()), "你好 hello");
}

#[test]
fn strips_sensevoice_tags() {
    assert_eq!(clean("<|zh|><|NEUTRAL|><|Speech|>你好 世界"), "你好 世界");
    assert_eq!(clean("  <|en|>hello   there "), "hello there");
}

/// 「有声音却长时间没切出句子」要告警；停顿几秒、纯静音、以及「VAD 正在攒
/// 这一句」都不该报。
///
/// 这是给 sherpa 那个「非语音直接丢弃」的漏洞兜底：没有它，这段音频
/// 在日志、转写、界面上都是零痕迹。
#[test]
fn warns_only_when_speech_went_missing() {
    // 有能量、VAD 也没在攒 → 音频被当非语音丢了，要报
    assert!(no_segment_warning(30.0, 0.3, false).is_some());
    // 停顿几秒是正常说话节奏
    assert!(no_segment_warning(3.0, 0.3, false).is_none());
    // 纯静音：不是丢样本，是本来就没声音
    assert!(no_segment_warning(30.0, 0.001, false).is_none());
    // VAD 正在攒这一句：音频没丢，只是还没到句尾
    assert!(no_segment_warning(30.0, 0.3, true).is_none());
}

/// 前端读 transcript.durationSeconds / elapsedMs：序列化必须是 camelCase，
/// 否则结果区的时长/耗时/RTF 和历史记录里的时长全是 0。
#[test]
fn transcript_serializes_camel_case() {
    let transcript = Transcript {
        text: "你好".into(),
        segments: vec![Segment {
            text: "你好".into(),
            start: 0.0,
            end: 1.0,
        }],
        duration_seconds: 1.5,
        elapsed_ms: 200,
        language: "zh".into(),
        warning: None,
    };
    let value = serde_json::to_value(&transcript).unwrap();
    assert!(value.get("warning").is_none());
    assert_eq!(value["durationSeconds"], serde_json::json!(1.5));
    assert_eq!(value["elapsedMs"], serde_json::json!(200));
    assert!(value.get("duration_seconds").is_none());
    assert!(value.get("elapsed_ms").is_none());
}
