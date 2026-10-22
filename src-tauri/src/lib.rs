//! VoiceNotes —— 本地语音转文字桌面应用（Tauri 2 + SenseVoiceSmall INT8）。

pub mod asr;
pub mod decode;
pub mod audio;
pub mod models;
pub mod settings;

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use sherpa_onnx::LinearResampler;
use tauri::{AppHandle, Emitter, Manager, State};

use asr::{
    Engine, OfflineEvent, PunctuationEngine, StreamEvent, StreamingEngine, StreamingSession, Transcript,
};
use audio::Recorder;
use settings::Settings;

/// 一次流式录音会话：停止标志 + 结果通道。
struct StreamSession {
    stop: Arc<AtomicBool>,
    rx: Receiver<Transcript>,
}

pub struct AppState {
    settings: Mutex<Settings>,
    recorder: Mutex<Option<Recorder>>,
    engine: Arc<Mutex<Option<Arc<Engine>>>>,
    streaming_engine: Arc<Mutex<Option<Arc<StreamingEngine>>>>,
    punct_engine: Arc<Mutex<Option<Arc<PunctuationEngine>>>>,
    stream: Mutex<Option<StreamSession>>,
    offline_session: Mutex<Option<OfflineRun>>,
    cancel_download: Arc<AtomicBool>,
    downloading: Arc<AtomicBool>,
}

impl AppState {
    fn new(settings: Settings) -> Self {
        Self {
            settings: Mutex::new(settings),
            recorder: Mutex::new(None),
            engine: Arc::new(Mutex::new(None)),
            streaming_engine: Arc::new(Mutex::new(None)),
            punct_engine: Arc::new(Mutex::new(None)),
            stream: Mutex::new(None),
            offline_session: Mutex::new(None),
            cancel_download: Arc::new(AtomicBool::new(false)),
            downloading: Arc::new(AtomicBool::new(false)),
        }
    }

