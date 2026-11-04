//! 三个引擎（整段 SenseVoice / 流式 Zipformer / 标点 CT-Transformer）的
//! 构造与加载，加上 AppState 和界面要的 Status。
//!
//! 「模型长什么样」在这里，「拿着模型怎么识别」在 asr.rs。

use std::collections::HashSet;
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
    clean, should_pre_roll, MergedSegment, Segment, SegmentMerger, Transcript,
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
    /// 设置的锁外面包一层 `Arc`：加载引擎的阻塞线程要在**持有引擎锁**的前提下
    /// 读设置（见 `install_engine`），得能把它带进那个线程。
    pub settings: Arc<Mutex<Settings>>,
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
            settings: Arc::new(Mutex::new(settings)),
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
    /// 术语表里模型不认识的条目数（模型没下载时算不出来，按 0 记）
    pub hotwords_ignored: usize,
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
    // 模型词表外的字：这些术语流式用不了（sherpa 会整条跳过）。
    let hotwords_ignored = streaming
        .as_ref()
        .map(|paths| {
            let vocab = streaming_vocab_chars(&paths.tokens);
            split_hotwords_by_vocab(&settings.hotwords, &vocab).1.len()
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
        hotwords_ignored,
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

/// 把加载结果映射成引擎状态事件：成功 ready，失败 error。
///
/// 抽成纯函数是为了能脱开 `AppHandle` 单测 —— 少了 error 事件，
/// 前端徽标会永远停在「加载中」。
fn engine_state_for<T>(outcome: &Result<T, String>) -> EngineState {
    match outcome {
        Ok(_) => EngineState {
            phase: "ready",
            message: None,
        },
        Err(err) => EngineState {
            phase: "error",
            message: Some(err.clone()),
        },
    }
}

/// 整段引擎的身份：它是照着哪些设置建的。
///
/// 引擎照着**当时的**设置构建。设置变了（模型目录 / 语言 / ITN），
/// 旧引擎就过期了 —— 继续用它 = 把新设置静默吞掉。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OfflineEngineKey {
    /// 实际用的模型目录（解析后的真实路径，不是设置里那个原始字符串）
    pub model_dir: String,
    pub language: String,
    pub itn: bool,
}

/// 流式引擎的身份：照着哪份热词表建的。
///
/// 词表是建解码器时定死的（greedy vs modified_beam_search），
/// 换了词表不重建 = 新术语表不生效。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamingEngineKey {
    pub hotwords: Vec<String>,
}

/// 装引擎的骨架：拿槽锁 → 锁内读设置 → 算这次要装的身份 → 比身份 → 复用或重建。
///
/// 「锁内读设置」是关键：设置是在**锁外**被改的（`set_model_dir` /
/// `set_hotwords`），读设置和装引擎之间只要有缝，旧快照就会把新设置顶掉；
/// 而原来的陈旧判定只看语言和 ITN，还不自愈 —— 新设置被静默吞掉。
///
/// 身份必须覆盖**所有**影响识别的设置：整段是模型目录 / 语言 / ITN，
/// 流式是热词。只比语言和 ITN 的话，换了模型目录照样复用旧引擎。
pub fn install_engine<T, K, P>(
    engine_slot: &Mutex<Option<Arc<T>>>,
    settings: &Mutex<Settings>,
    key_of: impl Fn(&T) -> K,
    prepare: impl Fn(&Settings) -> Result<(K, P), String>,
    build: impl FnOnce(P) -> Result<T, String>,
    lock_error: &str,
) -> Result<Arc<T>, String>
where
    K: PartialEq,
{
    let mut guard = engine_slot.lock().map_err(|_| code(lock_error))?;
    // ★ 锁内读设置：设置在锁外被改，这里读到的就是改完之后的那份。
    let current = settings.lock().map(|s| s.clone()).unwrap_or_default();
    let (wanted, prepared) = prepare(&current)?;
    if let Some(existing) = guard.as_ref() {
        if key_of(existing) == wanted {
            return Ok(existing.clone());
        }
    }
    let built = Arc::new(build(prepared)?);
    *guard = Some(built.clone());
    Ok(built)
}

