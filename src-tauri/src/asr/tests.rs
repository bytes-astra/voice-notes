//! `asr` 模块的单元测试（从 `asr.rs` 拆出）。

use super::*;
use sherpa_onnx::SileroVadModelConfig;

/// VAD 建不起来要报错，不能 panic。
///
/// panic 会让识别线程直接死掉：用户录完停下来只看到「停止超时」，
/// 真因（模型损坏、版本不匹配）被完全掩盖。
#[test]
fn vad_failure_is_reported_instead_of_panicking() {
    let vad = VadModelConfig {
        silero_vad: SileroVadModelConfig {
            model: Some("/nonexistent/silero_vad.onnx".into()),
            ..Default::default()
        },
        ..Default::default()
    };
    let error = match create_vad_detector(&vad) {
        Ok(_) => panic!("VAD 建不起来必须报错"),
        Err(error) => error,
    };
    assert!(error.starts_with("model.loadFailed"), "错误码不对：{error}");
}

/// 转写结果由句子拼出来：文本按拼句规则拼，句子原样带上。
///
/// 这段拼装有三条路径（录音、文件、整条一次），放一处才不会各拼各的。
#[test]
fn transcript_assembly_joins_the_segments() {
    let segments = vec![
        Segment {
            text: "你好".into(),
            start: 0.0,
            end: 1.0,
        },
        Segment {
            text: "世界".into(),
            start: 1.0,
            end: 2.0,
        },
    ];
    let transcript = transcript_from(&segments, 2.0, 100, "zh");
    assert_eq!(transcript.text, "你好世界");
    assert_eq!(transcript.segments.len(), 2);
    assert_eq!(transcript.duration_seconds, 2.0);
    assert_eq!(transcript.elapsed_ms, 100);
    assert_eq!(transcript.language, "zh");
    assert!(transcript.warning.is_none());
}

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