    fn settings(&self) -> Settings {
        self.settings.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    model_ready: bool,
    model_source: Option<String>,
    model_dir: Option<String>,
    model_dir_custom: bool,
    vad_ready: bool,
    output_dir: String,
    language: String,
    mode: String,
    itn: bool,
    downloading: bool,
    installed_bytes: u64,
    total_bytes: u64,
    streaming_ready: bool,
    streaming_dir: Option<String>,
    streaming_bytes: u64,
    streaming_total_bytes: u64,
    punct: bool,
    punct_ready: bool,
    input_device: Option<String>,
    punct_bytes: u64,
    punct_total_bytes: u64,
    app_version: String,
}

struct EventSink(AppHandle);

impl models::ProgressSink for EventSink {
    fn report(&self, progress: models::Progress) {
        let _ = self.0.emit("model:progress", progress);
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct EngineState {
    phase: &'static str,
    message: Option<String>,
}

/// 把流式事件转成 UI 事件，并维护已定稿文本。
struct StreamEmitter {
    app: AppHandle,
}

impl StreamEmitter {
    fn apply(&mut self, events: Vec<StreamEvent>) {
        for event in events {
            match event {
                StreamEvent::Partial(text) => {
                    let _ = self.app.emit("stream:partial", PartialPayload { partial: text });
                }
                StreamEvent::Segment(segment) => {
                    let _ = self.app.emit(
                        "stream:segment",
                        SegmentPayload {
                            segment: segment.text,
                            start: segment.start,
                            end: segment.end,
                        },
                    );
                }
            }
        }
    }
}

/// 一句识别好的文本（增量载荷：只带新句子，不带累计全文）。
///
/// 累计全文在长时间会话下是 O(n²) 的传输与重绘：1000 句要传 50 万字符、
/// textarea 也要重写 1000 次。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SegmentPayload {
    segment: String,
    start: f32,
    end: f32,
}

/// 流式的中间结果（未成句的部分）。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PartialPayload {
    partial: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Saved {
    path: String,
    name: String,
}

fn output_dir(app: &AppHandle, settings: &Settings) -> PathBuf {
    settings
        .output_dir
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(|| settings::default_output_dir(app))
}

fn status_of(app: &AppHandle, settings: &Settings) -> Status {
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
        output_dir: output_dir(app, settings).to_string_lossy().into_owned(),
        language: settings.language.clone(),
        mode: settings.mode.clone(),
        itn: settings.itn,
        downloading: false,
        installed_bytes,
        total_bytes,
        streaming_ready: streaming.is_some(),
        streaming_dir: streaming.map(|p| p.dir),
        streaming_bytes,
        streaming_total_bytes: models::streaming_total_bytes(),
        punct: settings.punct,
        punct_ready: models::resolve_punctuation(app).is_some(),
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

fn persist(app: &AppHandle, state: &AppState) -> Result<(), String> {
    let settings = state.settings();
    settings::save(app, &settings)
}

#[tauri::command]
fn status(app: AppHandle, state: State<AppState>) -> Status {
    let mut status = status_of(&app, &state.settings());
    status.downloading = state.downloading.load(Ordering::Relaxed);
    status
}

#[tauri::command]
fn set_language(
    app: AppHandle,
    state: State<AppState>,
    language: String,
) -> Result<Status, String> {
    if !settings::LANGUAGES.contains(&language.as_str()) {
        return Err(format!("不支持的语言: {language}"));
    }
    {
        let mut guard = state
            .settings
            .lock()
            .map_err(|_| "设置状态异常".to_string())?;
        guard.language = language;
    }
    persist(&app, &state)?;
    Ok(status_of(&app, &state.settings()))
}

#[tauri::command]
fn set_itn(app: AppHandle, state: State<AppState>, enabled: bool) -> Result<Status, String> {
    {
        let mut guard = state
            .settings
            .lock()
            .map_err(|_| "设置状态异常".to_string())?;
        guard.itn = enabled;
    }
    persist(&app, &state)?;
    Ok(status_of(&app, &state.settings()))
}

#[tauri::command]
fn set_output_dir(
    app: AppHandle,
    state: State<AppState>,
    dir: Option<String>,
) -> Result<Status, String> {
    {
        let mut guard = state
            .settings
            .lock()
            .map_err(|_| "设置状态异常".to_string())?;
        guard.output_dir = dir.filter(|d| !d.trim().is_empty());
    }
    if let Some(dir) = state.settings().output_dir {
        fs::create_dir_all(&dir).map_err(|e| format!("无法创建目录 {dir}: {e}"))?;
    }
    persist(&app, &state)?;
    Ok(status_of(&app, &state.settings()))
}

#[tauri::command]
fn set_model_dir(
    app: AppHandle,
    state: State<AppState>,
    dir: Option<String>,
) -> Result<Status, String> {
    {
        let mut guard = state
            .settings
            .lock()
            .map_err(|_| "设置状态异常".to_string())?;
        guard.model_dir = dir.filter(|d| !d.trim().is_empty());
    }
    persist(&app, &state)?;
    // 模型换了，旧引擎作废。
    if let Ok(mut engine) = state.engine.lock() {
        *engine = None;
    }
    Ok(status_of(&app, &state.settings()))
}

async fn load_engine(app: &AppHandle, state: &AppState) -> Result<(), String> {
    let settings = state.settings();
    let paths = models::resolve(app, settings.model_dir.as_deref())
        .ok_or_else(|| "模型未就绪，请先下载语音模型".to_string())?;
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
        let mut guard = engine.lock().map_err(|_| "引擎状态异常".to_string())?;
        let stale = guard
            .as_ref()
            .map(|e| e.language != language || e.itn != itn)
            .unwrap_or(true);
        if stale {
            *guard = Some(Arc::new(Engine::new(&paths, &language, itn)?));
        }
        Ok(())
    })
    .await
    .map_err(|e| format!("加载模型失败: {e}"))?;

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

async fn ensure_streaming_engine(
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
    let paths = models::resolve_streaming(app)
        .ok_or_else(|| "流式模型未就绪，请先下载流式模型".to_string())?;
    let _ = app.emit(
        "streaming:state",
        EngineState {
            phase: "loading",
            message: None,
        },
    );
    let engine = tauri::async_runtime::spawn_blocking(move || StreamingEngine::new(&paths))
        .await
        .map_err(|e| format!("加载流式模型失败: {e}"))??;
    let engine = Arc::new(engine);
    if let Ok(mut slot) = state.streaming_engine.lock() {
        *slot = Some(engine.clone());
    }
    let _ = app.emit(
        "streaming:state",
        EngineState {
            phase: "ready",
            message: None,
        },
    );
    Ok(engine)
}

async fn ensure_punctuation(
    app: &AppHandle,
    state: &AppState,
) -> Result<Arc<PunctuationEngine>, String> {
    if let Some(engine) = state.punct_engine.lock().ok().and_then(|slot| slot.clone()) {
        return Ok(engine);
    }
    let model = models::resolve_punctuation(app).ok_or_else(|| "标点模型未下载".to_string())?;
    let engine = tauri::async_runtime::spawn_blocking(move || PunctuationEngine::new(&model))
        .await
        .map_err(|e| format!("加载标点模型失败: {e}"))??;
    let engine = Arc::new(engine);
    if let Ok(mut slot) = state.punct_engine.lock() {
        *slot = Some(engine.clone());
    }
    Ok(engine)
}

/// 取走新到的采样（消费式：取走即释放，内存不随录音时长增长）。
fn take_new(queue: &audio::SampleQueue) -> audio::NewAudio {
    queue.take_new()
}

/// 整段模式的识别线程。
struct OfflineRun {
    stop: Arc<AtomicBool>,
    rx: mpsc::Receiver<Transcript>,
}

/// 整段模式：边录音边按句识别。
///
/// 采集线程把新采样放进队列，这里取走、重采样到 16k、喂给 VAD 切句，
/// 切出一句就立刻送 SenseVoice 识别并把结果推给界面。
/// 识别完的音频立刻丢掉，只留文本 —— 内存不随录音时长增长。
fn run_offline<F>(
    engine: Arc<Engine>,
    queue: audio::SampleQueue,
    rate: u32,
    stop: Arc<AtomicBool>,
    mut emit: F,
) -> Transcript
where
    F: FnMut(OfflineEvent),
{
    let resampler = if rate as i32 == asr::SAMPLE_RATE {
        None
    } else {
        sherpa_onnx::LinearResampler::create(rate as i32, asr::SAMPLE_RATE)
    };
    let mut session = match engine.vad_config() {
        Some(vad) => Some(asr::OfflineSession::new(engine.clone(), vad)),
        None => None,
    };
    let mut parts: Vec<String> = Vec::new();
    // 已经喂给识别器的采样数（用来给缺口标记标时间）
    let mut fed_seconds = 0usize;

    loop {
        let new_audio = queue.take_new();
        if new_audio.dropped > 0 {
            // 识别跟不上：丢掉了一段音频，要在结果里留下可见的缺口。
            // 标记既进最终文本，也作为一句实时发出去（前端追加后才看得到）。
            let marker = audio::gap_marker(new_audio.dropped, rate);
            parts.push(marker.clone());
            let at = fed_seconds as f32 / asr::SAMPLE_RATE as f32;
            emit(OfflineEvent::Segment(asr::Segment {
                text: marker,
                start: at,
                end: at,
            }));
        }
        fed_seconds += new_audio.samples.len();
        if !new_audio.samples.is_empty() {
            let samples = match &resampler {
                Some(resampler) => resampler.resample(&new_audio.samples, false),
                None => new_audio.samples,
            };
            if let Some(session) = session.as_mut() {
                for event in session.push(&samples) {
                    let OfflineEvent::Segment(segment) = event;
                    parts.push(segment.text.clone());
                    emit(OfflineEvent::Segment(segment));
                }
            }
        }
        if stop.load(Ordering::Relaxed) {
            break;
        }
        std::thread::sleep(Duration::from_millis(60));
    }

    // 收尾：把 VAD 里还没成句的剩余音频冲出来
    if let Some(session) = session.as_mut() {
        for event in session.finish() {
            let OfflineEvent::Segment(segment) = event;
            parts.push(segment.text.clone());
            emit(OfflineEvent::Segment(segment));
        }
    }

    // 文本用 parts 拼（含缺口标记）；segments 只放真正的句子
    let (text, segments, elapsed_ms, duration_seconds) = match &session {
        Some(session) => (
            asr::join_segments(parts.iter().map(|s| s.as_str())),
            session.segments().to_vec(),
            session.elapsed_ms(),
            session.recorded_samples() as f32 / asr::SAMPLE_RATE as f32,
        ),
        None => (asr::join_segments(parts.iter().map(|s| s.as_str())), Vec::new(), 0, 0.0),
    };

    Transcript {
        text,
        segments,
        duration_seconds,
        elapsed_ms,
        language: engine.language.clone(),
    }
}

/// 边录音边喂流式识别器；每 60ms 取一次新采样，端点处提交一句。
fn run_streaming(
    app: &AppHandle,
    engine: Arc<StreamingEngine>,
    punctuator: Option<Arc<PunctuationEngine>>,
    queue: audio::SampleQueue,
    rate: u32,
    stop: Arc<AtomicBool>,
) -> Transcript {
    let mut emitter = StreamEmitter { app: app.clone() };
    run_streaming_core(engine, punctuator, queue, rate, stop, |events| {
        emitter.apply(events)
    })
}

fn run_streaming_core<F>(
    engine: Arc<StreamingEngine>,
    punctuator: Option<Arc<PunctuationEngine>>,
    queue: audio::SampleQueue,
    rate: u32,
    stop: Arc<AtomicBool>,
    mut emit: F,
) -> Transcript
where
    F: FnMut(Vec<StreamEvent>),
{
    let vad_config = engine.vad().cloned();
    let mut session = StreamingSession::new(engine, punctuator);
    let resampler = if rate as i32 == asr::SAMPLE_RATE {
        None
    } else {
        LinearResampler::create(rate as i32, asr::SAMPLE_RATE)
    };
    let vad = vad_config
        .as_ref()
        .and_then(|config| sherpa_onnx::VoiceActivityDetector::create(config, 60.0));
    // 说话起点前留 0.4s，避免 VAD 起音延迟把字头切掉
    let preroll_limit = (asr::SAMPLE_RATE as usize) / 5 * 2;
    let mut preroll: Vec<f32> = Vec::new();
    let mut active = false;
    // 已经喂给识别器的采样数（用来给缺口标记标时间）
    let mut fed_seconds = 0usize;
    // 识别跟不上时丢掉的音频，在结果里留个可见的缺口
    let mut gaps: Vec<asr::Segment> = Vec::new();
    let mut last_voice = std::time::Instant::now();
    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(60));
        let new_audio = take_new(&queue);
        if new_audio.dropped > 0 {
            // 识别跟不上：丢掉了一段音频，标记出来，不能静默丢
            let marker = audio::gap_marker(new_audio.dropped, rate);
            let at = fed_seconds as f32 / asr::SAMPLE_RATE as f32;
            gaps.push(asr::Segment {
                text: marker.clone(),
                start: at,
                end: at,
            });
            emit(vec![StreamEvent::Segment(asr::Segment {
                text: marker,
                start: at,
                end: at,
            })]);
        }
        fed_seconds += new_audio.samples.len();
        let chunk = new_audio.samples;
        if chunk.is_empty() {
            continue;
        }
        let samples = match &resampler {
            Some(resampler) => resampler.resample(&chunk, false),
            None => chunk,
        };
        if samples.is_empty() {
            continue;
        }