/// 加载（必要时重建）整段引擎，返回可用的引擎句柄。
///
/// 返回句柄而不是让调用方再去取一次：二次取锁既会在 async 执行器上
/// 阻塞（模型加载时那把锁能被占住好几秒），也会出现「load 与取之间
/// 引擎被换掉」的缝。
pub async fn load_engine(app: &AppHandle, state: &AppState) -> Result<Arc<Engine>, String> {
    let handle = app.clone();
    let _ = handle.emit(
        "engine:state",
        EngineState {
            phase: "loading",
            message: None,
        },
    );
    let slot = state.engine.clone();
    let settings = state.settings.clone();
    let build_app = handle.clone();
    let outcome = tauri::async_runtime::spawn_blocking(move || -> Result<Arc<Engine>, String> {
        install_engine(
            &slot,
            &settings,
            |engine: &Engine| engine.key.clone(),
            |s: &Settings| {
                let paths = models::resolve(&build_app, s.model_dir.as_deref())
                    .ok_or_else(|| code("model.needOffline"))?;
                let key = OfflineEngineKey {
                    model_dir: paths.dir.clone(),
                    language: s.language.clone(),
                    itn: s.itn,
                };
                Ok((key.clone(), (paths, key)))
            },
            |(paths, key): (ModelPaths, OfflineEngineKey)| {
                let loaded = Engine::new(&paths, &key.language, key.itn)?;
                log::info(format!(
                    "离线引擎加载完成：语言 {}，ITN {}，模型 {}",
                    key.language,
                    on_off(key.itn),
                    paths.dir
                ));
                Ok(loaded)
            },
            "error.engineState",
        )
    })
    .await
    .map_err(|e| code_detail("model.loadFailed", e))?;

    let _ = handle.emit("engine:state", engine_state_for(&outcome));
    outcome
}

/// 快路径：缓存里已经有引擎就直接拿。
///
/// 用 `try_lock` 而不是 `lock`：锁被占着说明**别的线程正在加载模型**
/// （239 MB / 161 MB / 294 MB，要好几秒）。在 async 执行器线程上死等
/// 会把整个执行器卡住 —— 命令的响应被拖到模型加载完，用户看着像界面冻住。
/// 拿不到就往下走加载路径：它在阻塞线程上等，而且拿到锁后会重新检查
/// 一遍，不会重复加载同一份模型。
pub fn cached_engine<T>(slot: &Mutex<Option<Arc<T>>>) -> Option<Arc<T>> {
    slot.try_lock().ok().and_then(|guard| guard.clone())
}

/// 快路径：缓存里的流式引擎身份与当前设置对得上就直接拿。
///
/// 对不上（刚改过热词）就不能拿 —— 拿了就等于把新设置静默吞掉。
///
/// 用 `try_lock` 而不是 `lock`：锁被占着说明**别的线程正在加载模型**
/// （239 MB / 161 MB / 294 MB，要好几秒）。在 async 执行器线程上死等
/// 会把整个执行器卡住 —— 命令的响应被拖到模型加载完，用户看着像界面冻住。
/// 拿不到就往下走加载路径：它在阻塞线程上等，而且拿到锁后会重新检查
/// 一遍，不会重复加载同一份模型。
pub fn cached_streaming_engine(
    engine_slot: &Mutex<Option<Arc<StreamingEngine>>>,
    settings: &Mutex<Settings>,
) -> Option<Arc<StreamingEngine>> {
    let guard = engine_slot.try_lock().ok()?;
    let engine = guard.as_ref()?.clone();
    let current = settings
        .lock()
        .map(|s| s.hotwords.clone())
        .unwrap_or_default();
    if engine.key.hotwords == current {
        Some(engine)
    } else {
        None
    }
}

pub async fn ensure_streaming_engine(
    app: &AppHandle,
    state: &AppState,
) -> Result<Arc<StreamingEngine>, String> {
    if let Some(engine) = cached_streaming_engine(&state.streaming_engine, &state.settings) {
        return Ok(engine);
    }
    let _ = app.emit(
        "streaming:state",
        EngineState {
            phase: "loading",
            message: None,
        },
    );
    let slot = state.streaming_engine.clone();
    let settings = state.settings.clone();
    let build_app = app.clone();
    // 检查和加载放在同一把锁里：预热和点录音并发时只会加载一份 161 MB 模型。
    let outcome =
        match tauri::async_runtime::spawn_blocking(move || -> Result<Arc<StreamingEngine>, String> {
            install_engine(
                &slot,
                &settings,
                |engine: &StreamingEngine| engine.key.clone(),
                |s: &Settings| {
                    let key = StreamingEngineKey {
                        hotwords: s.hotwords.clone(),
                    };
                    Ok((key.clone(), key))
                },
                |key: StreamingEngineKey| {
                    let paths = models::resolve_streaming(&build_app)
                        .ok_or_else(|| code("model.needStreaming"))?;
                    let engine = StreamingEngine::new(&paths, &key.hotwords)?;
                    log::info(format!(
                        "流式引擎加载完成：热词 {} 条，模型 {}",
                        key.hotwords.len(),
                        paths.dir
                    ));
                    Ok(engine)
                },
                "error.streamEngineState",
            )
        })
        .await
        {
            Ok(inner) => inner,
            Err(e) => Err(code_detail("model.streamLoadFailed", e)),
        };

    // 成功发 ready，失败发 error —— 和整段侧 load_engine 一个范式。
    // 少了 error 事件，前端 state.engine 会永远停在 loading（徽标卡「加载中」）。
    let _ = app.emit("streaming:state", engine_state_for(&outcome));
    outcome
}

