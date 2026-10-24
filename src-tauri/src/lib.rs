//! VoiceNotes —— 本地语音转文字桌面应用（Tauri 2 + SenseVoiceSmall INT8）。

pub mod asr;
pub mod audio;
pub mod decode;
pub mod engine;
pub mod log;
pub mod menu;
pub mod models;
pub mod output;
pub mod record;
pub mod settings;

use std::fs;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use asr::{OfflineEvent, StreamEvent, Transcript};
use engine::{PunctuationEngine, StreamingEngine};
use audio::AudioErrorPayload;
use engine::{
    ensure_punctuation, ensure_streaming_engine, load_engine, output_dir, persist, shared_engine,
    status_of, AppState, Status,
};
use log::on_off;
// install_menu 只在 macOS 存在（menu.rs 里是 cfg 掉的），import 也要跟着 cfg，
// 否则 Windows 上会报 unresolved import。
#[cfg(target_os = "macos")]
use menu::install_menu;
use output::{sanitize_name, write_text_file, Saved};
use settings::Settings;
use record::{log_transcript, run_offline, run_streaming_core, OfflineRun, StreamSession};

/// 错误码：后端只返回字典 key（前端 i18n 翻译），不返回展示文案。
///
/// `code("record.empty")` → `"record.empty"`；`code_detail("record.micError", e)` →
/// `"record.micError:<e>"`。前端 `core.js` 的 `fmtError()` 查字典翻译。
pub fn code(key: &str) -> String {
    key.to_string()
}

/// 带细节的错误码：细节是技术信息（路径、系统错误文本、格式名）。
///
/// **细节只进日志，不进界面**：用户看到的是字典里的干净文案
/// （`record.micError` → 「麦克风发生异常」），细节写进日志文件
/// （`~/Library/Logs/com.github.bytes-astra.voicenotes/voice-notes.log`，见 log.rs）。
/// 细节仍然跟着错误码一起回给前端，是为了填 `{value}` 这类
/// 用户看得懂的参数（比如「找不到输入设备：{value}」）。
pub fn code_detail(key: &str, detail: impl std::fmt::Display) -> String {
    let detail = detail.to_string();
    // 细节只进日志：用户看到的是字典里的干净文案。
    log::error(format!("{key}: {detail}"));
    format!("{key}:{detail}")
}

/// 进度事件的接收端。
///
/// 整段和流式的下载**可以同时跑**，而进度事件是同一个名字（`model:progress`），
/// 所以载荷里必须带 `kind` 标明这是哪个下载 —— 否则两个面板会互相显示对方的进度。
struct EventSink {
    app: AppHandle,
    /// "offline"（整段） / "streaming"（流式）
    kind: &'static str,
}

impl EventSink {
    fn new(app: AppHandle, kind: &'static str) -> Self {
        Self { app, kind }
    }
}

impl models::ProgressSink for EventSink {
    fn report(&self, progress: models::Progress) {
        let mut payload = serde_json::to_value(progress).unwrap_or_default();
        if let Some(obj) = payload.as_object_mut() {
            obj.insert(
                "kind".to_string(),
                serde_json::Value::String(self.kind.to_string()),
            );
        }
        let _ = self.app.emit("model:progress", payload);
    }
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
                    emit_gap_notice(&self.app, &segment.text);
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

/// 丢掉音频时给前端一条明确的提示（`fatal: false` → 前端弹 toast，录音继续）。
///
/// 缺口占位符（`[[gap:秒数]]`）本身会进结果文本；这里额外发一条事件，
/// 让用户知道「刚才丢了 N 秒」，而不只是看到一段占位符。
fn emit_gap_notice(app: &AppHandle, text: &str) {
    if let Some(seconds) = audio::gap_marker_seconds(text) {
        let _ = app.emit("audio:error", AudioErrorPayload::gap(seconds));
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
        return Err(code_detail("error.unsupportedLanguage", language));
    }
    {
        let mut guard = state
            .settings
            .lock()
            .map_err(|_| code("error.settingsState"))?;
        guard.language = language.clone();
    }
    persist(&app, &state)?;
    log::info(format!("设置变更：语言 = {language}"));
    Ok(status_of(&app, &state.settings()))
}

#[tauri::command]
fn set_itn(app: AppHandle, state: State<AppState>, enabled: bool) -> Result<Status, String> {
    {
        let mut guard = state
            .settings
            .lock()
            .map_err(|_| code("error.settingsState"))?;
        guard.itn = enabled;
    }
    persist(&app, &state)?;
    log::info(format!("设置变更：ITN = {}", on_off(enabled)));
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
            .map_err(|_| code("error.settingsState"))?;
        guard.output_dir = dir.filter(|d| !d.trim().is_empty());
    }
    if let Some(dir) = state.settings().output_dir {
        fs::create_dir_all(&dir).map_err(|e| code_detail("error.createDirFailed", format!("{dir}: {e}")))?;
    }
    persist(&app, &state)?;
    log::info(format!(
        "设置变更：输出目录 = {}",
        output_dir(&app, &state.settings()).display()
    ));
    Ok(status_of(&app, &state.settings()))
}