        let Some(detector) = vad.as_ref() else {
            emit(session.push(&samples));
            continue;
        };

        for window in samples.chunks(512) {
            detector.accept_waveform(window);
        }
        let detected = detector.detected();
        if detected {
            last_voice = std::time::Instant::now();
        }
        let speaking = detected || last_voice.elapsed() < Duration::from_millis(700);

        if speaking {
            if !active {
                active = true;
                if !preroll.is_empty() {
                    emit(session.push(&std::mem::take(&mut preroll)));
                }
            }
            emit(session.push(&samples));
        } else {
            if active {
                active = false;
                // 静音开始：立刻收尾当前句子，避免模型在噪声上继续编。
                emit(session.force_end());
            }
            preroll.extend_from_slice(&samples);
            let excess = preroll.len().saturating_sub(preroll_limit);
            if excess > 0 {
                preroll.drain(..excess);
            }
        }
    }

    // 收尾：把剩余采样和重采样器内部缓冲一起冲出去
    let last = take_new(&queue);
    if last.dropped > 0 {
        let marker = audio::gap_marker(last.dropped, rate);
        let at = fed_seconds as f32 / asr::SAMPLE_RATE as f32;
        gaps.push(asr::Segment {
            text: marker.clone(),
            start: at,
            end: at,
        });
        emit(vec![StreamEvent::Segment(asr::Segment {
            text: marker,
            start: at,
            end: at,
        })]);
    }
    let chunk = last.samples;
    let tail = match &resampler {
        Some(resampler) => resampler.resample(&chunk, true),
        None => chunk,
    };
    if !tail.is_empty() {
        emit(session.push(&tail));
    }
    emit(session.finish());
    let mut transcript = session.transcript();
    if !gaps.is_empty() {
        // 缺口标记按时间插进句子列表，文本重拼一次
        transcript.segments.extend(gaps);
        transcript.segments.sort_by(|a, b| {
            a.start
                .partial_cmp(&b.start)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        transcript.text = asr::join_segments(transcript.segments.iter().map(|s| s.text.as_str()));
    }
    transcript
}

#[tauri::command]
async fn warm_up(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    load_engine(&app, &state).await
}

#[tauri::command]
async fn warm_up_streaming(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    ensure_streaming_engine(&app, &state).await.map(|_| ())
}

#[tauri::command]
fn set_mode(app: AppHandle, state: State<AppState>, mode: String) -> Result<Status, String> {
    if !settings::MODES.contains(&mode.as_str()) {
        return Err(format!("不支持的模式: {mode}"));
    }
    {
        let mut guard = state
            .settings
            .lock()
            .map_err(|_| "设置状态异常".to_string())?;
        guard.mode = mode;
    }
    persist(&app, &state)?;
    Ok(status_of(&app, &state.settings()))
}

#[tauri::command]
fn set_punct(app: AppHandle, state: State<AppState>, enabled: bool) -> Result<Status, String> {
    {
        let mut guard = state
            .settings
            .lock()
            .map_err(|_| "设置状态异常".to_string())?;
        guard.punct = enabled;
    }
    if !enabled {
        if let Ok(mut slot) = state.punct_engine.lock() {
            *slot = None;
        }
    }
    persist(&app, &state)?;
    Ok(status_of(&app, &state.settings()))
}

/// 列出系统里的输入设备（麦克风）。
/// 预热麦克风：提前触发系统权限弹窗（界面渲染完之后再调用）。
#[tauri::command]
async fn warm_up_mic(state: State<'_, AppState>) -> Result<(), String> {
    let device = state.settings().input_device;
    tauri::async_runtime::spawn_blocking(move || audio::probe_input(device.as_deref()))
        .await
        .map_err(|e| format!("预热麦克风失败: {e}"))?
}

#[tauri::command]
fn list_input_devices() -> Vec<audio::InputDevice> {
    audio::list_input_devices()
}

/// 记住要用的输入设备（None = 系统默认）。
#[tauri::command]
fn set_input_device(
    app: AppHandle,
    state: State<AppState>,
    name: Option<String>,
) -> Result<Status, String> {
    {
        let mut guard = state
            .settings
            .lock()
            .map_err(|_| "设置状态异常".to_string())?;
        guard.input_device = name.filter(|value| !value.trim().is_empty());
    }
    persist(&app, &state)?;
    Ok(status_of(&app, &state.settings()))
}


#[tauri::command]
async fn download_punctuation_model(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Status, String> {
    if state.downloading.swap(true, Ordering::SeqCst) {
        return Err("正在下载中".into());
    }
    state.cancel_download.store(false, Ordering::SeqCst);
    let dir = models::punctuation_install_dir(&app)?;
    let sink = EventSink(app.clone());
    let result = models::download_punctuation(&dir, state.cancel_download.clone(), &sink).await;
    state.downloading.store(false, Ordering::SeqCst);
    result?;
    if state.settings().punct {
        let _ = ensure_punctuation(&app, &state).await;
    }
    Ok(status_of(&app, &state.settings()))
}

#[tauri::command]
async fn download_streaming_model(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Status, String> {
    if state.downloading.swap(true, Ordering::SeqCst) {
        return Err("正在下载中".into());
    }
    state.cancel_download.store(false, Ordering::SeqCst);
    let dir = models::streaming_install_dir(&app)?;
    let sink = EventSink(app.clone());
    let result = models::download_streaming(&dir, state.cancel_download.clone(), &sink).await;
    state.downloading.store(false, Ordering::SeqCst);
    result?;
    let _ = ensure_streaming_engine(&app, &state).await;
    Ok(status_of(&app, &state.settings()))
}

#[tauri::command]
async fn start_recording(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    {
        let slot = state
            .recorder
            .lock()
            .map_err(|_| "录音状态异常".to_string())?;
        if slot.is_some() {
            return Err("已经在录音了".into());
        }
    }
    let settings = state.settings();
    if settings.mode == "streaming" {
        if models::resolve_streaming(&app).is_none() {
            return Err("流式模型未就绪，请先下载流式模型".into());
        }
        let engine = ensure_streaming_engine(&app, &state).await?;
        let punctuator = if settings.punct {
            ensure_punctuation(&app, &state).await.ok()
        } else {
            None
        };
        let recorder = audio::start(app.clone(), settings.input_device.as_deref())?;
        let queue = recorder.queue();
        let rate = recorder.sample_rate();
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let thread_app = app.clone();
        let thread_stop = stop.clone();
        std::thread::Builder::new()
            .name("streaming-asr".into())
            .spawn(move || {
                let transcript =
                    run_streaming(&thread_app, engine, punctuator, queue, rate, thread_stop);
                let _ = tx.send(transcript);
            })
            .map_err(|e| format!("无法启动流式识别线程: {e}"))?;
        *state
            .recorder
            .lock()
            .map_err(|_| "录音状态异常".to_string())? = Some(recorder);
        *state
            .stream
            .lock()
            .map_err(|_| "流式状态异常".to_string())? = Some(StreamSession { stop, rx });
        return Ok(());
    }

    if models::resolve(&app, settings.model_dir.as_deref()).is_none() {
        return Err("模型未就绪，请先下载语音模型".into());
    }
    // 整段模式：边录边按句识别（音频消费掉即释放，内存不随录音时长增长）
    load_engine(&app, &state).await?;
    let engine = shared_engine(&state)?;
    let recorder = audio::start(app.clone(), settings.input_device.as_deref())?;
    let queue = recorder.queue();
    let rate = recorder.sample_rate();
    let stop = Arc::new(AtomicBool::new(false));
    let (tx, rx) = std::sync::mpsc::channel();
    let thread_app = app.clone();
    let thread_stop = stop.clone();
    std::thread::Builder::new()
        .name("offline-asr".into())
        .spawn(move || {
            let transcript = run_offline(engine, queue, rate, thread_stop, |event| {
                let OfflineEvent::Segment(segment) = event;
                let _ = thread_app.emit(
                    "stream:segment",
                    SegmentPayload {
                        segment: segment.text,
                        start: segment.start,
                        end: segment.end,
                    },
                );
            });
            let _ = tx.send(transcript);
        })
        .map_err(|e| format!("无法启动整段识别线程: {e}"))?;
    *state
        .recorder
        .lock()
        .map_err(|_| "录音状态异常".to_string())? = Some(recorder);
    *state
        .offline_session
        .lock()
        .map_err(|_| "录音状态异常".to_string())? = Some(OfflineRun { stop, rx });
    Ok(())
}

#[tauri::command]
async fn stop_recording(_app: AppHandle, state: State<'_, AppState>) -> Result<Transcript, String> {
    let recorder = {
        let mut slot = state
            .recorder
            .lock()
            .map_err(|_| "录音状态异常".to_string())?;
        slot.take()
    }
    .ok_or_else(|| "当前没有在录音".to_string())?;
    let stream = {
        let mut slot = state
            .stream
            .lock()
            .map_err(|_| "流式状态异常".to_string())?;
        slot.take()
    };

    if let Some(stream) = stream {
        // 先停采集（识别线程可能还有没取走的尾巴），无论成功与否都要让流式线程收尾。
        let _ = recorder.stop()?;
        stream.stop.store(true, Ordering::SeqCst);
        let streaming = stream
            .rx
            .recv_timeout(Duration::from_secs(20))
            .map_err(|_| "流式识别收尾超时".to_string())?;
        return Ok(streaming);
    }

    // 整段模式：识别线程一直在跑，这里停采集、让它收尾
    let offline = state
        .offline_session
        .lock()
        .map_err(|_| "录音状态异常".to_string())?
        .take();
    let _ = recorder.stop()?;
    let Some(offline) = offline else {
        return Err("识别线程已退出".into());
    };
    offline.stop.store(true, Ordering::SeqCst);
    let transcript = offline
        .rx
        .recv_timeout(Duration::from_secs(30))
        .map_err(|_| "整段识别收尾超时".to_string())?;
    if transcript.duration_seconds < 0.2 {
        return Err("录音太短，至少说 0.2 秒".into());
    }
    if transcript.text.trim().is_empty() {
        return Err("没有识别到内容，换个安静点的环境再试试".into());
    }
    Ok(transcript)
}




/// 转写一个音频 / 视频文件。
///
/// 解码在后端做（symphonia），逐块喂给整段会话：内存恒定，1 小时的文件也不会
/// 整条进内存。格式支持 mp3 / wav / flac / ogg / m4a / mp4 / mov（视频轨忽略）。
#[tauri::command]
async fn transcribe_file(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
) -> Result<Transcript, String> {
    load_engine(&app, &state).await?;
    let engine = shared_engine(&state)?;
    let thread_app = app.clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<Transcript, String> {
        let vad = engine
            .vad_config()
            .ok_or("缺少 VAD 模型，无法断句")?;
        let mut session = asr::OfflineSession::new(engine.clone(), vad);

        let emit_segment = |segment: &asr::Segment| {
            let _ = thread_app.emit(
                "stream:segment",
                SegmentPayload {
                    segment: segment.text.clone(),
                    start: segment.start,
                    end: segment.end,
                },
            );
        };

        decode::decode_to_16k(std::path::Path::new(&path), |chunk| {
            for event in session.push(chunk) {
                let asr::OfflineEvent::Segment(segment) = event;
                emit_segment(&segment);
            }
            Ok(())
        })?;

        for event in session.finish() {
            let asr::OfflineEvent::Segment(segment) = event;
            emit_segment(&segment);
        }

        Ok(Transcript {
            text: asr::join_segments(session.segments().iter().map(|s| s.text.as_str())),
            segments: session.segments().to_vec(),
            duration_seconds: session.recorded_samples() as f32 / asr::SAMPLE_RATE as f32,
            elapsed_ms: session.elapsed_ms(),
            language: engine.language.clone(),
        })
    })
    .await
    .map_err(|e| format!("转写失败: {e}"))?
}

/// 从缓存里拿一个可共享的整段引擎（录音线程和文件转写共用同一份模型）。
fn shared_engine(state: &AppState) -> Result<Arc<Engine>, String> {
    let guard = state.engine.lock().map_err(|_| "引擎状态异常".to_string())?;
    guard
        .as_ref()
        .cloned()
        .ok_or_else(|| "引擎未初始化".to_string())
}

#[tauri::command]
fn cancel_recording(state: State<AppState>) -> Result<(), String> {
    let recorder = {
        let mut slot = state
            .recorder
            .lock()
            .map_err(|_| "录音状态异常".to_string())?;
        slot.take()
    };
    if let Some(stream) = state.stream.lock().ok().and_then(|mut slot| slot.take()) {
        stream.stop.store(true, Ordering::SeqCst);
    }
    // 整段模式的识别线程也要停，否则它会一直空转（队列空了也每 60ms 醒一次）
    if let Some(offline) = state
        .offline_session
        .lock()
        .ok()
        .and_then(|mut slot| slot.take())
    {
        offline.stop.store(true, Ordering::SeqCst);
    }
    if let Some(recorder) = recorder {
        recorder.cancel();
    }
    Ok(())
}

#[tauri::command]
async fn download_model(app: AppHandle, state: State<'_, AppState>) -> Result<Status, String> {
    if state.downloading.swap(true, Ordering::SeqCst) {
        return Err("正在下载中".into());
    }
    state.cancel_download.store(false, Ordering::SeqCst);
    let dir = models::install_dir(&app)?;
    let sink = EventSink(app.clone());
    let result = models::download_all(&dir, state.cancel_download.clone(), &sink).await;
    state.downloading.store(false, Ordering::SeqCst);
    result?;
    let _ = load_engine(&app, &state).await;
    Ok(status_of(&app, &state.settings()))
}

#[tauri::command]
fn cancel_download(state: State<AppState>) {
    state.cancel_download.store(true, Ordering::SeqCst);
}

fn sanitize_name(input: &str) -> Option<String> {
    let cleaned: String = input
        .chars()
        .map(|ch| match ch {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\n' | '\r' | '\t' => ' ',
            _ => ch,
        })
        .collect();
    let cleaned = cleaned.trim().trim_matches('.').trim();
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned.chars().take(80).collect())
    }
}

/// 把文本写进目录；同名文件依次加序号，绝不覆盖。
pub fn write_text_file(
    dir: &std::path::Path,
    requested: &str,
    text: &str,
) -> Result<(PathBuf, String), String> {
    let base = sanitize_name(requested).unwrap_or_else(|| "转写".to_string());
    let name = if base.to_lowercase().ends_with(".txt") {
        base
    } else {
        format!("{base}.txt")
    };
    let stem = name.trim_end_matches(".txt").to_string();
    let mut path = dir.join(&name);
    let mut index = 2;
    while path.exists() {
        path = dir.join(format!("{stem} ({index}).txt"));
        index += 1;
    }
    fs::write(&path, text).map_err(|e| format!("写入失败: {e}"))?;
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| name.clone());
    Ok((path, file_name))
}

