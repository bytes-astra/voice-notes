//! 无模型回归：一次采集含多个 endpoint 时，每句按当前分片而不是批末计时。

use super::*;

#[test]
fn multiple_endpoints_in_one_batch_have_nonzero_time_ranges() {
    let mut input = StreamInput::default();
    let mut timeline = Timeline::default();
    // 600 ms 一次送入；用每个 200 ms 分片末尾定稿，确定性覆盖同批多个 endpoint。
    timeline.advance(9_600);
    input.push(&vec![0.1; 9_600], timeline.recorded);
    let mut spans = Vec::new();
    while let Some((_chunk, end)) = input.next_chunk(&mut timeline) {
        timeline.note_audio(end.saturating_sub(3_200));
        spans.push(timeline.commit_at(end));
    }

    assert_eq!(spans, vec![(0, 3_200), (3_200, 6_400), (6_400, 9_600)]);
    assert!(spans.iter().all(|(start, end)| start < end));
    assert_eq!(timeline.recorded, 9_600, "分片计时不能重复推进总时长");
}

#[test]
fn chunk_times_include_the_previous_batch_remainder() {
    let mut input = StreamInput::default();
    let mut timeline = Timeline::default();
    input.push(&vec![0.1; 3_201], 3_201);
    let (first, end) = input.next_chunk(&mut timeline).expect("首个完整分片");
    assert_eq!((first.len(), end), (3_200, 3_200));
    assert!(input.next_chunk(&mut timeline).is_none());

    input.push(&vec![0.2; 6_399], 9_600);
    let (second, end) = input.next_chunk(&mut timeline).expect("跨批分片");
    assert_eq!((second.len(), end), (3_200, 6_400));
    assert_eq!(second[0], 0.1, "前批余量不能被丢弃");
    assert!(second[1..].iter().all(|sample| *sample == 0.2));
    assert_eq!(input.next_chunk(&mut timeline).unwrap().1, 9_600);
    assert!(input.next_chunk(&mut timeline).is_none());
}

#[test]
fn chunk_after_a_gap_uses_its_real_recording_position() {
    let mut input = StreamInput::default();
    let mut timeline = Timeline::default();
    timeline.advance(1_600);
    input.push(&vec![0.1; 1_600], timeline.recorded);
    timeline.note_audio(0);
    assert!(input.next_chunk(&mut timeline).is_none());

    timeline.advance_dropped(32_000);
    timeline.advance(4_800);
    input.push(&vec![0.2; 4_800], timeline.recorded);
    let mut spans = Vec::new();
    while let Some((_chunk, end)) = input.next_chunk(&mut timeline) {
        timeline.note_audio(end.saturating_sub(3_200));
        spans.push(timeline.commit_at(end));
    }
    assert_eq!(spans, vec![(0, 35_200), (35_200, 38_400)]);
    assert_eq!(timeline.recorded, 38_400);
}

#[test]
fn preroll_uses_its_own_end_instead_of_the_current_batch_end() {
    let mut input = StreamInput::default();
    let mut timeline = Timeline::default();
    // 0.4s 预录位于 9.6..10s，当前批位于 10..10.4s，总时钟已到 10.4s。
    timeline.advance(166_400);
    input.push(&vec![0.1; 6_400], 160_000);
    let mut ends = Vec::new();
    while let Some((_chunk, end)) = input.next_chunk(&mut timeline) {
        ends.push(end);
    }
    input.push(&vec![0.2; 6_400], timeline.recorded);
    while let Some((_chunk, end)) = input.next_chunk(&mut timeline) {
        ends.push(end);
    }
    assert_eq!(ends, vec![156_800, 160_000, 163_200, 166_400]);
    assert_eq!(timeline.recorded, 166_400, "重放预录不能再次推进总时长");
}

/// 静音期时钟一直在走，随后重放预录：重放分片整段早于当前句起点，
/// 定稿时不能压成零时长。
#[test]
fn replayed_preroll_keeps_nonzero_spans_after_the_clock_moved() {
    let mut input = StreamInput::default();
    let mut timeline = Timeline::default();
    timeline.advance(166_400);
    timeline.advance_dropped(0); // 静音期：下一句起点跟到批末
    input.push(&vec![0.1; 6_400], 160_000);
    let mut spans = Vec::new();
    while let Some((chunk, end)) = input.next_chunk(&mut timeline) {
        timeline.note_audio(end.saturating_sub(chunk.len()));
        spans.push(timeline.commit_at(end));
    }
    assert_eq!(spans, vec![(153_600, 156_800), (156_800, 160_000)]);
    assert!(spans.iter().all(|(start, end)| start < end));
}

