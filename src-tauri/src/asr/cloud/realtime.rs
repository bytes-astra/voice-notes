//! OpenAI Realtime（`gpt-live-transcribe`）适配器。
//!
//! 协议（已对照官方文档复核，2026-11）：
//!
//! - 连接：`wss://api.openai.com/v1/realtime?intent=transcription`，
//!   请求头 `Authorization: Bearer <key>`；**URL 不带 model**。
//! - 建连后发 `session.update`：`session.type="transcription"`、
//!   `audio.input.format={type:"audio/pcm",rate:24000}`、`turn_detection=null`
//!   （断句由客户端决定）、`audio.input.transcription={model:"gpt-live-transcribe"}`
//!   （可带 `languages`（复数！）与 `keywords`）。
//! - 音频：`input_audio_buffer.append`，载荷是 base64 的 PCM16。
//! - 断句：`input_audio_buffer.commit`；服务端回 `input_audio_buffer.committed`
//!   （带 `item_id`）。
//! - 文本：`conversation.item.input_audio_transcription.delta`（字段 `delta`）
//!   与 `.completed`（字段 `transcript`），失败是 `.failed`。
//!
//! **跨轮的顺序不保证**：服务端可能先回后一轮的 completed。final 按
//! `item_id` 排好序再上抛（`FinalOrder`），否则句子的先后会乱。

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::Duration;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::mpsc as tokio_mpsc;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

use super::{CloudEvent, CloudProvider, EndpointKind};

/// Realtime 服务的默认地址（`intent=transcription` 决定这是转写会话）。
pub const DEFAULT_BASE_URL: &str = "wss://api.openai.com/v1/realtime?intent=transcription";
/// Realtime 的转写模型名。
pub const DEFAULT_MODEL: &str = "gpt-live-transcribe";
/// Realtime 的 `audio/pcm` 只收 24 kHz。
pub const TARGET_SAMPLE_RATE: u32 = 24000;
/// 建连与收尾的等待上限（秒）。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const FINISH_TIMEOUT: Duration = Duration::from_secs(10);
const UPLOAD_QUEUE_CAPACITY: usize = 84;
/// 控制帧（commit / finish）排队的等待上限。
///
/// 上传队列满说明网络在背压：控制帧不能丢（丢了这一句永远等不到 final），
/// 但也不能无限等（socket 卡死时录音线程会一直挂住）。等到这个上限就报错，
/// 让调用方按「发送失败」收尾。
const CONTROL_SEND_TIMEOUT: Duration = Duration::from_secs(3);

/// Realtime 会话配置。
pub struct RealtimeConfig {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    /// 语言（ISO-639-1）。空 = 不指定，让服务自己判断。
    pub languages: Vec<String>,
    /// 热词（术语表）。
    pub keywords: Vec<String>,
    /// 本机有没有那 2.3 MB 的 VAD。
    ///
    /// 有 → 客户端断句（静音时提交，句子边界与本地模式一致）；
    /// 没有 → 不允许建连：gpt-live-transcribe 不支持 server_vad。
    pub local_vad: bool,
}

/// 后台任务收到的命令。
enum Command {
    /// 发一段音频（base64 好的 PCM16）。
    Audio(String),
    /// 提交当前这一段。
    Commit,
    /// 收尾：提交剩余音频并等最后的结果。
    Finish(Sender<()>),
}

/// Realtime provider。
pub struct RealtimeProvider {
    config: RealtimeConfig,
    /// 发给后台任务的命令。
    commands: Option<tokio_mpsc::Sender<Command>>,
    /// 后台任务报的错（连接断了之类）。
    errors: Receiver<String>,
    error_tx: Sender<String>,
    dropped: Arc<Mutex<usize>>,
}

impl RealtimeProvider {
    pub fn new(config: RealtimeConfig) -> Self {
        let (error_tx, errors) = channel();
        Self {
            config,
            commands: None,
            errors,
            error_tx,
            dropped: Arc::new(Mutex::new(0)),
        }
    }

    /// 后台任务报的错（如果有）：连接断了之后，后面的发送都要失败。
    fn take_error(&self) -> Option<String> {
        self.errors.try_recv().ok()
    }
}

impl CloudProvider for RealtimeProvider {
    fn endpoint_kind(&self) -> EndpointKind {
        if self.config.local_vad {
            EndpointKind::ClientCommit
        } else {
            EndpointKind::ServerEndpoint
        }
    }

