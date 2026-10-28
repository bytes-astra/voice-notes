//! `asr` 模块的单元测试（从 `asr.rs` 拆出）。

use super::*;

fn samples(seconds: f32) -> Vec<f32> {
    vec![0.0; (seconds * SAMPLE_RATE as f32) as usize]
}

#[test]
fn long_segment_is_ready_immediately() {
    let mut merger = SegmentMerger::new();
    let ready = merger.push(0.0, samples(4.0));
    assert_eq!(ready.len(), 1);
    assert_eq!(ready[0].samples.len(), (4.0 * SAMPLE_RATE as f32) as usize);
}

#[test]
fn short_segment_waits_for_the_next_one() {
    let mut merger = SegmentMerger::new();
    // 1 秒的短句：先留着，不定稿
    assert!(merger.push(0.0, samples(1.0)).is_empty());
    // 下一句来了 → 合成一句
    let ready = merger.push(1.2, samples(2.0));
    assert_eq!(ready.len(), 1);
    assert_eq!(ready[0].start, 0.0);
    assert_eq!(ready[0].end, 3.2);
    assert_eq!(ready[0].samples.len(), (3.0 * SAMPLE_RATE as f32) as usize);
}

#[test]
fn long_pause_keeps_segments_apart() {
    let mut merger = SegmentMerger::new();
    assert!(merger.push(0.0, samples(1.0)).is_empty());
    // 隔了 10 秒才说下一句 → 不该合并成一句
    let ready = merger.push(11.0, samples(4.0));
    assert_eq!(ready.len(), 2);
    assert_eq!(ready[0].start, 0.0);
    assert_eq!(ready[0].end, 1.0);
    assert_eq!(ready[1].start, 11.0);
    assert_eq!(ready[1].end, 15.0);
}

#[test]
fn short_segment_after_long_pause_is_held_again() {
    let mut merger = SegmentMerger::new();
    assert!(merger.push(0.0, samples(1.0)).is_empty());
    // 上一句隔太久 → 自己定稿；这一句自己也短 → 继续留着
    let ready = merger.push(11.0, samples(2.0));
    assert_eq!(ready.len(), 1);
    assert_eq!(ready[0].start, 0.0);
    assert_eq!(ready[0].end, 1.0);
    assert_eq!(merger.flush().len(), 1);
}

#[test]
fn flush_returns_the_pending_short_segment() {
    let mut merger = SegmentMerger::new();
    assert!(merger.push(0.0, samples(1.0)).is_empty());
    let ready = merger.flush();
    assert_eq!(ready.len(), 1);
    assert_eq!(ready[0].start, 0.0);
    assert_eq!(ready[0].end, 1.0);
    assert!(merger.flush().is_empty());
}

#[test]
fn pending_end_reports_when_the_held_segment_finished() {
    let mut merger = SegmentMerger::new();
    assert_eq!(merger.pending_end(), None);
    assert!(merger.push(0.0, samples(1.0)).is_empty());
    assert_eq!(merger.pending_end(), Some(1.0));
    // 长句当场定稿，没有待定的短句
    let mut merger = SegmentMerger::new();
    assert_eq!(merger.push(0.0, samples(4.0)).len(), 1);
    assert_eq!(merger.pending_end(), None);
}