#[tauri::command]
async fn set_model_dir(
    app: AppHandle,
    state: State<'_, AppState>,
    dir: Option<String>,
) -> Result<Status, String> {
    {
        let mut guard = state
            .settings
            .lock()
            .map_err(|_| code("error.settingsState"))?;
        guard.model_dir = dir.filter(|d| !d.trim().is_empty());
    }
    persist(&app, &state)?;
    log::info(format!(
        "设置变更：模型目录 = {}",
        state
            .settings()
            .model_dir
            .clone()
            .unwrap_or_else(|| "默认".to_string())
    ));
    // 模型换了，旧引擎作废。加载可能正占着锁：清空放阻塞线程里，别卡住同步命令。
    let engine = state.engine.clone();
    let _ = tauri::async_runtime::spawn_blocking(move || {
        if let Ok(mut slot) = engine.lock() {
            *slot = None;
        }
    })
    .await;
    Ok(status_of(&app, &state.settings()))
}

/// 边录音边喂流式识别器；每 60ms 取一次新采样，端点处提交一句。
fn run_streaming(
    app: &AppHandle,
    engine: Arc<StreamingEngine>,
    punctuator: Option<Arc<PunctuationEngine>>,
    queue: audio::SampleQueue,
    rate: u32,
    stop: Arc<AtomicBool>,
    discard: Arc<AtomicBool>,
) -> Transcript {
    let mut emitter = StreamEmitter { app: app.clone() };
    run_streaming_core(engine, punctuator, queue, rate, stop, discard, |events| {
        emitter.apply(events)
    })
}

#[tauri::command]
async fn warm_up(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    load_engine(&app, &state).await?;
    log::info("整段引擎预热完成");
    Ok(())
}

#[tauri::command]
async fn warm_up_streaming(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    ensure_streaming_engine(&app, &state).await?;
    log::info("流式引擎预热完成");
    Ok(())
}

#[tauri::command]
async fn set_mode(
    app: AppHandle,
    state: State<'_, AppState>,
    mode: String,
) -> Result<Status, String> {
    if !settings::MODES.contains(&mode.as_str()) {
        return Err(code_detail("error.unsupportedMode", mode));
    }
    {
        let mut guard = state
            .settings
            .lock()
            .map_err(|_| code("error.settingsState"))?;
        guard.mode = mode.clone();
    }
    persist(&app, &state)?;
    log::info(format!("设置变更：模式 = {mode}"));
    // 只留当前模式要用的引擎：两份模型同时常驻要多占约 400 MB。
    // 加载可能正占着锁，清空放阻塞线程里，别卡住同步命令。
    let engine = state.engine.clone();
    let streaming = state.streaming_engine.clone();
    let punct = state.punct_engine.clone();
    let keep_streaming = mode == "streaming";
    let _ = tauri::async_runtime::spawn_blocking(move || {
        if keep_streaming {
            if let Ok(mut slot) = engine.lock() {
                *slot = None;
            }
        } else {
            if let Ok(mut slot) = streaming.lock() {
                *slot = None;
            }
            if let Ok(mut slot) = punct.lock() {
                *slot = None;
            }
        }
    })
    .await;
    Ok(status_of(&app, &state.settings()))
}