    fn target_sample_rate(&self) -> u32 {
        TARGET_SAMPLE_RATE
    }

    fn connect(
        &mut self,
        on_event: Box<dyn Fn(CloudEvent) + Send + Sync>,
    ) -> Result<(), String> {
        let (commands_tx, commands_rx) = tokio_mpsc::channel(UPLOAD_QUEUE_CAPACITY);
        let (ready_tx, ready_rx) = channel::<Result<(), String>>();
        let config = RealtimeConfig {
            base_url: self.config.base_url.clone(),
            api_key: self.config.api_key.clone(),
            model: self.config.model.clone(),
            languages: self.config.languages.clone(),
            keywords: self.config.keywords.clone(),
            local_vad: self.config.local_vad,
        };
        let error_tx = self.error_tx.clone();
        tauri::async_runtime::spawn(async move {
            run(config, commands_rx, on_event, ready_tx, error_tx).await;
        });
        self.commands = Some(commands_tx);
        // 等连上：网络握手要时间，但不能无限等。
        match ready_rx.recv_timeout(CONNECT_TIMEOUT) {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(crate::code("cloud.connectFailed")),
        }
    }

    fn send_audio(&mut self, pcm: &[i16]) -> Result<(), String> {
        if let Some(error) = self.take_error() {
            return Err(error);
        }
        let Some(commands) = self.commands.as_ref() else {
            return Err(crate::code("cloud.disconnected"));
        };
        let bytes: Vec<u8> = pcm.iter().flat_map(|sample| sample.to_le_bytes()).collect();
        match commands.try_send(Command::Audio(BASE64.encode(bytes))) {
            Ok(()) => Ok(()),
            Err(tokio_mpsc::error::TrySendError::Full(_)) => {
                if let Ok(mut dropped) = self.dropped.lock() {
                    *dropped += pcm.len() * 16000 / TARGET_SAMPLE_RATE as usize;
                }
                Ok(())
            }
            Err(tokio_mpsc::error::TrySendError::Closed(_)) => Err(crate::code("cloud.disconnected")),
        }
    }

    fn commit(&mut self) -> Result<(), String> {
        if let Some(error) = self.take_error() {
            return Err(error);
        }
        let Some(commands) = self.commands.as_ref() else {
            return Err(crate::code("cloud.disconnected"));
        };
        // 控制帧不能因为队列满就丢：丢了这次 commit，服务端就不会为这一句
        // 出 final，会话侧的区间与 pending 记账跟着整体错位。
        send_control(commands, Command::Commit, CONTROL_SEND_TIMEOUT)
    }

    fn finish(&mut self) -> Result<(), String> {
        if let Some(error) = self.take_error() {
            return Err(error);
        }
        let Some(commands) = self.commands.as_ref() else {
            return Err(crate::code("cloud.disconnected"));
        };
        let (done_tx, done_rx) = channel();
        // 同 `commit`：收尾命令丢了就等于最后一句永远等不到结果。
        send_control(commands, Command::Finish(done_tx), CONTROL_SEND_TIMEOUT)?;
        match done_rx.recv_timeout(FINISH_TIMEOUT) {
            Ok(()) => self.take_error().map_or(Ok(()), Err),
            Err(_) => Err(self
                .take_error()
                .unwrap_or_else(|| crate::code("cloud.timeout"))),
        }
    }

    fn take_dropped(&mut self) -> usize {
        self.dropped.lock().map(|mut n| std::mem::take(&mut *n)).unwrap_or(0)
    }

    fn take_error(&mut self) -> Option<String> {
        RealtimeProvider::take_error(self)
    }
}

/// 把控制帧送进上传队列：满了就**有界**等一会儿，不丢帧也不无限阻塞。
///
/// 音频帧（`send_audio`）满了可以直接丢并记缺口；commit / finish 丢不起 ——
/// 丢一次 commit 就少一条 final，丢一次 finish 就少最后一句。但 socket 卡死
/// 时上传队列永远排不进去，所以等满 `timeout` 就按发送失败返回。
fn send_control(
    commands: &tokio_mpsc::Sender<Command>,
    mut command: Command,
    timeout: Duration,
) -> Result<(), String> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match commands.try_send(command) {
            Ok(()) => return Ok(()),
            Err(tokio_mpsc::error::TrySendError::Full(returned)) => {
                command = returned;
                if std::time::Instant::now() >= deadline {
                    return Err(crate::code("cloud.sendFailed"));
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(tokio_mpsc::error::TrySendError::Closed(_)) => {
                return Err(crate::code("cloud.disconnected"));
            }
        }
    }
}

