//! 归一器的单测：三种协议形态 → 同一份「当前句累计串」。

use super::*;

/// 增量拼接：逐段追加。
#[test]
fn delta_appends_to_current_sentence() {
    let mut acc = PartialAccumulator::new(PartialSemantics::Delta);
    let first = acc.apply(Some("a".into()), PartialUpdate::Delta("你好".into()));
    assert_eq!(first, "你好");
    let second = acc.apply(Some("a".into()), PartialUpdate::Delta("，世界".into()));
    assert_eq!(second, "你好，世界");
}

/// 确认前缀 + 草稿：整体替换。
#[test]
fn preview_replaces_whole_sentence() {
    let mut acc = PartialAccumulator::new(PartialSemantics::Preview);
    let first = acc.apply(
        Some("a".into()),
        PartialUpdate::Preview { confirmed: "你好".into(), draft: "，".into() },
    );
    assert_eq!(first, "你好，");
    let second = acc.apply(
        Some("a".into()),
        PartialUpdate::Preview { confirmed: "你好，".into(), draft: "世界".into() },
    );
    assert_eq!(second, "你好，世界");
}

/// 区间替换：只改指定区间。
#[test]
fn range_update_rewrites_only_given_range() {
    let mut acc = PartialAccumulator::new(PartialSemantics::Range);
    acc.apply(Some("a".into()), PartialUpdate::Delta("你".into()));
    acc.apply(Some("a".into()), PartialUpdate::Delta("好".into()));
    let text = acc.apply(
        Some("a".into()),
        PartialUpdate::Replace { from: 0, to: 0, text: "您".into() },
    );
    assert_eq!(text, "您好");
}

/// 句定稿后清掉自己的桶；别的桶不受影响。
#[test]
fn finish_clears_only_its_own_bucket() {
    let mut acc = PartialAccumulator::new(PartialSemantics::Delta);
    acc.apply(Some("a".into()), PartialUpdate::Delta("第一句".into()));
    acc.apply(Some("b".into()), PartialUpdate::Delta("第二句".into()));
    acc.finish_sentence(Some("a".into()));
    // a 的桶没了，b 的还在。
    assert_eq!(acc.current().as_deref(), Some("第二句"));
}

/// 没有句标识 → 单桶：一条定稿清空整桶。
#[test]
fn no_id_uses_single_bucket() {
    let mut acc = PartialAccumulator::new(PartialSemantics::Delta);
    acc.apply(None, PartialUpdate::Delta("一句".into()));
    assert_eq!(acc.finish_sentence(None).as_deref(), Some("一句"));
    assert!(acc.current().is_none(), "单桶清空后没有活跃句");
}

/// 当前活跃句 = 最近被更新的那一句（用于旧句定稿后补发实时文本）。
#[test]
fn current_is_the_last_touched_sentence() {
    let mut acc = PartialAccumulator::new(PartialSemantics::Delta);
    acc.apply(Some("a".into()), PartialUpdate::Delta("甲".into()));
    acc.apply(Some("b".into()), PartialUpdate::Delta("乙".into()));
    assert_eq!(acc.current().as_deref(), Some("乙"));
}
