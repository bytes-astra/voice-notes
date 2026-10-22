//! SenseVoiceSmall INT8 推理：Silero VAD 切句 + sherpa-onnx 离线识别。

use std::sync::Arc;
use std::time::Instant;

use serde::Serialize;
use sherpa_onnx::{
    OfflinePunctuation, OfflinePunctuationConfig, OfflinePunctuationModelConfig, OfflineRecognizer,
    OfflineRecognizerConfig, OfflineSenseVoiceModelConfig, SileroVadModelConfig, VadModelConfig,
    VoiceActivityDetector,
};

use crate::models::ModelPaths;

pub const SAMPLE_RATE: i32 = 16_000;
const VAD_WINDOW: usize = 512;
/// 单次送入模型的音频上限（SenseVoice 训练长度 30s）。
const MAX_SEGMENT_SECONDS: f32 = 30.0;
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

pub struct Engine {
    pub language: String,
    pub itn: bool,
    recognizer: OfflineRecognizer,
    vad: Option<VadModelConfig>,
}

impl Engine {
    pub fn new(paths: &ModelPaths, language: &str, itn: bool) -> Result<Self, String> {
        let mut config = OfflineRecognizerConfig::default();
        config.model_config.sense_voice = OfflineSenseVoiceModelConfig {
            model: Some(paths.model.clone()),
            language: Some(language.to_string()),
            use_itn: itn,
        };
        config.model_config.tokens = Some(paths.tokens.clone());
        config.model_config.num_threads = threads();
        config.model_config.provider = Some("cpu".into());
        config.model_config.debug = false;

        let recognizer = OfflineRecognizer::create(&config)
            .ok_or_else(|| "无法加载 SenseVoice 模型（文件可能损坏，建议重新下载）".to_string())?;

        let vad = paths.vad.as_ref().and_then(|_| {
            Some(VadModelConfig {
                silero_vad: SileroVadModelConfig {
                    model: paths.vad.clone(),
                    threshold: 0.5,
                    min_silence_duration: 0.5,
                    min_speech_duration: 0.25,
                    window_size: VAD_WINDOW as i32,
                    max_speech_duration: MAX_SEGMENT_SECONDS,
                },
                sample_rate: SAMPLE_RATE,
                num_threads: 1,
                provider: Some("cpu".into()),
                debug: false,
                ..Default::default()
            })
        });

        Ok(Self {
            language: language.to_string(),
            itn,
            recognizer,
            vad,
        })
    }

    /// VAD 配置：整段模式「边录边识别」要用同一套参数（同样的断句行为）。
    pub fn vad_config(&self) -> Option<VadModelConfig> {
        self.vad.clone()
    }

    /// 对一段已经切好的句子直接解码（**不再跑 VAD**）。
    ///
    /// 边录边识别的路径下 VAD 在外层切句，这里再切一次会改变句子内部的边界，
    /// 所以单独提供一个「直接解码」的入口。
    pub fn recognize(&self, samples: &[f32]) -> String {
        decode(&self.recognizer, samples)
    }