/// 保存流式术语表。
///
/// 解码方式（greedy / beam）是在创建识别器时定死的，词表变了必须把流式引擎清掉，
/// 下次录音按新词表重建（与切换模式同样的代价：重新加载 161 MB 模型）。
#[tauri::command]
async fn set_hotwords(
    app: AppHandle,
    state: State<'_, AppState>,
    hotwords: Vec<String>,
) -> Result<Status, String> {
    {
        let mut guard = state
            .settings
            .lock()
            .map_err(|_| code("error.settingsState"))?;
        guard.hotwords = settings::clean_hotwords(hotwords);
    }
    persist(&app, &state)?;
    log::info(format!(
        "设置变更：热词 = {} 条",
        state.settings().hotwords.len()
    ));
    let streaming = state.streaming_engine.clone();
    let _ = tauri::async_runtime::spawn_blocking(move || {
        if let Ok(mut slot) = streaming.lock() {
            *slot = None;
        }
    })
    .await;
    Ok(status_of(&app, &state.settings()))
}

/// 示例术语表（内置，供设置页「填入示例」按钮用）。
#[tauri::command]
fn default_hotwords() -> Vec<String> {
    include_str!("../assets/hotwords.txt")
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

#[tauri::command]
async fn set_punct(
    app: AppHandle,
    state: State<'_, AppState>,
    enabled: bool,
) -> Result<Status, String> {
    {
        let mut guard = state
            .settings
            .lock()
            .map_err(|_| code("error.settingsState"))?;
        guard.punct = enabled;
    }
    if !enabled {
        // 同上：标点模型可能正在加载，别让同步命令卡在锁上。
        let punct = state.punct_engine.clone();
        let _ = tauri::async_runtime::spawn_blocking(move || {
            if let Ok(mut slot) = punct.lock() {
                *slot = None;
            }
        })
        .await;
    }
    persist(&app, &state)?;
    log::info(format!("设置变更：标点 = {}", on_off(enabled)));
    Ok(status_of(&app, &state.settings()))
}

/// 列出系统里的输入设备（麦克风）。
/// 预热麦克风：提前触发系统权限弹窗（界面渲染完之后再调用）。
#[tauri::command]
async fn warm_up_mic(state: State<'_, AppState>) -> Result<(), String> {
    let device = state.settings().input_device;
    tauri::async_runtime::spawn_blocking(move || audio::probe_input(device.as_deref()))
        .await
        .map_err(|e| code_detail("record.micWarmupFailed", e))??;
    log::info("麦克风预热完成");
    Ok(())
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
            .map_err(|_| code("error.settingsState"))?;
        guard.input_device = name.filter(|value| !value.trim().is_empty());
    }
    persist(&app, &state)?;
    log::info(format!(
        "设置变更：输入设备 = {}",
        state
            .settings()
            .input_device
            .clone()
            .unwrap_or_else(|| "默认".to_string())
    ));
    Ok(status_of(&app, &state.settings()))
}

