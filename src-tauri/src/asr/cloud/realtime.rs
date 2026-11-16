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
//! **跨轮的顺序不保证**：服务端可能先回后一轮的 completed。结果按**本地
//! 提交顺序**上抛（`CommitTracker`），回执先发（会话层靠它把句子区间绑到
//! `item_id` 上）—— 上抛顺序再怎么乱，句子区间都不会配错。

use std::collections::VecDeque;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::Duration;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::mpsc as tokio_mpsc;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

use super::partial::{self, PartialUpdate};
use super::{
    sanitize, Capabilities, CloudEvent, CloudProvider, EndpointKind, PartialSemantics, SendOutcome,
};

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
}

impl RealtimeProvider {
    pub fn new(config: RealtimeConfig) -> Self {
        let (error_tx, errors) = channel();
        Self {
            config,
            commands: None,
            errors,
            error_tx,
        }
    }

    /// 后台任务报的错（如果有）：连接断了之后，后面的发送都要失败。
    ///
    /// **不叫 `take_error`**：trait 上有同名默认方法，固有方法会被它遮蔽
    /// （调用落回默认实现 → 恒为 `None` → 后台错误永远读不出来）。
    fn take_background_error(&self) -> Option<String> {
        self.errors.try_recv().ok()
    }
}

impl RealtimeProvider {
    /// 能力声明（**唯一来源**）：`provider_capabilities()` 的表项就是它。
    ///
    /// 句边界恒为客户端提交 —— `gpt-live-transcribe` 不支持 server_vad，
    /// 「缺 VAD」不是「改用服务端断句」的理由（那是另一个能力：`needs_local_vad`）。
    pub const CAPABILITIES: Capabilities = Capabilities {
        endpoint_kind: EndpointKind::ClientCommit,
        needs_local_vad: true,
        partial_semantics: PartialSemantics::Delta,
        supports_keywords: true,
        target_sample_rate: TARGET_SAMPLE_RATE,
    };
}