/// 后台任务：连上 → 收音频 → 收事件。
async fn run(
    config: RealtimeConfig,
    mut commands: tokio_mpsc::Receiver<Command>,
    on_event: Box<dyn Fn(CloudEvent) + Send + Sync>,
    ready: Sender<Result<(), String>>,
    errors: Sender<String>,
) {
    let mut socket = match connect_socket(&config).await {
        Ok(socket) => socket,
        Err(error) => {
            let _ = ready.send(Err(error.clone()));
            let _ = errors.send(error);
            return;
        }
    };
    // 会话配置：转写会话、24 kHz PCM、关掉服务端断句。
    if let Err(error) = socket
        .send(Message::Text(encode_session_update(&config).into()))
        .await
    {
        let error = crate::code_detail("cloud.connectFailed", error.to_string());
        let _ = ready.send(Err(error.clone()));
        let _ = errors.send(error);
        return;
    }
    let mut ready = Some(ready);
    let mut order = FinalOrder::default();
    let mut finishing: Option<Sender<()>> = None;
    // **只有收尾才有超时**：录音本身没有时长上限（本地两条路径也没有）。
    // 之前这里挂着一个「建连 + 1 小时」的会话级 deadline，录满一小时就断连 ——
    // 那与「录音没有时长上限」的说法矛盾。
    let mut deadline: Option<tokio::time::Instant> = None;
    // 断句方式决定「还有没有在等的结果」怎么算。
    let client_commit = config.local_vad;
    let mut pending = Pending::default();
    loop {
        // 复制一份给 select 用：分支里要能改 `deadline` 本身。
        let deadline_at = deadline;
        tokio::select! {
            _ = async {
                match deadline_at {
                    Some(when) => tokio::time::sleep_until(when).await,
                    None => std::future::pending().await,
                }
            } => {
                // 收尾等太久：别把界面一直卡着。
                let _ = errors.send(crate::code("cloud.timeout"));
                if let Some(done) = finishing.take() {
                    let _ = done.send(());
                }
                break;
            }
            command = commands.recv() => {
                let Some(command) = command else { break };
                match command {
                    Command::Audio(audio) => {
                        if !audio.is_empty() {
                            pending.on_audio();
                        }
                        if let Err(error) = socket.send(Message::Text(encode_append(&audio).into())).await {
                            let _ = errors.send(crate::code_detail("cloud.sendFailed", error.to_string()));
                            break;
                        }
                    }
                    Command::Commit => {
                        if let Err(error) = socket.send(Message::Text(encode_commit().into())).await {
                            let _ = errors.send(crate::code_detail("cloud.sendFailed", error.to_string()));
                            break;
                        }
                        pending.on_commit(client_commit);
                    }
                    Command::Finish(done) => {
                        // 没什么可等的就直接交差：提交一个空缓冲，服务端多半不
                        // 回结果，等下去只是白等 10 秒，还会被误报成 `cloud.timeout`。
                        if !pending.outstanding(client_commit, order.is_empty()) {
                            let _ = done.send(());
                            break;
                        }
                        // 收尾：**只有还有没提交的音频时才提交一次** ——
                        // 客户端断句下，提交空缓冲服务端多半不回结果，等下去
                        // 只是白等 10 秒。服务端断句则始终提交一次，逼服务端把
                        // 尾巴定稿。
                        let need_commit = !client_commit || pending.uncommitted_audio;
                        if need_commit {
                            if let Err(error) = socket.send(Message::Text(encode_commit().into())).await {
                                let _ = errors.send(crate::code_detail("cloud.sendFailed", error.to_string()));
                                let _ = done.send(());
                                break;
                            }
                            pending.on_commit(client_commit);
                        }
                        finishing = Some(done);
                        deadline = Some(tokio::time::Instant::now() + FINISH_TIMEOUT);
                    }
                }
            }
            message = socket.next() => {
                let Some(message) = message else {
                    let _ = errors.send(crate::code("cloud.disconnected"));
                    if let Some(done) = finishing.take() {
                        let _ = done.send(());
                    }
                    break;
                };
                let raw = match message {
                    Ok(Message::Text(raw)) => raw,
                    // 二进制帧我们不发，也不该收：忽略。
                    Ok(Message::Binary(_)) | Ok(Message::Ping(_)) | Ok(Message::Pong(_)) => continue,
                    Ok(Message::Close(_)) => {
                        let _ = errors.send(crate::code("cloud.disconnected"));
                        if let Some(done) = finishing.take() {
                            let _ = done.send(());
                        }
                        break;
                    }
                    Ok(_) => continue,
                    Err(error) => {
                        let _ = errors.send(crate::code_detail("cloud.disconnected", error.to_string()));
                        if let Some(done) = finishing.take() {
                            let _ = done.send(());
                        }
                        break;
                    }
                };
                match decode_event(&raw) {
                    ServerEvent::Partial { item_id, delta } => {
                        // 记下最新的 item：服务端断句时靠它判断某条 final 是不是最新的那条。
                        pending.note_item(&item_id);
                        on_event(CloudEvent::PartialDelta { item_id, delta });
                    }
                    ServerEvent::Final { item_id, transcript } => {
                        // 先算「还有没有在等的音频」，再更新 latest_item ——
                        // 顺序反了的话，这条 final 永远算「最新」。
                        pending.on_final(&item_id, client_commit);
                        pending.note_item(&item_id);
                        for event in order.push(item_id, transcript) {
                            on_event(event);
                        }
                        // 收尾时最后一条结果到了：可以交差了。
                        if finishing.is_some() && order.is_empty() {
                            if let Some(done) = finishing.take() {
                                let _ = done.send(());
                                break;
                            }
                        }
                    }
                    ServerEvent::Failed { item_id, reason } => {
                        // 失败也要留痕：这句文本会从结果里消失，日志里必须有
                        // 一条可追的记录（服务端给的原因同样先脱敏）。
                        let reason = sanitize(&reason, &config.api_key);
                        let id = item_id.as_deref().unwrap_or("未知");
                        if reason.is_empty() {
                            crate::log::warn(format!("云端识别失败：item {id}"));
                        } else {
                            crate::log::warn(format!("云端识别失败：item {id}，{reason}"));
                        }
                        // 失败的 item 也要占住它在**提交队列**里的位置：用一条空
                        // final 走同一条排序通道。直接上抛会绕过 `FinalOrder` ——
                        // 失败的不是队首时，会话层按 FIFO 配区间就会配错（后面
                        // 每个句子的时间戳整体错位），排在它后面的 final 也会
                        // 因为队首永远等不到而卡在 `ready` 里。
                        pending.on_final(&item_id, client_commit);
                        pending.note_item(&item_id);
                        for event in order.push(item_id, String::new()) {
                            on_event(event);
                        }
                        if finishing.is_some() && order.is_empty() {
                            if let Some(done) = finishing.take() { let _ = done.send(()); break; }
                        }
                    }
                    ServerEvent::SessionUpdated => {
                        if let Some(ready) = ready.take() { let _ = ready.send(Ok(())); }
                    }
                    ServerEvent::Committed { item_id } => {
                        pending.note_item(&item_id);
                        for event in order.committed(item_id) {
                            on_event(event);
                        }
                    }
                    ServerEvent::Error { message } => {
                        let message = sanitize(&message, &config.api_key);
                        let lower = message.to_ascii_lowercase();
                        let code = if lower.contains("401") || lower.contains("unauthorized") || lower.contains("invalid api key") {
                            "cloud.authFailed"
                        } else { "cloud.protocolError" };
                        let error = crate::code_detail(code, message);
                        if let Some(ready) = ready.take() { let _ = ready.send(Err(error.clone())); }
                        let _ = errors.send(error);
                    }
                    ServerEvent::Other => {}
                }
            }
        }
    }
    if let Some(done) = finishing.take() {
        let _ = done.send(());
    }
}

