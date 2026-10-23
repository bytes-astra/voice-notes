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
const STREAM_CHUNK: usize = 3_200;

/// 收尾时补的静音长度（300ms）：让解码器把句尾最后一个字冲出来。
const TAIL_SILENCE_SAMPLES: usize = 4_800;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Segment {
    pub text: String,
    pub start: f32,
    pub end: f32,
}





/// 超过这个长度的句子才改用重叠窗口（秒）。
///
/// 实测（65 秒 / 130 秒连续 TTS）：SenseVoice 直接吃 55 秒或 130 秒的音频，
/// 准确率都是 98.6%，并没有因为超过官方说的 30 秒上限而变差 —— 官方那条
/// 限制更像「建议」而不是硬上限。硬切反而会把边界上的词劈开。
///
/// 但输入越长，编码器的注意力越吃显存（帧数平方级），所以留一个上限：
/// 超过 90 秒就切成 30 秒窗口 + 2 秒重叠，避免极端情况下把内存吃光。

/// 后一段开头应该从哪里切掉（返回**字节**下标，0 = 不切）。
///
/// 找前一段结尾和后一段开头的最长公共部分（比较时忽略标点），
/// 返回后一段里重复部分的结束位置。

/// 合并重叠窗口识别出来的文本：删掉相邻两段重复的部分。


/// 把「缺口」标记按时间插进句子列表，并重拼文本。
///
// ── 超长句切窗：超过 90 秒切成 30 秒窗口 + 2 秒重叠再合并 ──
const MAX_SINGLE_SHOT_SECONDS: usize = 90;

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

/// 是不是缺口占位符（`[[gap:秒数]]`）——和前端 `core.js` 的 `isGapMarker()` 同一套规则。
///
/// 缺口是「丢掉了一段音频」的位置标记，不是句子：它前后都不加空格。

/// 从缺口占位符里取出秒数（`[[gap:3]]` → 3）；不是占位符就返回 None。
///
/// 和前端 `core.js` 的 `GAP_RE`（`/\[\[gap:(\d+)\]\]/`）同一套规则。


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

#[cfg(test)]
mod tests {
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
        };
        let value = serde_json::to_value(&transcript).unwrap();
        assert_eq!(value["durationSeconds"], serde_json::json!(1.5));
        assert_eq!(value["elapsedMs"], serde_json::json!(200));
        assert!(value.get("duration_seconds").is_none());
        assert!(value.get("elapsed_ms").is_none());
    }
}

// ────────────────────────── 流式识别 ──────────────────────────


/// 标点模型（CT-Transformer），给流式定稿句补标点。

/// 流式模型句柄。


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
    /// 纯推理耗时累计（毫秒）
    elapsed_ms: u64,
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
            elapsed_ms: 0,
        }
    }

    /// 喂入 16 kHz 单声道采样，返回这一轮识别出来的句子。
    pub fn push(&mut self, samples: &[f32]) -> Vec<OfflineEvent> {
        let mut events = Vec::new();
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
        events
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
        events
    }

    /// 把 VAD 已经切好的句子取出来识别。
    fn drain(&mut self, events: &mut Vec<OfflineEvent>) {
        while let Some(segment) = self.detector.front() {
            let raw = segment.samples().to_vec();
            let start_samples = segment.start() as usize;
            let mut from = start_samples;
            if !self.drained_first
                && should_pre_roll(start_samples, &self.head[..start_samples.min(self.head.len())], &raw)
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
            let end = start + samples.len() as f32 / SAMPLE_RATE as f32;
            self.detector.pop();

            let started = Instant::now();
            let text = self.recognize_segment(&samples);
            self.elapsed_ms += started.elapsed().as_millis() as u64;
            if text.trim().is_empty() {
                continue;
            }
            let segment = Segment { text, start, end };
            events.push(OfflineEvent::Segment(segment.clone()));
            self.segments.push(segment);
        }
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
            segment_start: 0,
            started: Instant::now(),
        }
    }

    /// 按 200ms 一块送入（与官方示例的 chunk_size=3200 对齐），避免过小分片。
    fn accept(&mut self, chunk: &[f32], events: &mut Vec<StreamEvent>) {
        self.stream.accept_waveform(SAMPLE_RATE, chunk);
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
            self.segment_start = self.fed_samples;
            return;
        }
        let text = match &self.punctuator {
            Some(punctuator) => punctuator.apply(&self.partial),
            None => self.partial.clone(),
        };
        let segment = Segment {
            text,
            start: self.segment_start as f32 / SAMPLE_RATE as f32,
            end: self.fed_samples as f32 / SAMPLE_RATE as f32,
        };
        events.push(StreamEvent::Segment(segment.clone()));
        self.segments.push(segment);
        self.segment_start = self.fed_samples;
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
            self.accept(chunk, &mut events);
        }
        // 补的静音不算音频时长。
        self.fed_samples = self.fed_samples.saturating_sub(TAIL_SILENCE_SAMPLES);
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
            duration_seconds: self.fed_samples as f32 / SAMPLE_RATE as f32,
            elapsed_ms: self.started.elapsed().as_millis() as u64,
            language: "zh-en".into(),
        }
    }
}

