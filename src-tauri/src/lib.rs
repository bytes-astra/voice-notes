//! VoiceNotes —— 本地语音转文字桌面应用（Tauri 2 + SenseVoiceSmall INT8）。

pub mod asr;
pub mod audio;
pub mod models;
pub mod settings;
pub mod wav;

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use sherpa_onnx::LinearResampler;
use tauri::{AppHandle, Emitter, Manager, State};

use asr::{Engine, StreamEvent, StreamingEngine, StreamingSession, Transcript};
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
    engine: Arc<Mutex<Option<Engine>>>,
    streaming_engine: Arc<Mutex<Option<Arc<StreamingEngine>>>>,
    stream: Mutex<Option<StreamSession>>,
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
            stream: Mutex::new(None),
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

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct StreamPayload {
    partial: String,
    committed: String,
}

/// 把流式事件转成 UI 事件，并维护已定稿文本。
struct StreamEmitter {
    app: AppHandle,
    parts: Vec<String>,
    committed: String,
}

impl StreamEmitter {
    fn apply(&mut self, events: Vec<StreamEvent>) {
        for event in events {
            match event {
                StreamEvent::Partial(text) => {
                    let _ = self.app.emit(
                        "stream:partial",
                        StreamPayload {
                            partial: text,
                            committed: self.committed.clone(),
                        },
                    );
                }
                StreamEvent::Segment(segment) => {
                    self.parts.push(segment.text.clone());
                    self.committed = asr::join_segments(self.parts.iter().map(|s| s.as_str()));
                    let _ = self.app.emit(
                        "stream:segment",
                        StreamPayload {
                            partial: String::new(),
                            committed: self.committed.clone(),
                        },
                    );
                }
            }
        }
    }
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
            *guard = Some(Engine::new(&paths, &language, itn)?);
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

fn take_new(buffer: &Arc<Mutex<Vec<f32>>>, cursor: &mut usize) -> Vec<f32> {
    let Ok(guard) = buffer.lock() else {
        return Vec::new();
    };
    if guard.len() <= *cursor {
        return Vec::new();
    }
    let chunk = guard[*cursor..].to_vec();
    *cursor = guard.len();
    chunk
}

/// 边录音边喂流式识别器；每 60ms 取一次新采样，端点处提交一句。
fn run_streaming(
    app: &AppHandle,
    engine: Arc<StreamingEngine>,
    buffer: Arc<Mutex<Vec<f32>>>,
    rate: u32,
    stop: Arc<AtomicBool>,
) -> Transcript {
    let mut emitter = StreamEmitter {
        app: app.clone(),
        parts: Vec::new(),
        committed: String::new(),
    };
    run_streaming_core(engine, buffer, rate, stop, |events| emitter.apply(events))
}

fn run_streaming_core<F>(
    engine: Arc<StreamingEngine>,
    buffer: Arc<Mutex<Vec<f32>>>,
    rate: u32,
    stop: Arc<AtomicBool>,
    mut emit: F,
) -> Transcript
where
    F: FnMut(Vec<StreamEvent>),
{
    let mut session = StreamingSession::new(engine);
    let resampler = if rate as i32 == asr::SAMPLE_RATE {
        None
    } else {
        LinearResampler::create(rate as i32, asr::SAMPLE_RATE)
    };
    let mut cursor = 0usize;

    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(60));
        let chunk = take_new(&buffer, &mut cursor);
        if chunk.is_empty() {
            continue;
        }
        let samples = match &resampler {
            Some(resampler) => resampler.resample(&chunk, false),
            None => chunk,
        };
        if !samples.is_empty() {
            emit(session.push(&samples));
        }
    }

    // 收尾：把剩余采样和重采样器内部缓冲一起冲出去
    let chunk = take_new(&buffer, &mut cursor);
    let tail = match &resampler {
        Some(resampler) => resampler.resample(&chunk, true),
        None => chunk,
    };
    if !tail.is_empty() {
        emit(session.push(&tail));
    }
    emit(session.finish());
    session.transcript()
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
        let recorder = audio::start(app.clone())?;
        let buffer = recorder.buffer();
        let rate = recorder.sample_rate();
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let thread_app = app.clone();
        let thread_stop = stop.clone();
        std::thread::Builder::new()
            .name("streaming-asr".into())
            .spawn(move || {
                let transcript = run_streaming(&thread_app, engine, buffer, rate, thread_stop);
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
    let recorder = audio::start(app)?;
    *state
        .recorder
        .lock()
        .map_err(|_| "录音状态异常".to_string())? = Some(recorder);
    Ok(())
}

#[tauri::command]
async fn stop_recording(app: AppHandle, state: State<'_, AppState>) -> Result<Transcript, String> {
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
        // 先停采集（缓冲里已有全部音频），无论成功与否都要让流式线程收尾。
        let stopped = recorder.stop();
        stream.stop.store(true, Ordering::SeqCst);
        stopped?;
        return stream
            .rx
            .recv_timeout(Duration::from_secs(20))
            .map_err(|_| "流式识别收尾超时".to_string());
    }

    let rate = recorder.sample_rate();
    let samples = recorder.stop()?;
    let samples = audio::resample_to_16k(samples, rate);
    if samples.len() < wav::TARGET_RATE as usize / 5 {
        return Err("录音太短，至少说 0.2 秒".into());
    }

    transcribe_samples(&app, &state, samples, None).await
}

