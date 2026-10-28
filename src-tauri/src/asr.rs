//! 识别会话与转写数据结构：整段（Silero VAD 切句 + SenseVoice）和流式
//! （Zipformer transducer）的会话、Transcript / Segment、以及文本规则
//! （拼句、超长句切窗、pre-roll 回扩）。
//!
//! **模型怎么构造在 engine.rs**（三个引擎 + 加载 + AppState），这里只管
//! 「拿着引擎怎么把音频变成文字」。

use std::sync::Arc;
use std::time::Instant;

use serde::Serialize;
use sherpa_onnx::OnlineStream;
use sherpa_onnx::{VadModelConfig, VoiceActivityDetector};

use crate::audio::{ends_with_gap_marker, is_gap_marker};
use crate::engine::{Engine, PunctuationEngine, StreamingEngine};
use crate::log;

pub const SAMPLE_RATE: i32 = 16_000;

#[derive(Clone, Debug, Serialize)]
// 前端读的是 transcript.durationSeconds / elapsedMs：这里必须是 camelCase，
// 否则时长和耗时会变成 undefined，结果区和历史记录里恒显示 0。
#[serde(rename_all = "camelCase")]
pub struct Transcript {
    pub text: String,
    pub segments: Vec<Segment>,
    pub duration_seconds: f32,
    pub elapsed_ms: u64,
    pub language: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

/// VAD 的窗口大小（512 样本 = 32 ms @16k）：引擎配 VAD 和会话切句都要用。
/// 段起点落在音频开头附近时，段前往回扩的最大距离（0.4s）。
///
/// sherpa 的 VAD 内部 padding 只有 ≈314ms（`2×window + min_speech`）；起音偏软时
/// VAD 会把开头当成静音，padding 追不回来，第一个字就被切掉了。实测 TTS / 剪辑过的
/// 文件（开头直接是语音）每条都会丢首字。
pub const PRE_ROLL_SAMPLES: usize = 6_400;

/// 只有段起点早于这个位置才考虑回扩（0.6s）；更靠后的段前面有真实音频，padding 够用。
pub const PRE_ROLL_MAX_START: usize = 9_600;

fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

/// 第一段是否值得把起点回扩：起点在开头附近，且段前音频有语音级能量。
///
/// 纯静音/底噪不回扩 —— 否则会把开头的噪声当语音塞进结果（实测真人样本
/// real-05 会多出「您好」）。
pub fn should_pre_roll(start: usize, pre: &[f32], segment: &[f32]) -> bool {
    if start == 0 || start > PRE_ROLL_MAX_START {
        return false;
    }
    let pre_rms = rms(pre);
    let seg_rms = rms(segment);
    pre_rms > 0.005 && pre_rms > seg_rms * 0.3
}

pub const VAD_WINDOW: usize = 512;

/// 流式输入分片：3200 样本 = 200ms，与官方示例一致。
pub const STREAM_CHUNK: usize = 3_200;

/// 收尾时补的静音长度（300ms）：让解码器把句尾最后一个字冲出来。
const TAIL_SILENCE_SAMPLES: usize = 4_800;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Segment {
    pub text: String,
    pub start: f32,
    pub end: f32,
}

///
// ── 超长句切窗：超过 90 秒切成 30 秒窗口 + 2 秒重叠再合并 ──
/// 超过这个长度的句子才改用重叠窗口（秒）。
///
/// 实测（65 秒 / 130 秒连续 TTS）：SenseVoice 直接吃 55 秒或 130 秒的音频，
/// 准确率都是 98.6%，并没有因为超过官方说的 30 秒上限而变差 —— 官方那条
/// 限制更像「建议」而不是硬上限。硬切反而会把边界上的词劈开。
///
/// 但输入越长，编码器的注意力越吃显存（帧数平方级），所以留一个上限：
/// 超过 90 秒就切成 30 秒窗口 + 2 秒重叠，避免极端情况下把内存吃光。
const MAX_SINGLE_SHOT_SECONDS: usize = 90;

/// 连续多久没切出句子就要报一次（秒）。
///
/// sherpa 的 VAD 把判为非语音的音频直接丢掉，没有日志、没有缺口标记 —— 这段
/// 音频会凭空消失。这个阈值是「多久算异常」的门槛：正常说话停顿几秒是常事，
/// 超过 10 秒还没有一句，且这段时间音频有能量，就值得报。
const NO_SEGMENT_WARN_SECONDS: f32 = 10.0;

/// 判定「这段时间有声音」的峰值下限（0..1）。低于它就是静音，不报。
const NO_SEGMENT_PEAK_FLOOR: f32 = 0.01;

/// 「长时间没切出句子」的告警文案。返回 `None` = 不用报。
///
/// 拆成自由函数是为了能单测：会话要真模型才跑得起来，单测里没有。
///
/// `vad_holding_speech` = VAD 此刻正在攒一句话（`detector.detected()`）。
/// **这时的「没有句子」是正常的** —— 连续说话时 VAD 要等到句尾才切段，
/// 音频没丢，只是还在攒。真正要报的是「有能量、VAD 却没在攒」：那种情况下
/// 音频被判成非语音，被 sherpa 直接丢掉了。
fn no_segment_warning(
    silence_seconds: f32,
    peak: f32,
    vad_holding_speech: bool,
) -> Option<String> {
    if vad_holding_speech {
        return None;
    }
    if silence_seconds < NO_SEGMENT_WARN_SECONDS || peak < NO_SEGMENT_PEAK_FLOOR {
        return None;
    }
    Some(format!(
        "连续 {silence_seconds:.1}s 没切出句子（期间音频峰值 {peak:.4}）——这段音频可能已被 VAD 判为非语音丢弃"
    ))
}

/// 超长句的窗口参数：30 秒窗口 + 2 秒重叠 + 28 秒步进。
///
/// 相邻窗口重叠 2 秒，这段音频被解两次，两边文本的公共部分就是重叠区，
/// 合并时删掉即可（官方 long_audio_no_vad.py 的做法）。
const WINDOW_SECONDS: usize = 30;
const OVERLAP_SECONDS: usize = 2;
/// 合并时至少要能对上这么多字才算真的重叠上（对不上就保留全部，宁可重复也别丢话）
const MIN_OVERLAP_CHARS: usize = 4;
/// 公共部分最多取多少字（官方 merge_transcripts 也是这个上限）
const MAX_OVERLAP_CHARS: usize = 200;

/// 标点：比较重叠部分时忽略（模型在窗口边界常会多吐一个句号）。
fn is_merge_punct(c: char) -> bool {
    c.is_ascii_punctuation()
        || "\u{3002}\u{ff01}\u{ff1f}\u{3001}\u{ff1b}\u{ff1a}\u{ff08}\u{ff09}\u{300a}\u{300b}\u{2026}\u{2014}\u{201c}\u{201d}\u{2018}\u{2019},.!?;:()"
            .contains(c)
}

/// 后一段开头应该从哪里切掉（返回**字节**下标，0 = 不切）。
///
/// 找前一段结尾和后一段开头的最长公共部分（比较时忽略标点），
/// 返回后一段里重复部分的结束位置。
fn overlap_cut(previous: &str, next: &str) -> usize {
    let prev: Vec<char> = previous.chars().filter(|c| !is_merge_punct(*c)).collect();
    // next 的非标点字符 + 它们在原始字符串里的字节位置
    let mut next_chars: Vec<char> = Vec::new();
    let mut next_bytes: Vec<usize> = Vec::new();
    for (byte, c) in next.char_indices() {
        if !is_merge_punct(c) {
            next_chars.push(c);
            next_bytes.push(byte);
        }
    }
    let max = prev.len().min(next_chars.len()).min(MAX_OVERLAP_CHARS);
    for n in (MIN_OVERLAP_CHARS..=max).rev() {
        if prev[prev.len() - n..] == next_chars[..n] {
            // 切到第 n 个非标点字符之后（原始字节位置）
            return next_bytes.get(n).copied().unwrap_or(next.len());
        }
    }
    0
}

/// 合并重叠窗口识别出来的文本：删掉相邻两段重复的部分。
pub fn merge_transcripts(texts: &[String]) -> String {
    let mut result = String::new();
    for (index, text) in texts.iter().enumerate() {
        if index == 0 {
            result = text.clone();
            continue;
        }
        let cut = overlap_cut(&result, text);
        result.push_str(&text[cut..]);
    }
    result
}

/// 识别跟不上时会丢掉一段音频，丢掉的地方必须在结果里留个记号 ——
/// 静默丢一段音频，用户根本不知道结果缺了一块。
/// 把「缺口」标记按时间插进句子列表，并重拼文本。
pub fn insert_gaps(transcript: &mut Transcript, gaps: Vec<Segment>) {
    if gaps.is_empty() {
        return;
    }
    transcript.segments.extend(gaps);
    transcript.segments.sort_by(|a, b| {
        a.start
            .partial_cmp(&b.start)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    transcript.text = join_segments(transcript.segments.iter().map(|s| s.text.as_str()));
}

/// 去掉 SenseVoice 的 <|zh|><|NEUTRAL|> 之类控制标记，压平空白。
pub fn clean(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_tag = false;
    for ch in text.chars() {
        match ch {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn is_cjk(ch: char) -> bool {
    matches!(ch as u32,
        0x3000..=0x303F   // CJK 标点
        | 0x3040..=0x30FF // 假名
        | 0x3400..=0x4DBF // 扩展 A
        | 0x4E00..=0x9FFF // 统一表意
        | 0xAC00..=0xD7AF // 谚文
        | 0xF900..=0xFAFF // 兼容表意
        | 0xFF00..=0xFFEF // 全角
    )
}

/// 中文/日文/韩文之间直接拼接，其余用空格——与 DSH 的 `join(" ")` 相比更贴近书写习惯。
///
/// 缺口占位符（`[[gap:秒数]]`）前后都不加空格，规则与前端 `core.js` 的 `joinText()` 一致。
pub fn join_segments<'a>(parts: impl Iterator<Item = &'a str>) -> String {
    let mut out = String::new();
    for part in parts {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        // 规则和前端 `joinText()` 逐条对应：
        // 1. 右边是占位符 → 不加空格
        // 2. 左边（已拼好的文本）以占位符结尾 → 不加空格
        // 3. 否则中日韩之间直接连，其余加空格
        let need_space = match (out.chars().last(), part.chars().next()) {
            (Some(left), Some(right)) => {
                if is_gap_marker(part) || ends_with_gap_marker(&out) {
                    false
                } else {
                    !is_cjk(left) || !is_cjk(right)
                }
            }
            _ => false,
        };
        if need_space {
            out.push(' ');
        }
        out.push_str(part);
    }
    out.trim().to_string()
}

/// 短句合并：短于这个长度的句子不立刻定稿，等下一句来了合并成一句再识别。
///
/// VAD 按 `min_silence_duration` 切句，切出来的短句（「好的」「对」）单独送
/// SenseVoice 时上下文太少，容易把词认错；和下一句合成一句再识别，模型能拿到
/// 更长的上下文。参照 FunASR 的 `merge_vad=True, merge_length_s=15`。
const MERGE_MIN_SECONDS: f32 = 3.0;

/// 两句之间隔着这么久就不合并了（秒）。
///
/// 不设上限的话，用户说完「好的」停顿十秒再开口，这两句会被合成一句 —— 音频中间
/// 隔了十秒静音，模型却听成连续的一句话。
const MERGE_MAX_GAP_SECONDS: f32 = 2.0;

/// 可以送去识别的音频段（可能是两句合并出来的）。
pub struct MergedSegment {
    /// 起点（秒，相对会话起点）
    pub start: f32,
    /// 终点（秒）
    pub end: f32,
    /// 16 kHz 单声道采样
    pub samples: Vec<f32>,
}

/// 短句合并器：把 VAD 切出来的短句和下一句合并后再送识别。
///
/// 用法：`push()` 喂进一段，返回「现在可以识别了」的段（可能是空的）；收尾时用
/// `flush()` 把还留着的短句取出来定稿。
pub struct SegmentMerger {
    /// 还没定稿的短句：`(起点秒, 采样)`
    pending: Option<(f32, Vec<f32>)>,
}

impl SegmentMerger {
    pub fn new() -> Self {
        Self { pending: None }
    }

    /// 送入一段 VAD 切好的音频（`start` 是它在会话里的起点，秒）。
    ///
    /// 返回可以送去识别的段：句子够长就当场返回；太短就先留着，等下一句来了
    /// 合并；上一句等太久（间隔超过 `MERGE_MAX_GAP_SECONDS`）就各自定稿。
    pub fn push(&mut self, start: f32, samples: Vec<f32>) -> Vec<MergedSegment> {
        let mut ready = Vec::new();
        if let Some((prev_start, prev_samples)) = self.pending.take() {
            let prev_end = prev_start + prev_samples.len() as f32 / SAMPLE_RATE as f32;
            if start - prev_end < MERGE_MAX_GAP_SECONDS {
                // 间隔不大 → 合成一句送识别
                let mut merged = prev_samples;
                merged.extend_from_slice(&samples);
                ready.push(MergedSegment {
                    start: prev_start,
                    end: start + samples.len() as f32 / SAMPLE_RATE as f32,
                    samples: merged,
                });
                return ready;
            }
            // 隔太久 → 上一句自己定稿，不跨着大段静音合并
            ready.push(MergedSegment {
                start: prev_start,
                end: prev_end,
                samples: prev_samples,
            });
        }
        let end = start + samples.len() as f32 / SAMPLE_RATE as f32;
        if end - start < MERGE_MIN_SECONDS {
            self.pending = Some((start, samples));
        } else {
            ready.push(MergedSegment {
                start,
                end,
                samples,
            });
        }
        ready
    }

    /// 收尾：把还留着的短句取出来定稿。
    pub fn flush(&mut self) -> Vec<MergedSegment> {
        match self.pending.take() {
            Some((start, samples)) => {
                let end = start + samples.len() as f32 / SAMPLE_RATE as f32;
                vec![MergedSegment {
                    start,
                    end,
                    samples,
                }]
            }
            None => Vec::new(),
        }
    }

    /// 还留着的短句的结束时间（秒）。
    pub fn pending_end(&self) -> Option<f32> {
        self.pending
            .as_ref()
            .map(|(start, samples)| start + samples.len() as f32 / SAMPLE_RATE as f32)
    }
}

impl Default for SegmentMerger {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod merger_tests;

#[cfg(test)]
mod tests;

// ────────────────────────── 流式识别 ──────────────────────────

/// 整段模式的「边录边按句识别」会话。
///
/// 和 [`StreamingSession`] 一样是流式的，但用的是 SenseVoice（整段模型）：
/// VAD 在录音过程中持续切句，切出一句就立刻送去识别并产出事件。
///
/// 为什么要单独一个会话：整段模式以前是「录完再把整条音频喂给 VAD」，
/// 那要求把整条录音留在内存里（1 小时 ≈ 115MB）。改成边录边识别后，
/// 识别完的句子立刻丢掉音频只留文本，内存就恒定了。
///
/// 代价：SenseVoice 是无状态的，每句独立识别，跨句上下文丢失 ——
/// 这和整段模式「整条一次识别」时的分段行为其实是一样的。
pub struct OfflineSession {
    engine: Arc<Engine>,
    detector: VoiceActivityDetector,
    /// 还没凑够一个 VAD 窗口（512 样本）的采样
    vad_pending: Vec<f32>,
    /// 已经喂进来的采样总数（用来算时间轴）
    fed_samples: usize,
    segments: Vec<Segment>,
    /// 会话开头的音频（最多 0.6s）：第一段被 VAD 切掉字头时用它回扩。
    head: Vec<f32>,
    /// 是否已经处理过第一段
    drained_first: bool,
    /// 短句合并器：太短的句子先留着，等下一句来了合成一句再识别
    merger: SegmentMerger,
    /// 纯推理耗时累计（毫秒）
    elapsed_ms: u64,
    /// 上一句的时间（会话时间轴，秒）
    last_segment_at: f32,
    /// 上次做「长时间没切出句子」检查的时间（秒）
    last_check_at: f32,
    /// 自上次检查以来收到的音频峰值
    window_peak: f32,
}

/// 整段会话产出的事件。
#[derive(Clone, Debug)]
pub enum OfflineEvent {
    /// 识别好的一句
    Segment(Segment),
}

impl OfflineSession {
    pub fn new(engine: Arc<Engine>, vad: VadModelConfig) -> Self {
        // 缓冲给 60 秒：sherpa 的 max_speech_duration 不是硬切，而是把 VAD 参数
        // 调严（min_silence 0.5→0.1s、threshold 0.5→0.9）逼它找静音。真的连续
        // 说话时它找不到，句子可以长到 1 分钟以上 —— 缓冲不够就会触发扩容
        // （sherpa 会打印 Overflow 警告）。
        let detector = VoiceActivityDetector::create(&vad, 60.0).expect("创建 VAD 失败");
        Self {
            engine,
            detector,
            vad_pending: Vec::new(),
            fed_samples: 0,
            segments: Vec::new(),
            head: Vec::new(),
            drained_first: false,
            merger: SegmentMerger::new(),
            elapsed_ms: 0,
            last_segment_at: 0.0,
            last_check_at: 0.0,
            window_peak: 0.0,
        }
    }

    /// 喂入 16 kHz 单声道采样，返回这一轮识别出来的句子。
    pub fn push(&mut self, samples: &[f32]) -> Vec<OfflineEvent> {
        let mut events = Vec::new();
        self.note_audio_energy(samples);
        if !self.drained_first && self.head.len() < PRE_ROLL_MAX_START {
            let take = (PRE_ROLL_MAX_START - self.head.len()).min(samples.len());
            self.head.extend_from_slice(&samples[..take]);
        }
        self.vad_pending.extend_from_slice(samples);
        self.fed_samples += samples.len();
        while self.vad_pending.len() >= VAD_WINDOW {
            let window: Vec<f32> = self.vad_pending.drain(..VAD_WINDOW).collect();
            self.detector.accept_waveform(&window);
            self.drain(&mut events);
        }
        // 短句等太久（后面一直是静音）→ 别让用户干等，先定稿
        self.flush_stale_pending(&mut events);
        events
    }

    /// 记下收到的音频能量，并检查「有声音却长时间切不出句子」。
    ///
    /// sherpa 的 VAD 对判为非语音的音频是**直接丢弃**的
    /// （`voice-activity-detector.cc` 的 else 分支只 `buffer_.Pop`，不产生
    /// segment、不报错）。所以「这段话被整段吞掉」在日志、转写、界面上都是
    /// 零痕迹 —— 只有在这里补一条告警才看得见。
    ///
    /// 峰值低于 `NO_SEGMENT_PEAK_FLOOR` 的就是静音，不值得报（否则纯静音的
    /// 录音会刷屏）。
    fn note_audio_energy(&mut self, samples: &[f32]) {
        let peak = samples.iter().fold(0.0f32, |a, b| a.max(b.abs()));
        self.window_peak = self.window_peak.max(peak);
        let now = self.fed_samples as f32 / SAMPLE_RATE as f32;
        if now - self.last_check_at < NO_SEGMENT_WARN_SECONDS {
            return;
        }
        if let Some(message) = no_segment_warning(
            now - self.last_segment_at,
            self.window_peak,
            self.detector.detected(),
        ) {
            log::warn(message);
        }
        self.window_peak = 0.0;
        self.last_check_at = now;
    }

    /// 短句已经等够了（后面一直没人说话）→ 定稿。
    ///
    /// 不这样处理的话，用户说完「好的」停顿几秒，这两个字要一直等到下一句话
    /// 出现（或停录）才显示出来。VAD 切出一段之后，后面必然是静音（切段的前提
    /// 就是凑够了 `min_silence_duration`），所以「距上一段结束多久」就是静音时长。
    fn flush_stale_pending(&mut self, events: &mut Vec<OfflineEvent>) {
        // 用 VAD 自己的时间轴：它收到的是 fed_samples 减去还没喂进去的余量
        let now = (self.fed_samples - self.vad_pending.len()) as f32 / SAMPLE_RATE as f32;
        let Some(pending_end) = self.merger.pending_end() else {
            return;
        };
        if now - pending_end >= MERGE_MAX_GAP_SECONDS {
            for merged in self.merger.flush() {
                self.recognize_and_emit(merged, events);
            }
        }
    }

    /// 收尾：把 VAD 里还没成句的剩余音频冲出来识别。
    pub fn finish(&mut self) -> Vec<OfflineEvent> {
        let mut events = Vec::new();
        if !self.vad_pending.is_empty() {
            let rest = std::mem::take(&mut self.vad_pending);
            self.detector.accept_waveform(&rest);
        }
        self.detector.flush();
        self.drain(&mut events);
        // 还在等的短句：收尾时定稿
        for merged in self.merger.flush() {
            self.recognize_and_emit(merged, &mut events);
        }
        events
    }

    /// 把 VAD 已经切好的句子取出来识别。
    fn drain(&mut self, events: &mut Vec<OfflineEvent>) {
        while let Some(segment) = self.detector.front() {
            let raw = segment.samples().to_vec();
            let start_samples = segment.start() as usize;
            let mut from = start_samples;
            if !self.drained_first
                && should_pre_roll(
                    start_samples,
                    &self.head[..start_samples.min(self.head.len())],
                    &raw,
                )
            {
                from = start_samples.saturating_sub(PRE_ROLL_SAMPLES);
            }
            self.drained_first = true;
            let samples: Vec<f32> = if from < start_samples && start_samples <= self.head.len() {
                let mut merged = self.head[from..start_samples].to_vec();
                merged.extend_from_slice(&raw);
                merged
            } else {
                raw
            };
            let start = from as f32 / SAMPLE_RATE as f32;
            self.detector.pop();

            // 短句合并：太短的句子先留着，等下一句来了合成一句再识别
            for merged in self.merger.push(start, samples) {
                self.recognize_and_emit(merged, events);
            }
        }
    }

    /// 识别一段（可能是合并出来的）音频并产出事件。
    fn recognize_and_emit(&mut self, merged: MergedSegment, events: &mut Vec<OfflineEvent>) {
        let started = Instant::now();
        let text = self.recognize_segment(&merged.samples);
        self.elapsed_ms += started.elapsed().as_millis() as u64;
        if text.trim().is_empty() {
            // 识别不出东西的音频段直接丢掉 —— 但不能悄无声息：记一条，
            // 否则这段音频在日志里也是零痕迹。
            log::warn(format!(
                "识别结果为空，{:.1}s 的音频被丢掉",
                merged.end - merged.start
            ));
            return;
        }
        let segment = Segment {
            text,
            start: merged.start,
            end: merged.end,
        };
        self.last_segment_at = merged.end;
        events.push(OfflineEvent::Segment(segment.clone()));
        self.segments.push(segment);
    }

    /// 识别一段切好的音频。
    ///
    /// 常规长度直接一次识别（实测最准）；超过 `MAX_SINGLE_SHOT_SECONDS`
    /// 才改用重叠窗口，防止超长输入把模型显存吃光。
    fn recognize_segment(&self, samples: &[f32]) -> String {
        let single_limit = MAX_SINGLE_SHOT_SECONDS * SAMPLE_RATE as usize;
        if samples.len() <= single_limit {
            return self.engine.recognize(samples);
        }
        let window = WINDOW_SECONDS * SAMPLE_RATE as usize;
        let step = window - OVERLAP_SECONDS * SAMPLE_RATE as usize;
        let mut texts: Vec<String> = Vec::new();
        let mut start = 0usize;
        while start < samples.len() {
            let end = (start + window).min(samples.len());
            texts.push(self.engine.recognize(&samples[start..end]));
            if end >= samples.len() {
                break;
            }
            start += step;
        }
        merge_transcripts(&texts)
    }

    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }

    /// 纯推理耗时累计（毫秒）
    pub fn elapsed_ms(&self) -> u64 {
        self.elapsed_ms
    }

    /// 喂进来的采样总数（判断「录得太短」用；不含 VAD 内部缓冲）
    pub fn recorded_samples(&self) -> usize {
        self.fed_samples
    }
}

/// 一次流式识别会话：喂 PCM，收中间结果和断句。
pub struct StreamingSession {
    recognizer: Arc<StreamingEngine>,
    punctuator: Option<Arc<PunctuationEngine>>,
    stream: OnlineStream,
    partial: String,
    segments: Vec<Segment>,
    pending: Vec<f32>,
    fed_samples: usize,
    tail_silence_samples: usize,
    recorded_samples: usize,
    externally_timed: bool,
    segment_start: usize,
    started: Instant,
}

/// 流式回调事件。
#[derive(Clone, Debug)]
pub enum StreamEvent {
    Partial(String),
    Segment(Segment),
}

impl StreamingSession {
    pub fn new(engine: Arc<StreamingEngine>, punctuator: Option<Arc<PunctuationEngine>>) -> Self {
        let stream = engine.new_stream();
        Self {
            recognizer: engine,
            punctuator,
            stream,
            partial: String::new(),
            segments: Vec::new(),
            pending: Vec::new(),
            fed_samples: 0,
            tail_silence_samples: 0,
            recorded_samples: 0,
            externally_timed: false,
            segment_start: 0,
            started: Instant::now(),
        }
    }

    /// 真实录音时钟：即使音频只用于预录或被 VAD 门控，也占录音时长。
    /// 每块重采样后的输入只调用一次；缺口和补入的尾部静音不在这里计数。
    pub fn advance_time(&mut self, samples: &[f32]) {
        self.advance_samples(samples.len());
    }

    /// 丢弃的音频也占录音时间；调用方负责换算到 16 kHz。
    pub fn advance_samples(&mut self, samples: usize) {
        self.recorded_samples += samples;
        self.externally_timed = true;
    }

    /// 当前真实录音时间，供缺口标记和句子使用同一时间轴。
    pub fn elapsed_seconds(&self) -> f32 {
        self.recorded_samples as f32 / SAMPLE_RATE as f32
    }

    /// 按 200ms 一块送入（与官方示例的 chunk_size=3200 对齐），避免过小分片。
    fn accept(&mut self, chunk: &[f32], events: &mut Vec<StreamEvent>) {
        self.stream.accept_waveform(SAMPLE_RATE, chunk);
        if !self.externally_timed {
            self.recorded_samples += chunk.len();
        }
        self.fed_samples += chunk.len();
        self.pump(events);
    }

    /// 只在 ready 时 decode，否则 sherpa-onnx 的特征缓冲会越界。
    fn pump(&mut self, events: &mut Vec<StreamEvent>) {
        while self.recognizer.recognizer.is_ready(&self.stream) {
            self.recognizer.recognizer.decode(&self.stream);
            if let Some(result) = self.recognizer.recognizer.get_result(&self.stream) {
                let text = clean(&result.text);
                if text != self.partial {
                    self.partial = text.clone();
                    if !text.is_empty() {
                        events.push(StreamEvent::Partial(text));
                    }
                }
            }
            if self.recognizer.recognizer.is_endpoint(&self.stream) {
                self.commit(events);
                self.recognizer.recognizer.reset(&self.stream);
            }
        }
    }

    fn commit(&mut self, events: &mut Vec<StreamEvent>) {
        if self.partial.is_empty() {
            self.segment_start = self.recorded_samples;
            return;
        }
        let text = match &self.punctuator {
            Some(punctuator) => punctuator.apply(&self.partial),
            None => self.partial.clone(),
        };
        let segment = Segment {
            text,
            start: self.segment_start as f32 / SAMPLE_RATE as f32,
            end: self.recorded_samples as f32 / SAMPLE_RATE as f32,
        };
        events.push(StreamEvent::Segment(segment.clone()));
        self.segments.push(segment);
        self.segment_start = self.recorded_samples;
        self.partial.clear();
    }

    /// 喂入 16 kHz 单声道样本，返回需要推给 UI 的事件。
    pub fn push(&mut self, samples: &[f32]) -> Vec<StreamEvent> {
        let mut events = Vec::new();
        self.pending.extend_from_slice(samples);
        while self.pending.len() >= STREAM_CHUNK {
            let chunk: Vec<f32> = self.pending.drain(..STREAM_CHUNK).collect();
            self.accept(&chunk, &mut events);
        }
        events
    }

    /// 检测到静音时主动收尾：提交当前句子并重置解码器。
    pub fn force_end(&mut self) -> Vec<StreamEvent> {
        let mut events = Vec::new();
        self.commit(&mut events);
        self.recognizer.recognizer.reset(&self.stream);
        self.partial.clear();
        events
    }

    /// 结束输入，冲刷最后一段。
    pub fn finish(&mut self) -> Vec<StreamEvent> {
        let mut events = Vec::new();
        if !self.pending.is_empty() {
            let chunk = std::mem::take(&mut self.pending);
            self.accept(&chunk, &mut events);
        }
        // 结尾补一小段静音：解码器需要一点尾部上下文才会吐出最后一个字，
        // 否则句尾会被丢掉（“产品评审会” → “产品评审”）。
        let silence = vec![0.0f32; TAIL_SILENCE_SAMPLES];
        for chunk in silence.chunks(STREAM_CHUNK) {
            self.tail_silence_samples += chunk.len();
            self.accept(chunk, &mut events);
        }
        // 补的静音不算音频时长，但未喂出的真实尾包必须保留。
        self.fed_samples = self.fed_samples.saturating_sub(TAIL_SILENCE_SAMPLES);
        if !self.externally_timed {
            self.recorded_samples = self
                .recorded_samples
                .saturating_sub(self.tail_silence_samples);
        }
        self.stream.input_finished();
        self.pump(&mut events);
        self.commit(&mut events);
        events
    }

    /// 汇总为最终结果（无标点，流式模型本身不输出标点）。
    pub fn transcript(self) -> Transcript {
        let text = join_segments(self.segments.iter().map(|s| s.text.as_str()));
        Transcript {
            text,
            segments: self.segments,
            duration_seconds: self.recorded_samples as f32 / SAMPLE_RATE as f32,
            elapsed_ms: self.started.elapsed().as_millis() as u64,
            language: "zh-en".into(),
            warning: None,
        }
    }
}

#[cfg(test)]
mod streaming_time_tests;

#[cfg(test)]
mod gap_tests;

#[cfg(test)]
mod merge_tests;

#[cfg(test)]
mod pre_roll_tests;