#[tauri::command]
async fn download_streaming_model(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Status, String> {
    if state.downloading_streaming.swap(true, Ordering::SeqCst) {
        return Err(code("setup.busy"));
    }
    state.cancel_download_streaming.store(false, Ordering::SeqCst);
    let dir = models::streaming_install_dir(&app)?;
    // 标点模型和流式模型一起下载
    let punct_dir = models::punctuation_install_dir(&app)?;
    log::info(format!(
        "开始下载流式模型：{}（含标点模型 {}）",
        dir.display(),
        punct_dir.display()
    ));
    let sink = EventSink::new(app.clone(), "streaming");
    let result = models::download_streaming(
        &dir,
        &punct_dir,
        state.cancel_download_streaming.clone(),
        &sink,
    )
    .await;
    state.downloading_streaming.store(false, Ordering::SeqCst);
    result?;
    log::info("流式模型下载完成");
    let _ = ensure_streaming_engine(&app, &state).await;
    Ok(status_of(&app, &state.settings()))
}

#[tauri::command]
async fn start_recording(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    {
        let slot = state
            .recorder
            .lock()
            .map_err(|_| code("error.recordState"))?;
        if slot.is_some() {
            return Err(code("record.alreadyRecording"));
        }
    }
    let settings = state.settings();
    // 设备名：设置里没选就是系统默认（audio::start 挑不到也会退回默认）。
    let device_label = settings
        .input_device
        .clone()
        .unwrap_or_else(|| "默认".to_string());
    // 两种模式的两条路径差太多（引擎、会话、线程都不一样），分开写更清楚。
    if settings.mode == "streaming" {
        start_streaming_recording(app, state, settings, device_label).await
    } else {
        start_offline_recording(app, state, settings, device_label).await
    }
}

/// 流式模式：起采集 + 流式识别线程（200 ms 一片，边说边出字）。
async fn start_streaming_recording(
    app: AppHandle,
    state: State<'_, AppState>,
    settings: Settings,
    device_label: String,
) -> Result<(), String> {
    if models::resolve_streaming(&app).is_none() {
        return Err(code("model.needStreaming"));
    }
    let engine = ensure_streaming_engine(&app, &state).await?;
    let punctuator = if settings.punct {
        ensure_punctuation(&app, &state).await.ok()
    } else {
        None
    };
    let recorder = {
        let audio_app = app.clone();
        let device = settings.input_device.clone();
        // 开麦克风会等驱动（最长 10s），放进阻塞线程，别堵 async 执行器。
        tauri::async_runtime::spawn_blocking(move || audio::start(audio_app, device.as_deref()))
            .await
            .map_err(|e| code_detail("record.micOpenFailed", e))??
    };
    let queue = recorder.queue();
    let rate = recorder.sample_rate();
    log::info(format!(
        "开始录音：模式 streaming，设备 {device_label}，采样率 {rate} Hz"
    ));
    let stop = Arc::new(AtomicBool::new(false));
    let discard = Arc::new(AtomicBool::new(false));
    let (tx, rx) = std::sync::mpsc::channel();
    let thread_app = app.clone();
    let thread_stop = stop.clone();
    let thread_discard = discard.clone();
    std::thread::Builder::new()
        .name("streaming-asr".into())
        .spawn(move || {
            let transcript = run_streaming(
                &thread_app,
                engine,
                punctuator,
                queue,
                rate,
                thread_stop,
                thread_discard,
            );
            let _ = tx.send(transcript);
        })
        .map_err(|e| code_detail("error.streamThreadFailed", e))?;
    // 检查和写入放在同一把锁里：两次 start 并发时不能各起一个会话，
    // 否则先注册的那个识别线程永远收不到 stop（线程泄漏）。
    {
        let mut slot = state
            .recorder
            .lock()
            .map_err(|_| code("error.recordState"))?;
        if slot.is_some() {
            drop(slot);
            stop.store(true, Ordering::SeqCst);
            discard.store(true, Ordering::SeqCst);
            drop(recorder);
            return Err(code("record.alreadyRecording"));
        }
        *slot = Some(recorder);
        let session = StreamSession { stop, discard, rx };
        *state
            .stream
            .lock()
            .map_err(|_| code("error.streamState"))? = Some(session);
    }
    Ok(())
}

/// 整段模式：起采集 + 整段识别线程（边录边按句识别，音频消费掉即释放）。
async fn start_offline_recording(
    app: AppHandle,
    state: State<'_, AppState>,
    settings: Settings,
    device_label: String,
) -> Result<(), String> {
    // 整段模式：边录边按句识别（音频消费掉即释放，内存不随录音时长增长）
    load_engine(&app, &state).await?;
    let engine = shared_engine(&state)?;
    let vad = engine
        .vad_config()
        .ok_or_else(|| code("model.vadMissing"))?;
    let recorder = {
        let audio_app = app.clone();
        let device = settings.input_device.clone();
        tauri::async_runtime::spawn_blocking(move || audio::start(audio_app, device.as_deref()))
            .await
            .map_err(|e| code_detail("record.micOpenFailed", e))??
    };
    let queue = recorder.queue();
    let rate = recorder.sample_rate();
    log::info(format!(
        "开始录音：模式 offline，设备 {device_label}，采样率 {rate} Hz"
    ));
    let stop = Arc::new(AtomicBool::new(false));
    let discard = Arc::new(AtomicBool::new(false));
    let (tx, rx) = std::sync::mpsc::channel();
    let thread_app = app.clone();
    let thread_stop = stop.clone();
    let thread_discard = discard.clone();
    std::thread::Builder::new()
        .name("offline-asr".into())
        .spawn(move || {
            let transcript = run_offline(
                engine,
                vad,
                queue,
                rate,
                thread_stop,
                thread_discard,
                |event| {
                    let OfflineEvent::Segment(segment) = event;
                    emit_gap_notice(&thread_app, &segment.text);
                    let _ = thread_app.emit(
                        "stream:segment",
                        SegmentPayload {
                            segment: segment.text,
                            start: segment.start,
                            end: segment.end,
                        },
                    );
                },
            );
            let _ = tx.send(transcript);
        })
        .map_err(|e| code_detail("error.offlineThreadFailed", e))?;
    // 同上：检查和写入在同一把锁里，并发 start 只能成功一个。
    {
        let mut slot = state
            .recorder
            .lock()
            .map_err(|_| code("error.recordState"))?;
        if slot.is_some() {
            drop(slot);
            stop.store(true, Ordering::SeqCst);
            discard.store(true, Ordering::SeqCst);
            drop(recorder);
            return Err(code("record.alreadyRecording"));
        }
        *slot = Some(recorder);
        *state
            .offline_session
            .lock()
            .map_err(|_| code("error.recordState"))? = Some(OfflineRun { stop, discard, rx });
    }
    Ok(())
}

#[tauri::command]
async fn stop_recording(_app: AppHandle, state: State<'_, AppState>) -> Result<Transcript, String> {
    let recorder = {
        let mut slot = state
            .recorder
            .lock()
            .map_err(|_| code("error.recordState"))?;
        slot.take()
    }
    .ok_or_else(|| code("record.notRecording"))?;
    let stream = {
        let mut slot = state
            .stream
            .lock()
            .map_err(|_| code("error.streamState"))?;
        slot.take()
    };

    // 停止采集 + 等识别线程收尾都是阻塞操作（最长 20/30 秒），放进阻塞线程。
    if let Some(stream) = stream {
        let transcript = tauri::async_runtime::spawn_blocking(move || -> Result<Transcript, String> {
            // 先停采集（尾巴留在队列里，由识别线程收尾时取走）。
            let stopped = recorder.stop();
            // stop 报错也要让识别线程退出，否则它会一直空转。
            stream.stop.store(true, Ordering::SeqCst);
            stopped?;
            stream
                .rx
                .recv_timeout(Duration::from_secs(20))
                .map_err(|_| code("record.streamTimeout"))
        })
        .await
        .map_err(|e| code_detail("record.stopFailed", e))??;
        log_transcript("流式", &transcript);
        return Ok(transcript);
    }

    // 整段模式：识别线程一直在跑，这里停采集，尾巴由它收尾时取走
    let offline = state
        .offline_session
        .lock()
        .map_err(|_| code("error.recordState"))?
        .take();
    let Some(offline) = offline else {
        return Err(code("record.threadExited"));
    };
    let transcript = tauri::async_runtime::spawn_blocking(move || -> Result<Transcript, String> {
        let stopped = recorder.stop();
        offline.stop.store(true, Ordering::SeqCst);
        stopped?;
        let transcript = offline
            .rx
            .recv_timeout(Duration::from_secs(30))
            .map_err(|_| code("record.offlineTimeout"))?;
        if transcript.duration_seconds < 0.2 {
            return Err(code_detail("record.tooShort", "0.2"));
        }
        if transcript.text.trim().is_empty() {
            return Err(code("record.empty"));
        }
        Ok(transcript)
    })
    .await
    .map_err(|e| code_detail("record.stopFailed", e))??;
    log_transcript("整段", &transcript);
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
    log::info(format!("文件转写开始：{path}"));
    load_engine(&app, &state).await?;
    let engine = shared_engine(&state)?;
    let thread_app = app.clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<Transcript, String> {
        let vad = engine
            .vad_config()
            .ok_or_else(|| code("model.vadMissing"))?;
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

        let decode_info = decode::decode_to_16k(std::path::Path::new(&path), |chunk| {
            for event in session.push(chunk) {
                let asr::OfflineEvent::Segment(segment) = event;
                emit_segment(&segment);
            }
            Ok(())
        })?;
        // 解码只完成了一部分：界面只提示「文件解码中断」，原因进日志。
        if let Some(warning) = decode_info.warning {
            log::warn(format!("decode.partial: {warning}"));
            let _ = thread_app.emit(
                "audio:error",
                AudioErrorPayload::partial("decode.partial", warning),
            );
        }

        for event in session.finish() {
            let asr::OfflineEvent::Segment(segment) = event;
            emit_segment(&segment);
        }

        let transcript = Transcript {
            text: asr::join_segments(session.segments().iter().map(|s| s.text.as_str())),
            segments: session.segments().to_vec(),
            duration_seconds: session.recorded_samples() as f32 / asr::SAMPLE_RATE as f32,
            elapsed_ms: session.elapsed_ms(),
            language: engine.language.clone(),
        };
        log::info(format!(
            "文件转写完成：时长 {:.1}s，句子 {} 段",
            transcript.duration_seconds,
            transcript.segments.len()
        ));
        Ok(transcript)
    })
    .await
    // 阻塞线程 panic 了：原样把系统信息抛出去（前端 fmtError 查不到字典就显示原文）。
    .map_err(|e| e.to_string())?
}