#[cfg(test)]
mod gap_tests {
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
        };
        insert_gaps(&mut transcript, Vec::new());
        assert_eq!(transcript.text, "只有一句。");
        assert_eq!(transcript.segments.len(), 1);
    }
}

#[cfg(test)]
mod merge_tests {
    use super::*;

    #[test]
    fn merges_overlapping_text() {
        let texts = vec![
            "今天下午三点我们开一个产品评审会讨论下个版本的排期和优先级主要是把核心流程的体验做顺再把一些边角".to_string(),
            "版本的排期和优先级主要是把核心流程的体验做顺再把一些边角的功能砍掉这样团队能专注".to_string(),
        ];
        let merged = merge_transcripts(&texts);
        println!("合并结果：{merged}");
        assert!(merged.contains("一些边角的功能砍掉"), "边界处被劈开了: {merged}");
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
        let texts = vec!["排期和优先级。".to_string(), "排期和优先级主要是".to_string()];
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
}

#[cfg(test)]
mod pre_roll_tests {
    use super::*;
    use std::path::PathBuf;
    use crate::models;

    #[test]
    fn pre_roll_only_for_speech_like_prefix() {
        let speech = vec![0.1f32; 32_000];
        let segment = vec![0.1f32; 16_000];
        // 0.2s 处起句，段前是语音级能量 → 回扩
        assert!(should_pre_roll(3_200, &speech, &segment));
        // 段前是静音/底噪 → 不回扩（否则会把噪声塞进结果）
        assert!(!should_pre_roll(3_200, &vec![0.001f32; 32_000], &segment));
        // 从 0 开始没有可回扩的音频
        assert!(!should_pre_roll(0, &speech, &segment));
        // 2s 处起句：前面有真实音频，sherpa 的 padding 够用
        assert!(!should_pre_roll(32_000, &speech, &segment));
    }

    #[test]
    fn punctuates_sentences() {
        let Some(model) = std::env::var("VOICE_NOTES_PUNCT_MODEL").ok() else {
            eprintln!("跳过：未设置 VOICE_NOTES_PUNCT_MODEL");
            return;
        };
        let engine = PunctuationEngine::new(&model).expect("加载标点模型");
        let out = engine.apply("开放时间早上九点至下午五点");
        println!("标点结果：{out}");
        assert!(
            out.contains('。') || out.contains('，'),
            "没有补出标点: {out}"
        );
    }

    #[test]
    fn streams_with_punctuation() {
        let (Some(model_dir), Some(punct_model)) = (
            std::env::var("VOICE_NOTES_STREAMING_MODEL_DIR").ok(),
            std::env::var("VOICE_NOTES_PUNCT_MODEL").ok(),
        ) else {
            eprintln!("跳过：缺少流式模型或标点模型环境变量");
            return;
        };
        let dir = PathBuf::from(model_dir);
        let paths = models::StreamingPaths {
            dir: dir.display().to_string(),
            encoder: dir.join("encoder.int8.onnx").display().to_string(),
            decoder: dir.join("decoder.onnx").display().to_string(),
            joiner: dir.join("joiner.int8.onnx").display().to_string(),
            tokens: dir.join("tokens.txt").display().to_string(),
            vad: None,
        };
        let engine = Arc::new(StreamingEngine::new(&paths, &[]).expect("加载流式模型"));
        let punctuator = Arc::new(PunctuationEngine::new(&punct_model).expect("加载标点模型"));
        let wave = sherpa_onnx::Wave::read("tests/fixtures/zh.wav").expect("读取测试音频");
        let mut session = StreamingSession::new(engine, Some(punctuator));
        for chunk in wave.samples().chunks(1_600) {
            session.push(chunk);
        }
        session.finish();
        let transcript = session.transcript();
        println!("带标点结果：{}", transcript.text);
        assert!(
            transcript.text.contains('。') || transcript.text.contains('，'),
            "流式结果没有标点: {}",
            transcript.text
        );
    }
}
