//! VoiceNotes —— 本地语音转文字桌面应用（Tauri 2 + SenseVoiceSmall INT8）。

pub mod asr;
pub mod audio;
pub mod models;
pub mod settings;
pub mod wav;

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use asr::{Engine, Transcript};
use audio::Recorder;
use settings::Settings;

pub struct AppState {
    settings: Mutex<Settings>,
    recorder: Mutex<Option<Recorder>>,
    engine: Arc<Mutex<Option<Engine>>>,
    cancel_download: Arc<AtomicBool>,
    downloading: Arc<AtomicBool>,
}

impl AppState {
    fn new(settings: Settings) -> Self {
        Self {
            settings: Mutex::new(settings),
            recorder: Mutex::new(None),
            engine: Arc::new(Mutex::new(None)),
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
    itn: bool,
    dsh_available: bool,
    dsh_dir: Option<String>,
    downloading: bool,
    installed_bytes: u64,
    total_bytes: u64,
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
    let dsh = models::dsh_model_dir();
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
    Status {
        model_ready: paths.is_some(),
        vad_ready: paths.as_ref().map(|p| p.vad.is_some()).unwrap_or(false),
        model_source: paths.as_ref().map(|p| p.source.clone()),
        model_dir: paths.as_ref().map(|p| p.dir.clone()),
        output_dir: output_dir(app, settings).to_string_lossy().into_owned(),
        language: settings.language.clone(),
        itn: settings.itn,
        dsh_available: dsh.is_some(),
        dsh_dir: dsh.map(|p| p.to_string_lossy().into_owned()),
        downloading: false,
        installed_bytes,
        total_bytes,
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

#[tauri::command]
async fn warm_up(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    load_engine(&app, &state).await
}

#[tauri::command]
fn start_recording(app: AppHandle, state: State<AppState>) -> Result<(), String> {
    let settings = state.settings();
    if models::resolve(&app, settings.model_dir.as_deref()).is_none() {
        return Err("模型未就绪，请先下载语音模型".into());
    }
    let mut slot = state
        .recorder
        .lock()
        .map_err(|_| "录音状态异常".to_string())?;
    if slot.is_some() {
        return Err("已经在录音了".into());
    }
    *slot = Some(audio::start(app)?);
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
            start_recording,
            stop_recording,
            cancel_recording,
            transcribe_wav,
            download_model,
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
