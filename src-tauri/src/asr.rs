//! SenseVoiceSmall INT8 推理：Silero VAD 切句 + sherpa-onnx 离线识别。

use std::time::Instant;

use serde::Serialize;
use sherpa_onnx::{
    OfflineRecognizer, OfflineRecognizerConfig, OfflineSenseVoiceModelConfig, SileroVadModelConfig,
    VadModelConfig, VoiceActivityDetector,
};

use crate::models::ModelPaths;

pub const SAMPLE_RATE: i32 = 16_000;
const VAD_WINDOW: usize = 512;
/// 单次送入模型的音频上限（SenseVoice 训练长度 30s）。
const MAX_SEGMENT_SECONDS: f32 = 30.0;

#[derive(Clone, Debug, Serialize)]
pub struct Segment {
    pub text: String,
    pub start: f32,
    pub end: f32,
}

#[derive(Clone, Debug, Serialize)]
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
}
