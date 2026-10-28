//! `asr` 模块的单元测试（从 `asr.rs` 拆出）。

use super::*;
use crate::audio::gap_marker_seconds;

fn seg(text: &str, start: f32) -> Segment {
    Segment {
        text: text.into(),
        start,
        end: start + 1.0,
    }
}

#[test]
fn inserts_gap_in_time_order() {
    let mut transcript = Transcript {
        text: "第一句。第二句。".into(),
        segments: vec![seg("第一句。", 0.0), seg("第二句。", 2.0)],
        duration_seconds: 3.0,
        elapsed_ms: 100,
        language: "zh".into(),
        warning: None,
    };
    insert_gaps(&mut transcript, vec![seg("[[gap:3]]", 1.0)]);
    assert_eq!(transcript.segments.len(), 3);
    assert_eq!(transcript.segments[1].text, "[[gap:3]]");
    // 占位符前后不加空格（和前端 core.js 的 joinText 规则一致）
    assert_eq!(transcript.text, "第一句。[[gap:3]]第二句。");
}

/// 缺口占位符前后都不加空格（前端 `core.js` 的 `joinText()` 同一套规则）。
#[test]
fn gap_placeholder_joins_without_spaces() {
    assert_eq!(
        join_segments(["第一句。", "[[gap:3]]", "第二句。"].into_iter()),
        "第一句。[[gap:3]]第二句。"
    );
    // 占位符在中间：两侧都不加空格
    assert_eq!(
        join_segments(["hello", "[[gap:12]]", "world"].into_iter()),
        "hello[[gap:12]]world"
    );
    // 占位符开头 / 结尾
    assert_eq!(
        join_segments(["[[gap:2]]", "后面一句"].into_iter()),
        "[[gap:2]]后面一句"
    );
    assert_eq!(
        join_segments(["前面一句", "[[gap:2]]"].into_iter()),
        "前面一句[[gap:2]]"
    );
    // 不是占位符的普通文本仍然按 CJK 规则拼
    assert_eq!(join_segments(["你好", "world"].into_iter()), "你好 world");
}

/// 占位符只在结尾时才算「前一段是占位符」：中间夹了别的文字就按普通规则拼。
#[test]
fn only_ends_with_placeholder_skips_the_space() {
    assert_eq!(
        join_segments(["[[gap:3]]", "第二句。"].into_iter()),
        "[[gap:3]]第二句。"
    );
    // 结尾不是占位符（后面还有别的字）→ 走普通规则
    assert_eq!(
        join_segments(["[[gap:3]]x", "第二句。"].into_iter()),
        "[[gap:3]]x 第二句。"
    );
}

/// 从占位符里取秒数（前端拿它填 `record.gapWarning` 的 {seconds}）。
#[test]
fn parses_gap_seconds() {
    assert_eq!(gap_marker_seconds("[[gap:3]]"), Some(3));
    assert_eq!(gap_marker_seconds("[[gap:12]]"), Some(12));
    assert_eq!(gap_marker_seconds("普通句子"), None);
    assert_eq!(gap_marker_seconds("[[gap:]]"), None);
    assert_eq!(gap_marker_seconds("[[gap:abc]]"), None);
}

#[test]
fn no_gaps_leaves_text_untouched() {
    let mut transcript = Transcript {
        text: "只有一句。".into(),
        segments: vec![seg("只有一句。", 0.0)],
        duration_seconds: 1.0,
        elapsed_ms: 50,
        language: "zh".into(),
        warning: None,
    };
    insert_gaps(&mut transcript, Vec::new());
    assert_eq!(transcript.text, "只有一句。");
    assert_eq!(transcript.segments.len(), 1);
}