impl CloudProvider for RealtimeProvider {
    fn capabilities(&self) -> Capabilities {
        Self::CAPABILITIES
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

    fn send_audio(&mut self, pcm: &[i16]) -> Result<SendOutcome, String> {
        if let Some(error) = self.take_background_error() {
            return Err(error);
        }
        let Some(commands) = self.commands.as_ref() else {
            return Err(crate::code("cloud.disconnected"));
        };
        let bytes: Vec<u8> = pcm.iter().flat_map(|sample| sample.to_le_bytes()).collect();
        match commands.try_send(Command::Audio(BASE64.encode(bytes))) {
            // 整块入队了：这次确实投递了。
            Ok(()) => Ok(SendOutcome::delivered()),
            Err(tokio_mpsc::error::TrySendError::Full(_)) => {
                // 队列满：这一段整段没送出去。调用方据此在结果里留缺口。
                let samples = pcm.len() * 16000 / TARGET_SAMPLE_RATE as usize;
                Ok(SendOutcome { dropped: samples, delivered: false })
            }
            Err(tokio_mpsc::error::TrySendError::Closed(_)) => Err(crate::code("cloud.disconnected")),
        }
    }

    fn commit(&mut self) -> Result<(), String> {
        if let Some(error) = self.take_background_error() {
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
        if let Some(error) = self.take_background_error() {
            return Err(error);
        }
        let Some(commands) = self.commands.as_ref() else {
            return Err(crate::code("cloud.disconnected"));
        };
        let (done_tx, done_rx) = channel();
        // 同 `commit`：收尾命令丢了就等于最后一句永远等不到结果。
        send_control(commands, Command::Finish(done_tx), CONTROL_SEND_TIMEOUT)?;
        match done_rx.recv_timeout(FINISH_TIMEOUT) {
            Ok(()) => self.take_background_error().map_or(Ok(()), Err),
            Err(_) => Err(self
                .take_background_error()
                .unwrap_or_else(|| crate::code("cloud.timeout"))),
        }
    }

    /// 后台错误：转发到 provider 自己的错误通道（**不递归**：这里调的
    /// 是固有方法 `take_background_error`，不是 trait 的同名方法）。
    fn take_error(&mut self) -> Option<String> {
        self.take_background_error()
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
    let mut tracker = CommitTracker::default();
    let mut finishing: Option<Sender<()>> = None;
    // **只有收尾才有超时**：录音本身没有时长上限（本地两条路径也没有）。
    // 之前这里挂着一个「建连 + 1 小时」的会话级 deadline，录满一小时就断连 ——
    // 那与「录音没有时长上限」的说法矛盾。
    let mut deadline: Option<tokio::time::Instant> = None;
    // 断句方式决定「还有没有在等的结果」怎么算。
    // 句边界方式取自能力声明（唯一来源）。
    let client_commit = RealtimeProvider::CAPABILITIES.endpoint_kind == EndpointKind::ClientCommit;
    // 中间结果归一器：按 item 分桶累计，句定稿时清掉自己的桶。
    let mut accumulator =
        partial::PartialAccumulator::new(RealtimeProvider::CAPABILITIES.partial_semantics);
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
                        // 本地提交了一次：记一个槽，等服务端的回执 / 结果。
                        tracker.commit();
                    }
                    Command::Finish(done) => {
                        // 没什么可等的就直接交差：提交一个空缓冲，服务端多半不
                        // 回结果，等下去只是白等 10 秒，还会被误报成 `cloud.timeout`。
                        if !pending.outstanding(client_commit, tracker.is_empty()) {
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
                            // 这次提交也要占一个槽：否则服务端为它回的 final
                            // 找不到对应槽 —— 最后一句会被丢掉，或被绑到前一句的
                            // 区间上（文本与时间互换）。
                            tracker.commit();
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
                        // 中间结果在 provider 内部归一为「当前句的累计串」，
                        // 上层只认这一种语义（见 `partial::PartialAccumulator`）。
                        let text = accumulator.apply(item_id.clone(), PartialUpdate::Delta(delta));
                        if !text.is_empty() {
                            on_event(CloudEvent::Partial(text));
                        }
                    }
                    ServerEvent::Final { item_id, transcript } => {
                        // 先算「还有没有在等的音频」，再更新 latest_item ——
                        // 顺序反了的话，这条 final 永远算「最新」。
                        pending.on_final(&item_id, client_commit);
                        pending.note_item(&item_id);
                        tracker.resolve(item_id.clone(), Outcome::Text(transcript));
                        for event in tracker.flush() {
                            on_event(event);
                        }
                        // 这一句定稿：清掉它自己的桶，再补发当前活跃句的实时文本 ——
                        // 前端收到定稿事件会清空实时行，迟到的旧句不该把新句
                        // 正在显示的内容一起清掉。
                        accumulator.finish_sentence(item_id);
                        if let Some(current) = accumulator.current() {
                            on_event(CloudEvent::Partial(current));
                        }
                        // 收尾时最后一条结果到了：可以交差了。
                        if finishing.is_some() && tracker.is_empty() {
                            if let Some(done) = finishing.take() {
                                let _ = done.send(());
                                break;
                            }
                        }
                    }
                    ServerEvent::Failed { item_id, reason } => {
                        // 失败也要留痕：这句文本会从结果里消失，日志里必须有
                        // 一条可追的记录（服务端给的原因在**关口**里脱敏 ——
                        // 关口是云端模块唯一的日志出口）。
                        let id = item_id.as_deref().unwrap_or("未知");
                        if reason.is_empty() {
                            super::logging::warn(
                                &format!("云端识别失败：item {id}"),
                                Some(&config.api_key),
                            );
                        } else {
                            super::logging::warn(
                                &format!("云端识别失败：item {id}，{reason}"),
                                Some(&config.api_key),
                            );
                        }
                        // 失败的一轮不会回 final：用空文本走同一条通道 ——
                        // 会话层照常弹出这一轮的区间，后续句子才不会错位。
                        pending.on_final(&item_id, client_commit);
                        pending.note_item(&item_id);
                        tracker.resolve(item_id.clone(), Outcome::Failed);
                        accumulator.finish_sentence(item_id);
                        for event in tracker.flush() {
                            on_event(event);
                        }
                        if finishing.is_some() && tracker.is_empty() {
                            if let Some(done) = finishing.take() {
                                let _ = done.send(());
                                break;
                            }
                        }
                    }
                    ServerEvent::SessionUpdated => {
                        if let Some(ready) = ready.take() { let _ = ready.send(Ok(())); }
                    }
                    ServerEvent::Committed { item_id } => {
                        pending.note_item(&item_id);
                        for event in tracker.committed(item_id) {
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
                        // 报错要把提交槽解开：否则槽永远悬着 → flush 不再往前推进
                        // → 这一句之后的所有 final 都被扣住（转写里静默少了后文），
                        // 收尾还要白等 10 秒。
                        tracker.resolve(None, Outcome::Failed);
                        for event in tracker.flush() {
                            on_event(event);
                        }
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

/// 外发热词按 Realtime 协议过滤：`<`、`>`、CR、LF 都不能进 keywords。
///
/// 这是 **OpenAI Realtime 的字符限制**，别家规则不同 —— 各 provider 在自己
/// 内部过滤（见 `cloud::cloud_keywords` 只负责「要不要带」）。
pub(crate) fn clean_cloud_keywords(words: &[String]) -> (Vec<String>, usize) {
    let mut clean = Vec::new();
    let mut ignored = 0;
    for word in words {
        if word.chars().any(|ch| matches!(ch, '<' | '>' | '\r' | '\n')) {
            ignored += 1;
        } else {
            clean.push(word.clone());
        }
    }
    (clean, ignored)
}

/// 默认 Realtime 转写模型只支持客户端提交；其它兼容模型必须显式走这个契约。
pub(crate) fn validate_realtime_model(model: &str) -> Result<(), String> {
    if model.trim().is_empty() {
        return Err(crate::code("cloud.unsupportedModel"));
    }
    Ok(())
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

/// 一次提交的结果。
#[derive(Clone)]
enum Outcome {
    /// 服务端返回了定稿文本。
    Text(String),
    /// 这一轮识别失败（服务端不会为它回 final）。
    Failed,
}

/// 一次本地提交在服务端那边的状态。
#[derive(Default)]
struct CommitSlot {
    /// 服务端给这一轮分配的 `item_id`（回执或失败事件里带来）。
    item_id: Option<String>,
    /// 回执发给会话层了没有（发了就不重复发）。
    acked: bool,
    /// 这一轮的结果。
    outcome: Option<Outcome>,
}

/// 提交跟踪：把服务端的回执 / 结果绑到**本地提交顺序**上。
///
/// **为什么不用「到达顺序」**：服务端的 final 到达顺序不保证（跨轮可能乱序），
/// 失败的那一轮更是可能永远没有 final。按到达顺序上抛，会话层按 FIFO 配
/// 区间就会整体错位一格 —— 后面每一句都抢到前一句的区间。
///
/// **回执仍按本地提交顺序绑定**（`committed` 绑到最早还没绑定的槽）：服务端
/// 回执也可能晚到或不到（连接异常），纯 id 配对反而无从下手。
/// 会话层拿到 id 后按 id 配区间，上抛顺序再怎么乱都不会错位。
#[derive(Default)]
struct CommitTracker {
    slots: VecDeque<CommitSlot>,
}

impl CommitTracker {
    /// 本地又提交了一次。
    fn commit(&mut self) {
        self.slots.push_back(CommitSlot::default());
    }

    /// 服务端确认了这一轮：把 `item_id` 绑到最早还没绑定的槽。
    ///
    /// 返回按提交顺序可以上抛的事件（回执优先）。
    fn committed(&mut self, item_id: Option<String>) -> Vec<CloudEvent> {
        let Some(item_id) = item_id else { return Vec::new() };
        if self
            .slots
            .iter()
            .any(|slot| slot.item_id.as_deref() == Some(item_id.as_str()))
        {
            return self.flush();
        }
        if let Some(slot) = self.slots.iter_mut().find(|slot| slot.item_id.is_none()) {
            slot.item_id = Some(item_id);
        }
        self.flush()
    }

    /// 一轮的结果到了：绑到对应的槽（按 id；找不到就绑最早的还没出结果的）。
    fn resolve(&mut self, item_id: Option<String>, outcome: Outcome) {
        if let Some(item_id) = &item_id {
            if let Some(slot) = self
                .slots
                .iter_mut()
                .find(|slot| slot.item_id.as_deref() == Some(item_id.as_str()))
            {
                if slot.outcome.is_none() {
                    slot.outcome = Some(outcome);
                }
                return;
            }
        }
        // 没有 id，或 id 对不上：绑到最早的还没出结果的槽。
        if let Some(slot) = self.slots.iter_mut().find(|slot| slot.outcome.is_none()) {
            if slot.item_id.is_none() {
                slot.item_id = item_id;
            }
            slot.outcome = Some(outcome);
        }
    }

    /// 按提交顺序上抛：先发回执（会话层靠它把区间绑到 id 上），再发结果。
    fn flush(&mut self) -> Vec<CloudEvent> {
        let mut out = Vec::new();
        while !self.slots.is_empty() {
            // 回执：必须按提交顺序发 —— 会话层按顺序把区间绑到 id 上。
            let ack = match self.slots.front() {
                Some(slot) if !slot.acked => slot.item_id.clone(),
                _ => None,
            };
            if let Some(item_id) = ack {
                if let Some(slot) = self.slots.front_mut() {
                    slot.acked = true;
                }
                out.push(CloudEvent::CommitAck {
                    item_id: Some(item_id),
                });
                continue;
            }
            // 结果：这一轮出结果了就上抛（失败走空文本的 final）。
            let Some(outcome) = self.slots.front().and_then(|slot| slot.outcome.clone()) else {
                break;
            };
            let item_id = self.slots.front().and_then(|slot| slot.item_id.clone());
            self.slots.pop_front();
            let text = match outcome {
                Outcome::Text(text) => text,
                // 失败也走 final 通道（空文本）：会话层照常弹出这一轮的区间、
                // 不产句子 —— 后面每一句的区间才不会错位。
                Outcome::Failed => String::new(),
            };
            out.push(CloudEvent::Final {
                item_id,
                text,
                start_ms: None,
                end_ms: None,
            });
        }
        out
    }

    /// 还没出结果的提交（收尾时用来判断能不能交差）。
    fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }
}

#[cfg(test)]
mod protocol_tests;