    /// 转写 16 kHz 单声道样本；VAD 可用时按语音段切分，否则按 30s 窗口兜底。
    pub fn transcribe(&self, samples: &[f32]) -> Transcript {
        let started = Instant::now();
        let duration = samples.len() as f32 / SAMPLE_RATE as f32;
        let mut segments = Vec::new();

        if let Some(vad_config) = &self.vad {
            if let Some(detector) =
                VoiceActivityDetector::create(vad_config, MAX_SEGMENT_SECONDS + 2.0)
            {
                detector.reset();
                for chunk in samples.chunks(VAD_WINDOW) {
                    detector.accept_waveform(chunk);
                    while let Some(segment) = detector.front() {
                        let text = decode(&self.recognizer, segment.samples());
                        let start = segment.start() as f32 / SAMPLE_RATE as f32;
                        let end = start + segment.samples().len() as f32 / SAMPLE_RATE as f32;
                        if !text.is_empty() {
                            segments.push(Segment { text, start, end });
                        }
                        detector.pop();
                    }
                }
                detector.flush();
                while let Some(segment) = detector.front() {
                    let text = decode(&self.recognizer, segment.samples());
                    let start = segment.start() as f32 / SAMPLE_RATE as f32;
                    let end = start + segment.samples().len() as f32 / SAMPLE_RATE as f32;
                    if !text.is_empty() {
                        segments.push(Segment { text, start, end });
                    }
                    detector.pop();
                }
            }
        }

        if segments.is_empty() && !samples.is_empty() {
            // VAD 没切出内容（太短、太轻或纯噪音）时，直接按 30s 窗口兜底识别一次。
            let limit = (MAX_SEGMENT_SECONDS * SAMPLE_RATE as f32) as usize;
            for (index, window) in samples.chunks(limit).enumerate() {
                let text = decode(&self.recognizer, window);
                if !text.is_empty() {
                    let start = (index * limit) as f32 / SAMPLE_RATE as f32;
                    segments.push(Segment {
                        text,
                        start,
                        end: start + window.len() as f32 / SAMPLE_RATE as f32,
                    });
                }
            }
        }

        let text = join_segments(segments.iter().map(|s| s.text.as_str()));
        Transcript {
            text,
            segments,
            duration_seconds: duration,
            elapsed_ms: started.elapsed().as_millis() as u64,
            language: self.language.clone(),
        }
    }
}

fn threads() -> i32 {
    std::thread::available_parallelism()
        .map(|n| (n.get() / 2).clamp(1, 4) as i32)
        .unwrap_or(2)
}


/// 超过这个长度的句子才改用重叠窗口（秒）。
///
/// 实测（65 秒 / 130 秒连续 TTS）：SenseVoice 直接吃 55 秒或 130 秒的音频，
/// 准确率都是 98.6%，并没有因为超过官方说的 30 秒上限而变差 —— 官方那条
/// 限制更像「建议」而不是硬上限。硬切反而会把边界上的词劈开。
///
/// 但输入越长，编码器的注意力越吃显存（帧数平方级），所以留一个上限：
/// 超过 90 秒就切成 30 秒窗口 + 2 秒重叠，避免极端情况下把内存吃光。
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

fn decode(recognizer: &OfflineRecognizer, samples: &[f32]) -> String {
    let stream = recognizer.create_stream();
    stream.accept_waveform(SAMPLE_RATE, samples);
    recognizer.decode(&stream);
    stream
        .get_result()
        .map(|r| clean(&r.text))
        .unwrap_or_default()
}