/// 从缓存里拿一个可共享的整段引擎（录音线程和文件转写共用同一份模型）。

#[tauri::command]
fn cancel_recording(state: State<AppState>) -> Result<(), String> {
    let recorder = {
        let mut slot = state
            .recorder
            .lock()
            .map_err(|_| code("error.recordState"))?;
        slot.take()
    };
    if let Some(stream) = state.stream.lock().ok().and_then(|mut slot| slot.take()) {
        stream.discard.store(true, Ordering::SeqCst);
        stream.stop.store(true, Ordering::SeqCst);
    }
    // 整段模式的识别线程也要停，否则它会一直空转（队列空了也每 60ms 醒一次）
    if let Some(offline) = state
        .offline_session
        .lock()
        .ok()
        .and_then(|mut slot| slot.take())
    {
        offline.discard.store(true, Ordering::SeqCst);
        offline.stop.store(true, Ordering::SeqCst);
    }
    if let Some(recorder) = recorder {
        recorder.cancel();
    }
    log::info("录音已取消");
    Ok(())
}

#[tauri::command]
async fn download_model(app: AppHandle, state: State<'_, AppState>) -> Result<Status, String> {
    if state.downloading.swap(true, Ordering::SeqCst) {
        return Err(code("setup.busy"));
    }
    state.cancel_download.store(false, Ordering::SeqCst);
    let dir = models::install_dir(&app)?;
    log::info(format!("开始下载离线模型：{}", dir.display()));
    let sink = EventSink::new(app.clone(), "offline");
    let result = models::download_all(&dir, state.cancel_download.clone(), &sink).await;
    state.downloading.store(false, Ordering::SeqCst);
    result?;
    log::info("离线模型下载完成");
    // 流式模式用不到整段引擎：别为了下载把它常驻在内存里（文件转写会按需加载）。
    if state.settings().mode == "offline" {
        let _ = load_engine(&app, &state).await;
    }
    Ok(status_of(&app, &state.settings()))
}