/// 重放的句子横跨两个分片：定稿发生在第二片，起点仍要落在第一片。
///
/// 回归测试：之前拿「触发定稿的那一片」的起点当句子起点 —— 句子前半段的
/// 音频落在区间外面，界面上这句话的时长会短一截。
#[test]
fn a_replayed_sentence_over_two_chunks_keeps_the_first_chunk_start() {
    let mut input = StreamInput::default();
    let mut timeline = Timeline::default();
    timeline.advance(166_400);
    timeline.advance_dropped(0); // 静音期：下一句起点跟到批末
    input.push(&vec![0.1; 6_400], 160_000);
    // 第一片喂进去，但不在这定稿。
    let (chunk, end) = input.next_chunk(&mut timeline).expect("首个重放分片");
    timeline.note_audio(end.saturating_sub(chunk.len()));
    // 第二片：这句在这里定稿。
    let (chunk, end) = input.next_chunk(&mut timeline).expect("第二个重放分片");
    timeline.note_audio(end.saturating_sub(chunk.len()));
    let span = timeline.commit_at(end);
    assert_eq!(span, (153_600, 160_000), "起点要落在第一片上");
}

#[test]
fn final_remainder_keeps_the_last_real_input_end() {
    let mut input = StreamInput::default();
    let mut timeline = Timeline::default();
    timeline.advance(8_000);
    input.push(&vec![0.1; 8_000], timeline.recorded);
    let mut spans = Vec::new();
    while let Some((_chunk, end)) = input.next_chunk(&mut timeline) {
        timeline.note_audio(end.saturating_sub(3_200));
        spans.push(timeline.commit_at(end));
    }
    // 后来的门控静音已计入录音，但没有喂给识别器；不能拿它给旧余量定时。
    timeline.advance(1_600);
    let (tail, end) = input.finish().expect("不足 200ms 的真实尾包");
    assert_eq!(tail.len(), 1_600);
    timeline.note_audio(end.saturating_sub(tail.len()));
    spans.push(timeline.commit_at(end));
    assert_eq!(spans, vec![(0, 3_200), (3_200, 6_400), (6_400, 8_000)]);
    assert_eq!(input.end, 8_000, "补静音仍使用最后一个真实输入位置");
    assert_eq!(timeline.recorded, 9_600);
    assert!(input.finish().is_none());
}

/// 最后一个完整分片发生 endpoint 后，余量仍属于下一句，不能被下一轮挪走。
#[test]
fn endpoint_remainder_keeps_its_start_across_the_next_capture_batch() {
    for (dropped, expected) in [(0, (3_200, 6_400)), (32_000, (3_200, 38_400))] {
        let mut input = StreamInput::default();
        let mut timeline = Timeline::default();
        timeline.advance(4_800);
        input.push(&vec![0.1; 4_800], timeline.recorded);
        timeline.note_audio(0);
        let (_chunk, end) = input.next_chunk(&mut timeline).unwrap();
        timeline.note_audio(end.saturating_sub(3_200));
        assert_eq!(timeline.commit_at(end), (0, 3_200));
        assert!(input.next_chunk(&mut timeline).is_none());

        // 与生产循环同序：每轮先报告丢弃量，即便本轮丢弃量为零。
        timeline.advance_dropped(dropped);
        timeline.advance(1_600);
        input.push(&vec![0.2; 1_600], timeline.recorded);
        let (_chunk, end) = input.next_chunk(&mut timeline).unwrap();
        timeline.note_audio(end.saturating_sub(3_200));
        let span = timeline.commit_at(end);
        assert_eq!(span, expected, "dropped={dropped} 时不能跳过前批 pending");
        assert!(span.0 < span.1);
    }
}

#[test]
fn empty_push_does_not_retimestamp_pending_audio() {
    let mut input = StreamInput::default();
    input.push(&vec![0.1; 1_600], 1_600);
    input.push(&[], 16_000);
    let (tail, end) = input.finish().unwrap();
    assert_eq!((tail.len(), end), (1_600, 1_600));
}