/// 把「缺口」标记按时间插进句子列表，并重拼文本。
///
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
fn clean(text: &str) -> String {
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
pub fn join_segments<'a>(parts: impl Iterator<Item = &'a str>) -> String {
    let mut out = String::new();
    for part in parts {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let need_space = match (out.chars().last(), part.chars().next()) {
            (Some(left), Some(right)) => !is_cjk(left) || !is_cjk(right),
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

use sherpa_onnx::{
    OnlineRecognizer, OnlineRecognizerConfig, OnlineStream, OnlineTransducerModelConfig,
};

use crate::models::StreamingPaths;

impl StreamingEngine {
    /// 加载流式 Zipformer（中英双语，greedy search + 端点检测）。
    pub fn new(paths: &StreamingPaths) -> Result<Self, String> {
        let mut config = OnlineRecognizerConfig::default();
        config.model_config.transducer = OnlineTransducerModelConfig {
            encoder: Some(paths.encoder.clone()),
            decoder: Some(paths.decoder.clone()),
            joiner: Some(paths.joiner.clone()),
        };
        config.model_config.tokens = Some(paths.tokens.clone());
        config.model_config.num_threads = threads();
        config.model_config.provider = Some("cpu".into());
        config.decoding_method = Some("greedy_search".into());
        config.enable_endpoint = true;
        config.rule1_min_trailing_silence = 2.4;
        config.rule2_min_trailing_silence = 1.2;
        config.rule3_min_utterance_length = 20.0;

        let recognizer = OnlineRecognizer::create(&config)
            .ok_or_else(|| "无法加载流式模型（文件可能损坏，建议重新下载）".to_string())?;
        let vad = paths.vad.as_ref().map(|path| VadModelConfig {
            silero_vad: SileroVadModelConfig {
                model: Some(path.clone()),
                threshold: 0.5,
                min_silence_duration: 0.5,
                min_speech_duration: 0.25,
                window_size: 512,
                max_speech_duration: 20.0,
            },
            sample_rate: SAMPLE_RATE,
            num_threads: 1,
            provider: Some("cpu".into()),
            debug: false,
            ..Default::default()
        });
        Ok(Self { recognizer, vad })
    }

    /// 静音门控用的 VAD 配置（模型缺失时为 None，退化为全量喂入）。
    pub fn vad(&self) -> Option<&VadModelConfig> {
        self.vad.as_ref()
    }

    pub fn new_stream(&self) -> OnlineStream {
        self.recognizer.create_stream()
    }
}

/// 标点模型（CT-Transformer），给流式定稿句补标点。
pub struct PunctuationEngine {
    punct: OfflinePunctuation,
}

impl PunctuationEngine {
    pub fn new(model: &str) -> Result<Self, String> {
        let config = OfflinePunctuationConfig {
            model: OfflinePunctuationModelConfig {
                ct_transformer: Some(model.to_string()),
                num_threads: 1,
                debug: false,
                provider: Some("cpu".into()),
            },
        };
        let punct = OfflinePunctuation::create(&config)
            .ok_or_else(|| "无法加载标点模型（文件可能损坏）".to_string())?;
        Ok(Self { punct })
    }

    /// 加标点；失败时原样返回，不影响主流程。
    pub fn apply(&self, text: &str) -> String {
        self.punct
            .add_punctuation(text)
            .filter(|out| !out.trim().is_empty())
            .unwrap_or_else(|| text.to_string())
    }
}

/// 流式模型句柄。
pub struct StreamingEngine {
    pub recognizer: OnlineRecognizer,
    vad: Option<VadModelConfig>,
}


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
    /// 被 VAD 切出并识别成功的句子的样本数
    recognized_samples: usize,
    segments: Vec<Segment>,
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
            recognized_samples: 0,
            segments: Vec::new(),
            elapsed_ms: 0,
        }
    }

    /// 喂入 16 kHz 单声道采样，返回这一轮识别出来的句子。
    pub fn push(&mut self, samples: &[f32]) -> Vec<OfflineEvent> {
        let mut events = Vec::new();
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
            let samples = segment.samples().to_vec();
            let start = segment.start() as f32 / SAMPLE_RATE as f32;
            let end = start + samples.len() as f32 / SAMPLE_RATE as f32;
            self.detector.pop();

            let started = Instant::now();
            let text = self.recognize_segment(&samples);
            self.elapsed_ms += started.elapsed().as_millis() as u64;
            if text.trim().is_empty() {
                continue;
            }
            self.recognized_samples += samples.len();
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

    /// 被 VAD 切出并识别成功的句子的采样数（判断「没识别到内容」用）
    pub fn recognized_samples(&self) -> usize {
        self.recognized_samples
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
        insert_gaps(
            &mut transcript,
            vec![seg("（此处约 3 秒未识别）", 1.0)],
        );
        assert_eq!(transcript.segments.len(), 3);
        assert_eq!(transcript.segments[1].text, "（此处约 3 秒未识别）");
        assert_eq!(transcript.text, "第一句。（此处约 3 秒未识别）第二句。");
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