/// 打开转写结果的保存目录。
///
/// **不用 opener 插件**：那个要靠 capability 的 scope 限定允许打开的路径，而保存
/// 目录是用户可改的（`settings.output_dir`），静态 scope 盖不全 —— 表现就是
/// 「Not allowed to open path」。后端自己解析出真实路径再交给系统打开。
#[tauri::command]
fn open_output_dir(app: AppHandle, state: State<AppState>) -> Result<(), String> {
    let dir = engine::output_dir(&app, &state.settings());
    #[cfg(target_os = "macos")]
    let mut cmd = std::process::Command::new("open");
    #[cfg(target_os = "windows")]
    let mut cmd = std::process::Command::new("explorer");
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut cmd = std::process::Command::new("xdg-open");
    cmd.arg(&dir)
        .spawn()
        .map(|_| ())
        .map_err(|e| code_detail("settings.openDirFailed", e))
}

#[tauri::command]
fn cancel_download(state: State<AppState>, kind: Option<String>) {
    // 两个下载可以同时跑，所以取消也要指明取消哪一个。
    match kind.as_deref() {
        Some("streaming") => state
            .cancel_download_streaming
            .store(true, Ordering::SeqCst),
        _ => state.cancel_download.store(true, Ordering::SeqCst),
    }
}