/// 连上 Realtime。
async fn connect_socket(
    config: &RealtimeConfig,
) -> Result<tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>, String> {
    let mut request = config
        .base_url
        .as_str()
        .into_client_request()
        .map_err(|error| crate::code_detail("cloud.connectFailed", error.to_string()))?;
    // 凭据只进请求头：URL 与日志里都不许出现。
    let auth = format!("Bearer {}", config.api_key);
    let header = auth
        .parse()
        .map_err(|_| crate::code("cloud.connectFailed"))?;
    request.headers_mut().insert("Authorization", header);
    let (socket, _) = tokio_tungstenite::connect_async(request)
        .await
        .map_err(|error| {
            // 错误详情里不能有 key（有些库会把请求头带进错误里）。
            crate::code_detail("cloud.connectFailed", sanitize(&error.to_string(), &config.api_key))
        })?;
    Ok(socket)
}

/// 把凭据从文本里抹掉：日志与错误里都不该出现它。
fn sanitize(text: &str, secret: &str) -> String {
    if secret.is_empty() {
        return text.to_string();
    }
    text.replace(secret, "***")
}

/// 会话配置帧。
fn encode_session_update(config: &RealtimeConfig) -> String {
    let mut transcription = json!({ "model": config.model });
    if !config.languages.is_empty() {
        // 注意是 `languages`（复数），不是 `language`。
        transcription["languages"] = json!(config.languages);
    }
    if !config.keywords.is_empty() {
        transcription["keywords"] = json!(config.keywords);
    }
    // gpt-live-transcribe 不支持 server_vad / semantic_vad：只能客户端 commit。
    let turn_detection = Value::Null;
    json!({
        "type": "session.update",
        "session": {
            "type": "transcription",
            "audio": {
                "input": {
                    "format": { "type": "audio/pcm", "rate": TARGET_SAMPLE_RATE },
                    "transcription": transcription,
                    "turn_detection": turn_detection,
                }
            }
        }
    })
    .to_string()
}

