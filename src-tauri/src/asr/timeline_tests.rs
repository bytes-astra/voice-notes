//! `asr` 模块的单元测试（从 `asr.rs` 拆出）。
//!
//! 这里测的是**流式会话的时间轴**：真实录音时钟 + 当前这句的起点。
//!
//! 不用真模型 —— 真模型的用例没设环境变量时会跳过（CI 上就是这样），
//! 这条规则不能只靠它们守。

use super::*;

/// 一秒的采样数（测试里少写几遍）。
const SEC: usize = SAMPLE_RATE as usize;

/// 洞落在这句**之前**：这句的起点要挪到洞之后。
///
/// 回归测试：之前流式的句子起点没有按丢掉的时间后移，缺口标记落到句子
/// **后面** —— 文本变成「下午五点[[gap:4]]」，读起来像句子之后才丢的音频
/// （整段模式是「[[gap:4]]下午5点。」）。
///
/// 口径与整段模式一致：洞在识别器吃到这句的音频**之前** → 起点后移。
#[test]
fn a_hole_before_the_sentence_moves_its_start_past_the_hole() {
    let mut timeline = Timeline::default();
    // 第一句：喂 1 秒就定稿
    timeline.advance(SEC);
    timeline.note_audio(0);
    // 这句 [0, 1s)，下一句从 1s 开始。
    timeline.commit_at(timeline.recorded, true);
    // 接着丢了 4 秒 —— 这句还没吃到音频 → 洞在它**前面**
    timeline.advance_dropped(4 * SEC);
    timeline.note_audio(5 * SEC);
    let (start, end) = timeline.commit_at(timeline.recorded, true);

    assert_eq!(start, 5 * SEC, "这句的起点应落在洞之后（= 洞的终点）");
    assert_eq!(end, 5 * SEC);
}

/// 洞落在这句**里面**：起点不动。
///
/// 这句已经吃到音频了 —— 洞在它中间，句子的真实跨度自然把空档包住
/// （终点由时钟自动带上）。
#[test]
fn a_hole_inside_the_sentence_keeps_its_start() {
    let mut timeline = Timeline::default();
    timeline.advance(SEC);
    timeline.note_audio(0);
    // 这句 [0, 1s)，下一句从 1s 开始。
    timeline.commit_at(timeline.recorded, true);
    // 第二句先吃到 1 秒音频，然后中间丢了 4 秒
    timeline.advance(SEC);
    timeline.note_audio(SEC);
    timeline.advance_dropped(4 * SEC);
    let (start, end) = timeline.commit_at(timeline.recorded, true);

    assert_eq!(start, SEC, "洞在句子里面时起点不动");
    assert_eq!(end, 6 * SEC, "终点要把丢掉的音频算进去");
}

/// 缺口前缓存的预录不能让句子起点退回缺口之前。
#[test]
fn replayed_audio_cannot_move_start_before_a_preceding_gap() {
    let mut timeline = Timeline::default();
    timeline.advance(6_400);
    timeline.advance_dropped(16_000);
    timeline.advance(3_200);
    timeline.note_audio(0);
    timeline.note_audio(22_400);
    assert_eq!(timeline.commit_at(25_600, true), (22_400, 25_600));
}

/// 一个洞都不丢：下一句从上一句定稿的时刻开始。
#[test]
fn without_holes_the_start_is_the_last_commit() {
    let mut timeline = Timeline::default();
    timeline.advance(SEC);
    timeline.note_audio(0);
    timeline.commit_at(timeline.recorded, true);
    timeline.advance(SEC);
    timeline.note_audio(SEC);
    let (start, end) = timeline.commit_at(timeline.recorded, true);

    assert_eq!(start, SEC, "这句从上一句定稿的时刻开始");
    assert_eq!(end, 2 * SEC);
}

/// 定稿后「这句吃到音频了吗」要复位 —— 下一句是新的。
#[test]
fn commit_resets_the_has_audio_flag() {
    let mut timeline = Timeline::default();
    timeline.note_audio(0);
    assert!(timeline.segment_has_audio);
    timeline.commit_at(timeline.recorded, true);
    assert!(!timeline.segment_has_audio, "定稿后下一句还没吃到音频");
}

/// 连续两个洞：起点落在**最后一个**洞之后。
#[test]
fn consecutive_holes_leave_the_start_after_the_last_one() {
    let mut timeline = Timeline::default();
    timeline.advance_dropped(SEC);
    timeline.advance_dropped(2 * SEC);
    timeline.note_audio(3 * SEC);
    let (start, _) = timeline.commit_at(timeline.recorded, true);

    assert_eq!(start, 3 * SEC);
}

