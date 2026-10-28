//! 三个引擎（整段 SenseVoice / 流式 Zipformer / 标点 CT-Transformer）的
//! 构造与加载，加上 AppState 和界面要的 Status。
//!
//! 「模型长什么样」在这里，「拿着模型怎么识别」在 asr.rs。

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::Serialize;
use sherpa_onnx::{
    OfflinePunctuation, OfflinePunctuationConfig, OfflinePunctuationModelConfig, OfflineRecognizer,
    OfflineRecognizerConfig, OfflineSenseVoiceModelConfig, SileroVadModelConfig, VadModelConfig,
    VoiceActivityDetector,
};
use tauri::{AppHandle, Emitter};

use crate::asr::{
    clean, join_segments, should_pre_roll, MergedSegment, Segment, SegmentMerger, Transcript,
    PRE_ROLL_SAMPLES, SAMPLE_RATE, VAD_WINDOW,
};
use crate::audio::Recorder;
use crate::log::{self, on_off};
use crate::models;
use crate::models::ModelPaths;
use crate::record::RecordingRun;
use crate::settings::{self, Settings};
use crate::{code, code_detail};
pub struct AppState {
    pub settings: Mutex<Settings>,
    pub recorder: Mutex<Option<Recorder>>,
    pub recording_starting: AtomicBool,
    pub next_recording_id: std::sync::atomic::AtomicU64,
    pub engine: Arc<Mutex<Option<Arc<Engine>>>>,
    pub streaming_engine: Arc<Mutex<Option<Arc<StreamingEngine>>>>,
    pub punct_engine: Arc<Mutex<Option<Arc<PunctuationEngine>>>>,
    pub stream: Mutex<Option<RecordingRun>>,
    pub offline_session: Mutex<Option<RecordingRun>>,
    pub cancel_download: Arc<AtomicBool>,
    pub downloading: Arc<AtomicBool>,
    /// 流式模型下载有自己的一套标志：整段在下载时不该把流式也挡住
    /// （两个下载可以同时跑，进度靠事件载荷里的 kind 区分）。
    pub cancel_download_streaming: Arc<AtomicBool>,
    pub downloading_streaming: Arc<AtomicBool>,
}

pub struct RecordingStartGuard<'a>(pub &'a AtomicBool);

pub struct DownloadFlagGuard(Arc<AtomicBool>);