/// 音频帧。
fn encode_append(audio_base64: &str) -> String {
    json!({ "type": "input_audio_buffer.append", "audio": audio_base64 }).to_string()
}

/// 提交帧。
fn encode_commit() -> String {
    json!({ "type": "input_audio_buffer.commit" }).to_string()
}

/// 服务端事件里我们关心的部分。
#[derive(Debug, PartialEq)]
enum ServerEvent {
    Partial {
        item_id: Option<String>,
        delta: String,
    },
    Final {
        item_id: Option<String>,
        transcript: String,
    },
    Committed {
        item_id: Option<String>,
    },
    Error {
        message: String,
    },
    Failed {
        item_id: Option<String>,
        /// 服务端给的原因（`error.message`），只进日志、已脱敏。
        reason: String,
    },
    SessionUpdated,
    Other,
}

/// 解析服务端事件。
///
/// 事件名以官方文档为准：增量是
/// `conversation.item.input_audio_transcription.delta`（字段 `delta`），
/// 定稿是 `.completed`（字段 `transcript`）。
fn decode_event(raw: &str) -> ServerEvent {
    let Ok(value) = serde_json::from_str::<Value>(raw) else {
        return ServerEvent::Other;
    };
    let kind = value.get("type").and_then(Value::as_str).unwrap_or("");
    let item_id = value
        .get("item_id")
        .and_then(Value::as_str)
        .map(str::to_string);
    match kind {
        "conversation.item.input_audio_transcription.delta" => ServerEvent::Partial {
            item_id,
            delta: value
                .get("delta")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        },
        "conversation.item.input_audio_transcription.completed" => ServerEvent::Final {
            item_id,
            transcript: value
                .get("transcript")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        },
        "input_audio_buffer.committed" => ServerEvent::Committed { item_id },
        "conversation.item.input_audio_transcription.failed" => ServerEvent::Failed {
            item_id,
            reason: value
                .get("error")
                .and_then(|error| error.get("message"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        },
        "session.updated" => ServerEvent::SessionUpdated,
        "error" => ServerEvent::Error {
            message: value
                .get("error")
                .and_then(|error| error.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string(),
        },
        _ => ServerEvent::Other,
    }
}

/// 收尾时「还有没有在等的结果」的记账。
///
/// 两种断句方式算的不一样：
/// - **客户端断句**（`client_commit`）：每次提交对应一句。已提交但还没回来的
///   结果记在 `pending_utterances`；提交之后又喂了音频就是 `uncommitted_audio`。
/// - **服务端断句**（`server_endpoint`）：客户端不知道句子的边界，只能看
///   「最后一次结果之后还有没有新音频」。
#[derive(Default)]
struct Pending {
    /// 客户端断句：上次提交之后又喂了音频。
    uncommitted_audio: bool,
    /// 客户端断句：已提交、还没回来的结果数。
    pending_utterances: usize,
    /// 服务端断句：最后一次结果之后又喂了音频。
    audio_since_final: bool,
    /// 最近一次带 `item_id` 的事件（服务端断句时用来判断 final 是不是最新的那条）。
    latest_item: Option<String>,
}

impl Pending {
    /// 喂了音频。
    fn on_audio(&mut self) {
        self.uncommitted_audio = true;
        self.audio_since_final = true;
    }

    /// 提交了一句。
    fn on_commit(&mut self, client_commit: bool) {
        if client_commit {
            self.uncommitted_audio = false;
            self.pending_utterances += 1;
        }
    }

    /// 来了一条定稿结果。
    fn on_final(&mut self, item_id: &Option<String>, client_commit: bool) {
        if client_commit {
            self.pending_utterances = self.pending_utterances.saturating_sub(1);
            return;
        }
        // 服务端断句：只有「这条 final 就是最新那一件」才能认为音频都定稿了。
        // 跨件的顺序不保证 —— 迟到的旧 final 不能把新音频的等待抹掉。
        let is_latest = match item_id {
            Some(id) => self.latest_item.as_deref() == Some(id.as_str()),
            None => false,
        };
        if is_latest {
            self.audio_since_final = false;
        }
    }

    /// 记下事件里的 `item_id`。
    fn note_item(&mut self, item_id: &Option<String>) {
        if let Some(id) = item_id {
            self.latest_item = Some(id.clone());
        }
    }

    /// 还有没有在等的东西？`order_empty` = 没有排队等着的定稿结果。
    fn outstanding(&self, client_commit: bool, order_empty: bool) -> bool {
        if !order_empty {
            return true;
        }
        if client_commit {
            self.uncommitted_audio || self.pending_utterances > 0
        } else {
            self.audio_since_final
        }
    }
}

/// final 的排序：按服务端的提交顺序上抛。
///
/// 跨轮顺序不保证：服务端可能先回后一轮的 completed。已提交但还没回来的
/// `item_id` 排成队，final 到了先存着，等它前面那几个到了再一起发。
#[derive(Default)]
struct FinalOrder {
    /// 已提交、还没回来的 `item_id`（按提交顺序）。
    queue: VecDeque<String>,
    /// 已经回来、但还没轮到的事件（按到达顺序）。
    ready: Vec<(String, CloudEvent)>,
    /// 已经发出去过的 `item_id`：服务端可能先给 final、后给 committed，
    /// 这种回执不该再排进队（排进去就永远等不到，队列也就永远不会空）。
    emitted: Vec<String>,
}

impl FinalOrder {
    /// 服务端确认了这一轮：可能顺带把等着的 final 放出来。
    fn committed(&mut self, item_id: Option<String>) -> Vec<CloudEvent> {
        if let Some(item_id) = item_id {
            // 已经发出去过的就别再排队等了。
            if !self.queue.contains(&item_id) && !self.emitted.contains(&item_id) {
                self.queue.push_back(item_id);
            }
        }
        self.flush()
    }

    /// 一条 final 到了：能发就发，不能发就存着。
    ///
    /// 识别失败的 item 用**空文本**走同一条通道（`run` 的 `Failed` 分支）：
    /// 排序、放行后面的 final、让会话层弹掉对应区间，三件事都由它一次做完。
    fn push(&mut self, item_id: Option<String>, transcript: String) -> Vec<CloudEvent> {
        let event = CloudEvent::Final {
            text: transcript,
            start_ms: None,
            end_ms: None,
        };
        match item_id {
            Some(item_id) => {
                self.ready.push((item_id, event));
                self.flush()
            }
            // 没有 item_id：没法排序，直接发。
            None => vec![event],
        }
    }

    /// 队首就绪就发出去。
    fn flush(&mut self) -> Vec<CloudEvent> {
        let mut out = Vec::new();
        while let Some(front) = self.queue.front().cloned() {
            let Some(index) = self.ready.iter().position(|(id, _)| *id == front) else {
                break;
            };
            let (_, event) = self.ready.remove(index);
            self.queue.pop_front();
            self.emitted.push(front);
            out.push(event);
        }
        // 队列空了：剩下的（服务端没给 committed 的）按到达顺序发出去。
        if self.queue.is_empty() && !self.ready.is_empty() {
            for (id, event) in self.ready.drain(..) {
                self.emitted.push(id);
                out.push(event);
            }
        }
        out
    }

    /// 还没等着的结果（收尾时用来判断能不能交差）。
    fn is_empty(&self) -> bool {
        self.queue.is_empty() && self.ready.is_empty()
    }
}

#[cfg(test)]
mod protocol_tests;