#[tauri::command]
/// 把文本写进目录；同名文件依次加序号，绝不覆盖。
fn save_text(
    app: AppHandle,
    state: State<AppState>,
    text: String,
    file_name: Option<String>,
) -> Result<Saved, String> {
    if text.trim().is_empty() {
        return Err(code("result.noSave"));
    }
    let settings = state.settings();
    let dir = output_dir(&app, &settings);
    fs::create_dir_all(&dir).map_err(|e| code_detail("error.createOutputDir", format!("{}: {e}", dir.display())))?;

    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let requested = file_name
        .as_deref()
        .and_then(sanitize_name)
        .unwrap_or_else(|| format!("transcript-{stamp}"));
    let (path, name) = write_text_file(&dir, &requested, &text)?;
    Ok(Saved {
        path: path.to_string_lossy().into_owned(),
        name,
    })
}

/// 前端（webview）那边的事件也记进同一个日志文件。
///
/// 前端的异常、界面操作走这个命令 —— 这样一份文件就是完整故事，
/// 用户反馈问题时不用再拼「界面说的」和「后端日志」两份东西。
/// 不用用户配置什么，日志一直开着。
#[tauri::command]
fn client_log(level: String, message: String) {
    let level = match level.to_ascii_uppercase().as_str() {
        "WARN" => "WARN",
        "ERROR" => "ERROR",
        _ => "INFO",
    };
    log::write(level, format!("[ui] {message}"));
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            // 日志一直开着，不需要用户配置：写到系统标准日志目录 + stderr。
            log::init(app.handle());
            let loaded = settings::load(app.handle());
            // 启动快照：一行看清这次会话用的是什么配置。
            log::info(format!(
                "启动：版本 {}，模式 {}，语言 {}，ITN {}，标点 {}",
                app.package_info().version,
                loaded.mode,
                loaded.language,
                on_off(loaded.itn),
                on_off(loaded.punct),
            ));
            app.manage(AppState::new(loaded));
            // 旧版默认输出目录是中文的「语音转文字」，一次性搬成 ASCII 的 VoiceNotes
            settings::migrate_output_dir(app.handle());
            // macOS 原生材质（NSVisualEffectView）：窗口是透明的，侧栏不画底色，
            // 系统材质从后面透出来；主区自己刷不透明底色。
            #[cfg(target_os = "macos")]
            if let Some(window) = app.get_webview_window("main") {
                use tauri::window::{Effect, EffectsBuilder};
                let _ = window.set_effects(
                    EffectsBuilder::new().effect(Effect::Sidebar).build(),
                );
            }
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
            set_hotwords,
            default_hotwords,
            list_input_devices,
            warm_up_mic,
            set_input_device,
            start_recording,
            stop_recording,
            cancel_recording,
            transcribe_file,
            download_model,
            download_streaming_model,
            cancel_download,
            open_output_dir,
            save_text,
            client_log,
        ])
        .run(tauri::generate_context!())
        .expect("error while running voice-notes");
}
