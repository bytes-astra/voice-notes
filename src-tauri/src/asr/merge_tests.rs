//! `asr` 模块的单元测试（从 `asr.rs` 拆出）。

use super::*;

#[test]
fn merges_overlapping_text() {
    let texts = vec![
        "今天下午三点我们开一个产品评审会讨论下个版本的排期和优先级主要是把核心流程的体验做顺再把一些边角".to_string(),
        "版本的排期和优先级主要是把核心流程的体验做顺再把一些边角的功能砍掉这样团队能专注".to_string(),
    ];
    let merged = merge_transcripts(&texts);
    println!("合并结果：{merged}");
    assert!(
        merged.contains("一些边角的功能砍掉"),
        "边界处被劈开了: {merged}"
    );
    assert_eq!(merged.matches("边角").count(), 1, "边界处重复了: {merged}");
}

#[test]
fn keeps_text_when_no_overlap() {
    let texts = vec!["第一句话。".to_string(), "完全不相干的内容。".to_string()];
    let merged = merge_transcripts(&texts);
    assert_eq!(merged, "第一句话。完全不相干的内容。");
}

#[test]
fn ignores_punctuation_when_comparing() {
    // 前一段以句号结尾，后一段开头没有句号 —— 也要能对上
    let texts = vec![
        "排期和优先级。".to_string(),
        "排期和优先级主要是".to_string(),
    ];
    let merged = merge_transcripts(&texts);
    assert_eq!(merged, "排期和优先级。主要是");
}

#[test]
fn short_common_part_is_not_merged() {
    // 只对上 2 个字（不足 MIN_OVERLAP_CHARS），宁可保留重复也不删
    let texts = vec!["好的".to_string(), "好的继续".to_string()];
    let merged = merge_transcripts(&texts);
    assert_eq!(merged, "好的好的继续");
}