#[tauri::command]
async fn transcribe_wav(
    app: AppHandle,
    state: State<'_, AppState>,
    audio_base64: String,
    language: Option<String>,
) -> Result<Transcript, String> {
    let bytes = wav::decode_base64(&audio_base64)?;
    let samples = wav::wav_to_samples(&bytes)?;
    if samples.len() < wav::TARGET_RATE as usize / 5 {
        return Err("音频太短，无法转写".into());
    }
    let language = language
        .filter(|l| settings::LANGUAGES.contains(&l.as_str()))
        .filter(|l| l != "auto");
    transcribe_samples(&app, &state, samples, language).await
}

async fn transcribe_samples(
    app: &AppHandle,
    state: &AppState,
    samples: Vec<f32>,
    language_override: Option<String>,
) -> Result<Transcript, String> {
    let settings = state.settings();
    let paths = models::resolve(app, settings.model_dir.as_deref())
        .ok_or_else(|| "模型未就绪，请先下载语音模型".to_string())?;
    let engine = state.engine.clone();
    let language = language_override.unwrap_or(settings.language);
    let itn = settings.itn;

    tauri::async_runtime::spawn_blocking(move || -> Result<Transcript, String> {
        let mut guard = engine.lock().map_err(|_| "引擎状态异常".to_string())?;
        let stale = guard
            .as_ref()
            .map(|e| e.language != language || e.itn != itn)
            .unwrap_or(true);
        if stale {
            *guard = Some(Engine::new(&paths, &language, itn)?);
        }
        Ok(guard
            .as_ref()
            .ok_or_else(|| "引擎未初始化".to_string())?
            .transcribe(&samples))
    })
    .await
    .map_err(|e| format!("转写失败: {e}"))?
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
    let menu = MenuBuilder::new(handle)
        .items(&[&app_menu, &edit_menu])
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
            start_recording,
            stop_recording,
            cancel_recording,
            transcribe_wav,
            download_model,
            download_streaming_model,
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
            decoder: dir.join("decoder.int8.onnx").display().to_string(),
            joiner: dir.join("joiner.int8.onnx").display().to_string(),
            tokens: dir.join("tokens.txt").display().to_string(),
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
        let buffer = Arc::new(Mutex::new(upsampled));
        let stop = Arc::new(AtomicBool::new(false));
        let stopper = stop.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(400));
            stopper.store(true, Ordering::SeqCst);
        });

        let mut partials = 0usize;
        let transcript = run_streaming_core(engine, buffer, 48_000, stop, |events| {
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