#[tauri::command]
fn save_text(
    app: AppHandle,
    state: State<AppState>,
    text: String,
    file_name: Option<String>,
) -> Result<Saved, String> {
    if text.trim().is_empty() {
        return Err("没有可保存的文字".into());
    }
    let settings = state.settings();
    let dir = output_dir(&app, &settings);
    fs::create_dir_all(&dir).map_err(|e| format!("无法创建输出目录 {}: {e}", dir.display()))?;

    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let requested = file_name
        .as_deref()
        .and_then(sanitize_name)
        .unwrap_or_else(|| format!("转写-{stamp}"));
    let (path, name) = write_text_file(&dir, &requested, &text)?;
    Ok(Saved {
        path: path.to_string_lossy().into_owned(),
        name,
    })
}

#[cfg(target_os = "macos")]
fn install_menu(app: &mut tauri::App) -> tauri::Result<()> {
    use tauri::menu::{MenuBuilder, PredefinedMenuItem, SubmenuBuilder};

    let handle = app.handle();
    let app_menu = SubmenuBuilder::new(handle, "VoiceNotes")
        .item(&PredefinedMenuItem::about(handle, None, None)?)
        .separator()
        .item(&PredefinedMenuItem::hide(handle, None)?)
        .item(&PredefinedMenuItem::hide_others(handle, None)?)
        .item(&PredefinedMenuItem::show_all(handle, None)?)
        .separator()
        .item(&PredefinedMenuItem::quit(handle, None)?)
        .build()?;
    let edit_menu = SubmenuBuilder::new(handle, "编辑")
        .item(&PredefinedMenuItem::undo(handle, None)?)
        .item(&PredefinedMenuItem::redo(handle, None)?)
        .separator()
        .item(&PredefinedMenuItem::cut(handle, None)?)
        .item(&PredefinedMenuItem::copy(handle, None)?)
        .item(&PredefinedMenuItem::paste(handle, None)?)
        .item(&PredefinedMenuItem::select_all(handle, None)?)
        .build()?;
    let view_menu = SubmenuBuilder::new(handle, "视图")
        .item(&PredefinedMenuItem::fullscreen(handle, None)?)
        .build()?;
    let menu = MenuBuilder::new(handle)
        .items(&[&app_menu, &edit_menu, &view_menu])
        .build()?;
    app.set_menu(menu)?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let loaded = settings::load(app.handle());
            app.manage(AppState::new(loaded));
            #[cfg(target_os = "macos")]
            install_menu(app)?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            status,
            set_language,
            set_itn,
            set_output_dir,
            set_model_dir,
            warm_up,
            warm_up_streaming,
            set_mode,
            set_punct,
            list_input_devices,
            warm_up_mic,
            set_input_device,
            start_recording,
            stop_recording,
            cancel_recording,
            transcribe_file,
            download_model,
            download_streaming_model,
            download_punctuation_model,
            cancel_download,
            save_text,
        ])
        .run(tauri::generate_context!())
        .expect("error while running voice-notes");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_dangerous_file_names() {
        assert_eq!(sanitize_name("../../etc/passwd").is_some(), true);
        assert!(sanitize_name("/tmp/x").unwrap().contains("tmp"));
        assert_eq!(sanitize_name("   "), None);
        assert!(sanitize_name("会议记录: 周一").is_some());
    }

    #[test]
    fn writes_unique_files_without_overwriting() {
        let dir = std::env::temp_dir().join(format!("vn-save-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let (first, name1) = write_text_file(&dir, "会议记录", "一").unwrap();
        let (second, name2) = write_text_file(&dir, "会议记录", "二").unwrap();

        assert_eq!(name1, "会议记录.txt");
        assert_eq!(name2, "会议记录 (2).txt");
        assert_eq!(fs::read_to_string(&first).unwrap(), "一");
        assert_eq!(fs::read_to_string(&second).unwrap(), "二");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn streams_from_buffer_with_resampling() {
        let Some(dir) = std::env::var("VOICE_NOTES_STREAMING_MODEL_DIR")
            .ok()
            .map(PathBuf::from)
        else {
            eprintln!("跳过：未设置 VOICE_NOTES_STREAMING_MODEL_DIR");
            return;
        };
        let paths = models::StreamingPaths {
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
        let engine = Arc::new(StreamingEngine::new(&paths).expect("加载流式模型"));
        let wave = sherpa_onnx::Wave::read("tests/fixtures/zh.wav").expect("读取测试音频");

        // 模拟 48 kHz 设备：每个 16 kHz 样本重复三次。
        let mut upsampled = Vec::with_capacity(wave.samples().len() * 3);
        for sample in wave.samples() {
            upsampled.push(*sample);
            upsampled.push(*sample);
            upsampled.push(*sample);
        }
        let queue = audio::SampleQueue::new(48_000);
        queue.push(&upsampled);
        let stop = Arc::new(AtomicBool::new(false));
        let stopper = stop.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(400));
            stopper.store(true, Ordering::SeqCst);
        });

        let mut partials = 0usize;
        let transcript = run_streaming_core(engine, None, queue, 48_000, stop, |events| {
            for event in events {
                if let StreamEvent::Partial(_) = event {
                    partials += 1;
                }
            }
        });
        println!("流式结果：{}", transcript.text);
        assert!(!transcript.text.is_empty(), "流式结果为空");
        assert!(partials > 0, "没有中间结果");
        assert!(transcript.duration_seconds > 5.0);
    }

    /// 识别跟不上、丢掉一段音频时，结果里必须留一个可见的缺口标记。
    ///
    /// 回归测试：之前流式路径完全忽略了 `dropped`，音频被静默丢弃。
    #[test]
    fn streaming_gap_marker_reaches_transcript() {
        let Some(dir) = std::env::var("VOICE_NOTES_STREAMING_MODEL_DIR")
            .ok()
            .map(PathBuf::from)
        else {
            eprintln!("跳过：未设置 VOICE_NOTES_STREAMING_MODEL_DIR");
            return;
        };
        let paths = models::StreamingPaths {
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
        let engine = Arc::new(StreamingEngine::new(&paths).expect("加载流式模型"));
        let wave = sherpa_onnx::Wave::read("tests/fixtures/zh.wav").expect("读取测试音频");
        let mut upsampled = Vec::with_capacity(wave.samples().len() * 3);
        for sample in wave.samples() {
            upsampled.push(*sample);
            upsampled.push(*sample);
            upsampled.push(*sample);
        }
        // 上限调到 1000 个采样：音频远超上限 → 一定会触发丢弃
        let queue = audio::SampleQueue::with_limit(48_000, Some(1_000));
        queue.push(&upsampled);
        let stop = Arc::new(AtomicBool::new(false));
        let stopper = stop.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(400));
            stopper.store(true, Ordering::SeqCst);
        });

        let transcript = run_streaming_core(engine, None, queue, 48_000, stop, |_| {});
        println!("带缺口的流式结果：{}", transcript.text);
        assert!(
            transcript.text.contains("未识别"),
            "丢掉音频后结果里应该有缺口标记: {}",
            transcript.text
        );
        assert!(
            transcript.segments.iter().any(|s| s.text.contains("未识别")),
            "segments 里也该有缺口标记"
        );
    }

    /// 整段模式：丢掉音频后，缺口标记必须出现在最终文本里。
    ///
    /// 回归测试：之前标记只 push 进了本地 parts，最终文本却从 session.segments()
    /// 拼，标记被丢掉了。
    #[test]
    fn offline_gap_marker_reaches_transcript() {
        let Some(dir) = std::env::var("VOICE_NOTES_MODEL_DIR")
            .ok()
            .map(PathBuf::from)
        else {
            eprintln!("跳过：未设置 VOICE_NOTES_MODEL_DIR");
            return;
        };
        let model = [dir.join("model.int8.onnx"), dir.join("model.onnx")]
            .into_iter()
            .find(|path| path.is_file());
        let (Some(model), tokens) = (model, dir.join("tokens.txt")) else {
            eprintln!("跳过：找不到 SenseVoice 模型");
            return;
        };
        if !tokens.is_file() {
            eprintln!("跳过：找不到 tokens.txt");
            return;
        }
        let mut vad_candidates = vec![dir.join("silero_vad.onnx")];
        if let Some(parent) = dir.parent() {
            vad_candidates.push(parent.join("silero_vad.onnx"));
        }
        let vad_model = vad_candidates.into_iter().find(|path| path.is_file());
        let Some(vad_model) = vad_model else {
            eprintln!("跳过：找不到 VAD 模型");
            return;
        };
        let paths = models::ModelPaths {
            dir: dir.display().to_string(),
            model: model.display().to_string(),
            tokens: tokens.display().to_string(),
            vad: Some(vad_model.display().to_string()),
            source: "test".into(),
        };
        let engine = Arc::new(Engine::new(&paths, "zh", true).expect("加载模型"));
        let wave = sherpa_onnx::Wave::read("tests/fixtures/zh.wav").expect("读取测试音频");

        // 上限调到 1000 个采样：音频远超上限 → 一定会触发丢弃
        let queue = audio::SampleQueue::with_limit(16_000, Some(1_000));
        queue.push(wave.samples());
        let stop = Arc::new(AtomicBool::new(true));

        let mut events = Vec::new();
        let transcript = run_offline(engine, queue, 16_000, stop, |event| events.push(event));
        println!("带缺口的整段结果：{}", transcript.text);
        assert!(
            transcript.text.contains("未识别"),
            "丢掉音频后文本里应该有缺口标记: {}",
            transcript.text
        );
        assert!(
            events.iter().any(|e| match e {
                OfflineEvent::Segment(s) => s.text.contains("未识别"),
            }),
            "也应该实时发一个缺口事件"
        );
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
        let engine = Arc::new(StreamingEngine::new(&paths).expect("加载流式模型"));
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

    #[test]
    fn settings_round_trip() {
        let raw = r#"{"language":"zh","itn":false,"output_dir":"/tmp/out","model_dir":null}"#;
        let settings: Settings = serde_json::from_str(raw).unwrap();
        assert_eq!(settings.language, "zh");
        assert!(!settings.itn);
        let normalized = Settings {
            language: "klingon".into(),
            ..settings
        }
        .normalize();
        assert_eq!(normalized.language, "auto");
    }
}