impl RecordingStartGuard<'_> {
    pub fn release(self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

impl Drop for RecordingStartGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

impl DownloadFlagGuard {
    pub fn new(flag: Arc<AtomicBool>) -> Self {
        Self(flag)
    }
}

impl Drop for DownloadFlagGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

impl AppState {
    pub fn new(settings: Settings) -> Self {
        Self {
            settings: Mutex::new(settings),
            recorder: Mutex::new(None),
            recording_starting: AtomicBool::new(false),
            next_recording_id: std::sync::atomic::AtomicU64::new(1),
            engine: Arc::new(Mutex::new(None)),
            streaming_engine: Arc::new(Mutex::new(None)),
            punct_engine: Arc::new(Mutex::new(None)),
            stream: Mutex::new(None),
            offline_session: Mutex::new(None),
            cancel_download: Arc::new(AtomicBool::new(false)),
            downloading: Arc::new(AtomicBool::new(false)),
            cancel_download_streaming: Arc::new(AtomicBool::new(false)),
            downloading_streaming: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn settings(&self) -> Settings {
        self.settings.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineState {
    phase: &'static str,
    pub message: Option<String>,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub model_ready: bool,
    pub model_source: Option<String>,
    pub model_dir: Option<String>,
    pub model_dir_custom: bool,
    pub vad_ready: bool,
    pub output_dir: String,
    pub language: String,
    pub mode: String,
    pub itn: bool,
    /// 整段模型正在下载
    pub downloading: bool,
    /// 流式模型正在下载（两个下载各用各的标志，可以同时跑）
    pub downloading_streaming: bool,
    pub installed_bytes: u64,
    pub total_bytes: u64,
    pub streaming_ready: bool,
    pub streaming_dir: Option<String>,
    pub streaming_bytes: u64,
    pub streaming_total_bytes: u64,
    pub punct: bool,
    pub punct_ready: bool,
    pub hotwords: Vec<String>,
    pub input_device: Option<String>,
    pub punct_bytes: u64,
    pub punct_total_bytes: u64,
    pub app_version: String,
}

pub fn output_dir(app: &AppHandle, settings: &Settings) -> PathBuf {
    settings
        .output_dir
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(|| settings::default_output_dir(app))
}

pub fn status_of(app: &AppHandle, state: &AppState) -> Status {
    let settings = state.settings();
    let paths = models::resolve(app, settings.model_dir.as_deref());
    let installed_bytes = models::install_dir(app)
        .ok()
        .map(|dir| {
            models::ASSETS
                .iter()
                .filter_map(|asset| fs::metadata(dir.join(asset.name)).ok())
                .map(|meta| meta.len())
                .sum()
        })
        .unwrap_or(0);
    let total_bytes: u64 = models::ASSETS.iter().map(|a| a.bytes).sum();
    let streaming = models::resolve_streaming(app);
    let streaming_bytes = models::streaming_install_dir(app)
        .ok()
        .map(|dir| {
            models::STREAM_ASSETS
                .iter()
                .filter_map(|asset| fs::metadata(dir.join(asset.name)).ok())
                .map(|meta| meta.len())
                .sum()
        })
        .unwrap_or(0);
    Status {
        model_ready: paths.is_some(),
        vad_ready: paths.as_ref().map(|p| p.vad.is_some()).unwrap_or(false),
        model_source: paths.as_ref().map(|p| p.source.clone()),
        model_dir: paths.as_ref().map(|p| p.dir.clone()),
        model_dir_custom: settings.model_dir.is_some(),
        output_dir: output_dir(app, &settings).to_string_lossy().into_owned(),
        language: settings.language.clone(),
        mode: settings.mode.clone(),
        itn: settings.itn,
        // 两个下载各用各的标志：整段在下载时不该把流式也标成忙。
        downloading: state.downloading.load(Ordering::Relaxed),
        downloading_streaming: state.downloading_streaming.load(Ordering::Relaxed),
        installed_bytes,
        total_bytes,
        streaming_ready: streaming.is_some(),
        streaming_dir: streaming.map(|p| p.dir),
        streaming_bytes,
        // 流式下载 = 流式模型 + 标点模型
        streaming_total_bytes: models::streaming_total_bytes() + models::PUNCT.bytes,
        punct: settings.punct,
        punct_ready: models::resolve_punctuation(app).is_some(),
        hotwords: settings.hotwords.clone(),
        input_device: settings.input_device.clone(),
        punct_bytes: models::punctuation_install_dir(app)
            .ok()
            .and_then(|dir| fs::metadata(dir.join(models::PUNCT.name)).ok())
            .map(|meta| meta.len())
            .unwrap_or(0),
        punct_total_bytes: models::PUNCT.bytes,
        app_version: app.package_info().version.to_string(),
    }
}

pub fn persist(app: &AppHandle, state: &AppState) -> Result<(), String> {
    let settings = state.settings();
    settings::save(app, &settings)
}

pub async fn load_engine(app: &AppHandle, state: &AppState) -> Result<(), String> {
    let settings = state.settings();
    let paths = models::resolve(app, settings.model_dir.as_deref())
        .ok_or_else(|| code("model.needOffline"))?;
    let engine = state.engine.clone();
    let language = settings.language.clone();
    let itn = settings.itn;
    let handle = app.clone();
    let _ = handle.emit(
        "engine:state",
        EngineState {
            phase: "loading",
            message: None,
        },
    );
    let outcome = tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        let mut guard = engine.lock().map_err(|_| code("error.engineState"))?;
        let stale = guard
            .as_ref()
            .map(|e| e.language != language || e.itn != itn)
            .unwrap_or(true);
        if stale {
            let loaded = Engine::new(&paths, &language, itn)?;
            log::info(format!(
                "离线引擎加载完成：语言 {language}，ITN {}，模型 {}",
                on_off(itn),
                paths.dir
            ));
            *guard = Some(Arc::new(loaded));
        }
        Ok(())
    })
    .await
    .map_err(|e| code_detail("model.loadFailed", e))?;

    let payload = match &outcome {
        Ok(()) => EngineState {
            phase: "ready",
            message: None,
        },
        Err(err) => EngineState {
            phase: "error",
            message: Some(err.clone()),
        },
    };
    let _ = handle.emit("engine:state", payload);
    outcome
}

pub async fn ensure_streaming_engine(
    app: &AppHandle,
    state: &AppState,
) -> Result<Arc<StreamingEngine>, String> {
    if let Some(engine) = state
        .streaming_engine
        .lock()
        .ok()
        .and_then(|slot| slot.clone())
    {
        return Ok(engine);
    }
    let paths = models::resolve_streaming(app).ok_or_else(|| code("model.needStreaming"))?;
    let hotwords = state.settings().hotwords;
    let _ = app.emit(
        "streaming:state",
        EngineState {
            phase: "loading",
            message: None,
        },
    );
    // 检查和加载放在同一把锁里：预热和点录音并发时只会加载一份 161 MB 模型。
    let slot = state.streaming_engine.clone();
    let engine =
        tauri::async_runtime::spawn_blocking(move || -> Result<Arc<StreamingEngine>, String> {
            let mut guard = slot.lock().map_err(|_| code("error.streamEngineState"))?;
            if let Some(engine) = guard.as_ref() {
                return Ok(engine.clone());
            }
            let engine = Arc::new(StreamingEngine::new(&paths, &hotwords)?);
            log::info(format!(
                "流式引擎加载完成：热词 {} 条，模型 {}",
                hotwords.len(),
                paths.dir
            ));
            *guard = Some(engine.clone());
            Ok(engine)
        })
        .await
        .map_err(|e| code_detail("model.streamLoadFailed", e))??;
    let _ = app.emit(
        "streaming:state",
        EngineState {
            phase: "ready",
            message: None,
        },
    );
    Ok(engine)
}

pub async fn ensure_punctuation(
    app: &AppHandle,
    state: &AppState,
) -> Result<Arc<PunctuationEngine>, String> {
    if let Some(engine) = state.punct_engine.lock().ok().and_then(|slot| slot.clone()) {
        return Ok(engine);
    }
    let model = models::resolve_punctuation(app).ok_or_else(|| code("model.punctNotReady"))?;
    // 同上：检查和加载在同一把锁里，避免并发加载两份 294 MB 标点模型。
    let slot = state.punct_engine.clone();
    let engine =
        tauri::async_runtime::spawn_blocking(move || -> Result<Arc<PunctuationEngine>, String> {
            let mut guard = slot.lock().map_err(|_| code("error.punctEngineState"))?;
            if let Some(engine) = guard.as_ref() {
                return Ok(engine.clone());
            }
            let engine = Arc::new(PunctuationEngine::new(&model)?);
            log::info(format!("标点引擎加载完成：模型 {model}"));
            *guard = Some(engine.clone());
            Ok(engine)
        })
        .await
        .map_err(|e| code_detail("model.punctLoadFailed", e))??;
    Ok(engine)
}

pub fn shared_engine(state: &AppState) -> Result<Arc<Engine>, String> {
    let guard = state.engine.lock().map_err(|_| code("error.engineState"))?;
    guard
        .as_ref()
        .cloned()
        .ok_or_else(|| code("error.engineNotReady"))
}

// ──────────────────── 引擎构造 ────────────────────

/// VAD 认为「一句话太长」的阈值：sherpa 的 max_speech_duration 不是硬切，
/// 是「把 VAD 参数调严」（min_silence 0.5→0.1、threshold 0.5→0.9 逼它找静音），
/// 连续说话时找不到，句子可以长到 1 分钟以上。给 32 秒会触发 CircularBuffer 扩容。
const MAX_SEGMENT_SECONDS: f32 = 30.0;

/// VAD 的静音阈值：句间要静音多久才算断句（秒）。
///
/// 0.5 秒比 Silero 官方默认（0.1 秒）保守 5 倍，句子拖得长、尾巴带静音。
/// 调到 0.3 治「拖带静音」。**整段和流式必须一致** —— 否则两种模式的断句行为
/// 不一样，同一段音频在两个模式里会切出不同数量的句子。
const VAD_MIN_SILENCE: f32 = 0.3;

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

        // 细节是模型路径（技术信息，不是展示文案）：字典里 model.loadFailed 要一个 {error}。
        let recognizer = OfflineRecognizer::create(&config)
            .ok_or_else(|| code_detail("model.loadFailed", &paths.model))?;

        let vad = paths.vad.as_ref().map(|_| VadModelConfig {
            silero_vad: SileroVadModelConfig {
                model: paths.vad.clone(),
                threshold: 0.5,
                min_silence_duration: VAD_MIN_SILENCE,
                min_speech_duration: 0.25,
                window_size: VAD_WINDOW as i32,
                max_speech_duration: MAX_SEGMENT_SECONDS,
            },
            sample_rate: SAMPLE_RATE,
            num_threads: 1,
            provider: Some("cpu".into()),
            debug: false,
            ..Default::default()
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

    /// 从 VAD 里取出一段（含 pre-roll 回扩）。
    ///
    /// 返回 `(起点秒, 采样)`；VAD 里没有段了返回 `None`。`is_first` 用来保证
    /// pre-roll 只作用在第一段上。
    fn take_vad_segment(
        detector: &VoiceActivityDetector,
        samples: &[f32],
        is_first: &mut bool,
    ) -> Option<(f32, Vec<f32>)> {
        let segment = detector.front()?;
        let raw = segment.samples();
        let start_samples = segment.start() as usize;
        let mut from = start_samples;
        if *is_first
            && should_pre_roll(
                start_samples,
                &samples[..start_samples.min(samples.len())],
                raw,
            )
        {
            from = start_samples.saturating_sub(PRE_ROLL_SAMPLES);
        }
        *is_first = false;
        let end = (start_samples + raw.len()).min(samples.len());
        let audio = samples[from..end].to_vec();
        detector.pop();
        Some((from as f32 / SAMPLE_RATE as f32, audio))
    }

    /// 识别一段（可能是短句合并出来的）音频，追加到段落列表。
    fn push_decoded(&self, merged: &MergedSegment, segments: &mut Vec<Segment>) {
        let text = decode(&self.recognizer, &merged.samples);
        if !text.is_empty() {
            segments.push(Segment {
                text,
                start: merged.start,
                end: merged.end,
            });
        }
    }

    /// 转写 16 kHz 单声道样本；VAD 可用时按语音段切分，否则按 30s 窗口兜底。
    pub fn transcribe(&self, samples: &[f32]) -> Transcript {
        let started = Instant::now();
        let duration = samples.len() as f32 / SAMPLE_RATE as f32;
        let mut segments = Vec::new();

        if let Some(vad_config) = &self.vad {
            if let Some(detector) = VoiceActivityDetector::create(vad_config, 60.0) {
                detector.reset();
                let mut first = true;
                let mut merger = SegmentMerger::new();
                for chunk in samples.chunks(VAD_WINDOW) {
                    detector.accept_waveform(chunk);
                    while let Some((start, audio)) =
                        Self::take_vad_segment(&detector, samples, &mut first)
                    {
                        for merged in merger.push(start, audio) {
                            self.push_decoded(&merged, &mut segments);
                        }
                    }
                }
                detector.flush();
                while let Some((start, audio)) =
                    Self::take_vad_segment(&detector, samples, &mut first)
                {
                    for merged in merger.push(start, audio) {
                        self.push_decoded(&merged, &mut segments);
                    }
                }
                // 还在等的短句：收尾时定稿
                for merged in merger.flush() {
                    self.push_decoded(&merged, &mut segments);
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
            warning: None,
        }
    }
}

fn threads() -> i32 {
    std::thread::available_parallelism()
        .map(|n| (n.get() / 2).clamp(1, 4) as i32)
        .unwrap_or(2)
}

use sherpa_onnx::{
    OnlineRecognizer, OnlineRecognizerConfig, OnlineStream, OnlineTransducerModelConfig,
};

use crate::models::StreamingPaths;

impl StreamingEngine {
    /// 加载流式 Zipformer（中英双语，+ 可选的术语表热词）。
    ///
    /// 热词要求 `modified_beam_search`（greedy 不支持 context graph），没配词表时
    /// 保持 greedy，避免给不用热词的人平白加解码延迟。
    pub fn new(paths: &StreamingPaths, hotwords: &[String]) -> Result<Self, String> {
        let words = hotwords.join("\n");
        let use_hotwords = !words.trim().is_empty();
        let mut config = OnlineRecognizerConfig::default();
        config.model_config.transducer = OnlineTransducerModelConfig {
            encoder: Some(paths.encoder.clone()),
            decoder: Some(paths.decoder.clone()),
            joiner: Some(paths.joiner.clone()),
        };
        config.model_config.tokens = Some(paths.tokens.clone());
        config.model_config.num_threads = threads();
        config.model_config.provider = Some("cpu".into());
        config.decoding_method = Some(
            if use_hotwords {
                "modified_beam_search"
            } else {
                "greedy_search"
            }
            .into(),
        );
        if use_hotwords {
            // 用 config 级 hotwords_buf：per-stream 的 create_stream_with_hotwords 在本机
            // 实测对 cjkchar 模型不生效（文本不变），buf 方式可以。
            config.hotwords_buf = Some(words.as_bytes().to_vec());
            config.hotwords_score = 1.5;
        }
        config.enable_endpoint = true;
        config.rule1_min_trailing_silence = 2.4;
        config.rule2_min_trailing_silence = 1.2;
        // rule3 是「超过这个长度强制断句」：20 秒会把连续说话硬切一刀（实测 24.5s
        // 连续语音在 20.368s 触发）。真正的断句由 VAD 门控负责，这里只留兜底。
        config.rule3_min_utterance_length = 60.0;

        let recognizer = OnlineRecognizer::create(&config)
            .ok_or_else(|| code_detail("model.streamLoadFailed", &paths.encoder))?;
        let vad = paths.vad.as_ref().map(|path| VadModelConfig {
            silero_vad: SileroVadModelConfig {
                model: Some(path.clone()),
                threshold: 0.5,
                min_silence_duration: VAD_MIN_SILENCE,
                min_speech_duration: 0.25,
                window_size: VAD_WINDOW as i32,
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
            .ok_or_else(|| code_detail("model.punctLoadFailed", model))?;
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

fn decode(recognizer: &OfflineRecognizer, samples: &[f32]) -> String {
    let stream = recognizer.create_stream();
    stream.accept_waveform(SAMPLE_RATE, samples);
    recognizer.decode(&stream);
    stream
        .get_result()
        .map(|r| clean(&r.text))
        .unwrap_or_default()
}

// ──────────────────── 识别规则（pre-roll）────────────────────

#[cfg(test)]
mod tests;