pub async fn ensure_punctuation(
    app: &AppHandle,
    state: &AppState,
) -> Result<Arc<PunctuationEngine>, String> {
    if let Some(engine) = cached_engine(&state.punct_engine) {
        return Ok(engine);
    }
    let model = models::resolve_punctuation(app).ok_or_else(|| code("model.punctNotReady"))?;
    // 同上：检查和加载在同一把锁里，避免并发加载两份 294 MB 标点模型。
    let engine_slot = state.punct_engine.clone();
    let engine =
        tauri::async_runtime::spawn_blocking(move || -> Result<Arc<PunctuationEngine>, String> {
            let mut guard = engine_slot.lock().map_err(|_| code("error.punctEngineState"))?;
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

/// 流式门控 VAD 的「一句话最长」阈值（秒）。
///
/// 与整段的 MAX_SEGMENT_SECONDS 不同是**有意的**：流式拿 VAD 只做静音门控
/// （判「现在是不是在说」），20 秒是门控的耐心上限 —— 连续说话到 20 秒时
/// sherpa 会收紧参数逼它找静音，好让端点规则有机会收句。
/// 整段拿 VAD 直接切句，阈值是 30 秒。
///
/// 除此之外，两边的 VAD 参数必须完全一致 —— 见 `vad_config`。
const STREAM_MAX_SPEECH_SECONDS: f32 = 20.0;

/// 两种模式共用的 VAD 配置。
///
/// **整段和流式必须用同一个函数建 VAD 配置**：两份字面量各改各的，
/// 断句行为就会在两个模式里分叉（同一段音频切出不同数量的句子）。
///
/// 唯一允许不同的是 `max_speech_duration` —— 两条路径用它的地方不同，
/// 见 MAX_SEGMENT_SECONDS 与 STREAM_MAX_SPEECH_SECONDS 的注释。
fn vad_config(model: Option<String>, max_speech_duration: f32) -> VadModelConfig {
    VadModelConfig {
        silero_vad: SileroVadModelConfig {
            model,
            threshold: 0.5,
            min_silence_duration: VAD_MIN_SILENCE,
            min_speech_duration: 0.25,
            window_size: VAD_WINDOW as i32,
            max_speech_duration,
        },
        sample_rate: SAMPLE_RATE,
        num_threads: 1,
        provider: Some("cpu".into()),
        debug: false,
        ..Default::default()
    }
}

pub struct Engine {
    /// 这个引擎是照着哪份设置建的（判断能不能复用）。
    pub key: OfflineEngineKey,
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

        let vad = paths
            .vad
            .as_ref()
            .map(|_| vad_config(paths.vad.clone(), MAX_SEGMENT_SECONDS));

        Ok(Self {
            key: OfflineEngineKey {
                model_dir: paths.dir.clone(),
                language: language.to_string(),
                itn,
            },
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
    ///
    /// **这是「整条一次」的批处理路径**（探针、对比测试用），
    /// 跟生产管线的 `asr::OfflineSession` 是两套实现。差在两处，都是有意的：
    ///
    /// * 这里**没有 AGC**：生产路径的电平自动增益在 `record.rs` 里做，
    ///   这一层是纯逻辑，不碰音量。
    /// * 这里**有 30s 窗口兜底**，会话路径没有。会话路径靠 AGC 把偏小的
    ///   录音救回来（`audio::Agc`），一旦再给它加个兜底，
    ///   `quiet_pipeline_probe` 就分不出「AGC 还接着」和「AGC 被摘了」。
    ///
    /// 别把一边的东西顺手搬到另一边：改一边的时候想想另一边为什么没有。
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

        crate::asr::transcript_from(
            &segments,
            duration,
            started.elapsed().as_millis() as u64,
            &self.key.language,
        )
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

/// 流式模型词表里有的字。
///
/// 流式模型是 cjkchar **单字**词表（1426 个汉字）：热词按字编码 ——
/// sherpa 的 `EncodeHotwords` 逐字查符号表，任何一个字不在表里，**整条热词**
/// 就被跳过（sherpa 只会往 stderr 打一句 `Cannot find ID for token`，
/// 界面上看不出为什么没生效）。
pub fn streaming_vocab_chars(tokens_path: &str) -> HashSet<char> {
    let mut chars = HashSet::new();
    let Ok(text) = fs::read_to_string(tokens_path) else {
        return chars;
    };
    for line in text.lines() {
        let symbol = line.split_whitespace().next().unwrap_or("");
        let mut it = symbol.chars();
        // 单字符号才 usable —— `<blk>` / `<0xE7>` 这类多字符的跳过。
        if let (Some(ch), None) = (it.next(), it.next()) {
            if ch != '<' {
                chars.insert(ch);
            }
        }
    }
    chars
}

/// 把热词分成「模型能用」和「模型不认识」两拨。
///
/// 词表读不出来（文件缺失/损坏）时**全部当作能用** —— 宁可放行，
/// 也别因为读不到词表就把用户的术语表清空。
pub fn split_hotwords_by_vocab(
    hotwords: &[String],
    vocab: &HashSet<char>,
) -> (Vec<String>, Vec<String>) {
    if vocab.is_empty() {
        return (hotwords.to_vec(), Vec::new());
    }
    let mut usable = Vec::new();
    let mut ignored = Vec::new();
    for word in hotwords {
        if word.chars().all(|ch| vocab.contains(&ch)) {
            usable.push(word.clone());
        } else {
            ignored.push(word.clone());
        }
    }
    (usable, ignored)
}

impl StreamingEngine {
    /// 加载流式 Zipformer（中英双语，+ 可选的术语表热词）。
    ///
    /// 热词要求 `modified_beam_search`（greedy 不支持 context graph），没配词表时
    /// 保持 greedy，避免给不用热词的人平白加解码延迟。
    pub fn new(paths: &StreamingPaths, hotwords: &[String]) -> Result<Self, String> {
        // 模型不认识的字先剔掉：留着只会让 sherpa 跳过整条热词，
        // 还刷一屏 `Cannot find ID for token` 到 stderr。
        let vocab = streaming_vocab_chars(&paths.tokens);
        let (usable, ignored) = split_hotwords_by_vocab(hotwords, &vocab);
        if !ignored.is_empty() {
            log::warn(format!(
                "术语表里有 {} 条含模型不认识的字，不会生效：{}",
                ignored.len(),
                ignored.join("、")
            ));
        }
        let words = usable.join("\n");
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
        let vad = paths
            .vad
            .as_ref()
            .map(|path| vad_config(Some(path.clone()), STREAM_MAX_SPEECH_SECONDS));
        Ok(Self {
            key: StreamingEngineKey {
                hotwords: hotwords.to_vec(),
            },
            recognizer,
            vad,
        })
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
    /// 这个引擎是照着哪份热词表建的（判断能不能复用）。
    pub key: StreamingEngineKey,
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

/// 测试用的流式引擎：没有 `VOICE_NOTES_STREAMING_MODEL_DIR` 就返回 None，
/// 调用方自己跳过整条用例。
#[cfg(test)]
pub(crate) fn streaming_engine_for_test() -> Option<Arc<StreamingEngine>> {
    let dir = std::env::var("VOICE_NOTES_STREAMING_MODEL_DIR")
        .ok()
        .map(PathBuf::from)?;
    let paths = crate::models::StreamingPaths {
        dir: dir.display().to_string(),
        encoder: dir.join("encoder.int8.onnx").display().to_string(),
        decoder: dir.join("decoder.onnx").display().to_string(),
        joiner: dir.join("joiner.int8.onnx").display().to_string(),
        tokens: dir.join("tokens.txt").display().to_string(),
        vad: [dir.join("silero_vad.onnx")]
            .into_iter()
            .find(|path| path.is_file())
            .map(|path| path.display().to_string()),
    };
    Some(Arc::new(StreamingEngine::new(&paths, &[]).expect("加载流式模型")))
}

// ──────────────────── 识别规则（pre-roll）────────────────────

#[cfg(test)]
mod tests;
