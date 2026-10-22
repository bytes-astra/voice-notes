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
    /// 最终文本是否来自非流式校正（流式模式结束后用 SenseVoice 复核过）。
    pub corrected: bool,
    /// 识别结果明显异常时的提示标记（token，不是文案；前端自己翻译）。
    pub quality_warning: Option<String>,
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
            corrected: false,
            quality_warning: None,
        }
    }
}

fn threads() -> i32 {
    std::thread::available_parallelism()
        .map(|n| (n.get() / 2).clamp(1, 4) as i32)
        .unwrap_or(2)
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
            corrected: false,
            quality_warning: None,
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
            corrected: false,
            quality_warning: None,
        }
    }
}