/// 洞之后喂进来的音频照样推进时钟（时钟是真实录音时间，不是喂入时间）。
#[test]
fn the_clock_keeps_running_after_a_hole() {
    let mut timeline = Timeline::default();
    timeline.advance_dropped(SEC);
    timeline.advance(SEC);
    assert_eq!(timeline.recorded, 2 * SEC);
}

/// 分片整段早于当前句起点（静音期时钟一直在走，随后重放了旧音频）：
/// 这句退到分片自己的音频起点，不能压成零时长。
#[test]
fn a_chunk_ending_before_the_segment_start_keeps_its_own_span() {
    let mut timeline = Timeline::default();
    timeline.advance(10_000);
    // 静音期：下一句的起点跟着时钟走到 10_000。
    timeline.advance_dropped(0);
    // 重放的分片实际停在 6_800（起点 3_600），整段早于 10_000。
    timeline.note_audio(3_600);
    let (start, end) = timeline.commit_at(6_800, true);
    assert_eq!((start, end), (3_600, 6_800));
}

/// 句子横跨多个重放分片：起点取**第一个**分片的起点，不是定稿那一片的。
///
/// 回归测试：之前把「触发定稿的那片」的起点当句子起点，句子前半段的音频
/// 就落到了区间外面 —— 界面上这句话的时长会短一截。
#[test]
fn a_sentence_over_two_replayed_chunks_starts_at_the_first_one() {
    let mut timeline = Timeline::default();
    timeline.advance(10_000);
    timeline.advance_dropped(0);
    // 第一个重放分片：3_600..6_800；第二个：6_800..10_000。
    timeline.note_audio(3_600);
    // 第二个分片喂进来时，句子的音频起点仍然是第一个分片。
    timeline.note_audio(6_800);
    let (start, end) = timeline.commit_at(10_000, true);
    assert_eq!(
        (start, end),
        (3_600, 10_000),
        "句子的起点应落在最早那片音频上"
    );
}

/// 预录早于句起点，但句子持续到句起点之后时，也必须保留预录的时间。
#[test]
fn replayed_sentence_keeps_its_start_after_crossing_segment_start() {
    let mut timeline = Timeline::default();
    timeline.advance(10_000);
    timeline.advance_dropped(0);
    timeline.advance(3_200);
    timeline.note_audio(3_600);
    timeline.note_audio(6_800);
    timeline.note_audio(10_000);
    assert_eq!(timeline.commit_at(13_200, true), (3_600, 13_200));
}

/// 空定稿不能占用尚未重放的音频，否则下一句会被压成零时长。
#[test]
fn empty_commit_does_not_block_replayed_audio() {
    let mut timeline = Timeline::default();
    timeline.advance(10_000);
    timeline.note_audio(0);
    timeline.commit_at(10_000, false);
    timeline.note_audio(3_600);
    assert_eq!(timeline.commit_at(6_800, true), (3_600, 6_800));
}

/// 同样的重放也不能退进上一句的区间：退无可退时宁可保持零时长。
#[test]
fn a_replayed_chunk_never_reaches_back_into_the_previous_sentence() {
    let mut timeline = Timeline::default();
    timeline.advance(10_000);
    timeline.note_audio(0);
    timeline.commit_at(10_000, true);
    timeline.advance_dropped(0);
    timeline.note_audio(8_000);
    let (start, end) = timeline.commit_at(9_000, true);
    assert_eq!((start, end), (10_000, 10_000), "不能退到上一句里面");
}

/// 重放的分片定稿后，句起点不能被拉回过去 —— 下一句的音频在更靠后的位置。
///
/// 回归测试：之前重放分片定稿时把句起点改成那片分片的终点（比当前句起点
/// 更早），下一句就会从那一片开始 —— 下一句的区间里混进了根本不属于它的
/// 音频，句子之间也可能重叠。
#[test]
fn the_segment_start_never_moves_backwards() {
    let mut timeline = Timeline::default();
    timeline.advance(10_000);
    timeline.advance_dropped(0);
    // 重放的分片整段早于句起点（10_000）。
    timeline.note_audio(3_600);
    timeline.commit_at(6_800, true);
    assert_eq!(
        timeline.segment_start, 10_000,
        "句起点不能被重放的分片拉回过去"
    );

    // 下一句是正常喂进来的实时音频，起点应落在句起点（10_000）。
    timeline.advance(3_200);
    timeline.note_audio(10_000);
    let (start, end) = timeline.commit_at(13_200, true);
    assert_eq!((start, end), (10_000, 13_200));
}
