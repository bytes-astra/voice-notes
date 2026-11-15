//! 火山引擎 / 豆包 双向流式 ASR provider。
//!
//! 协议（官方文档 2026-11 核对）：
//! - 端点：`wss://openspeech.bytedance.com/api/v3/sauc/bigmodel_async`
//!   （优化版：只有结果变化时才回包）；
//! - 鉴权是**多个 HTTP 头**：`X-Api-Key`、`X-Api-Resource-Id`、
//!   `X-Api-Request-Id`、`X-Api-Connect-Id`；
//! - 传输是**自研二进制帧**（见 `volcengine/frame.rs`）；
//! - 断句在**服务端**（`end_window_size` / `vad_segment_duration`）——
//!   没有客户端 commit、没有 item_id；
//! - 中间结果：`result.text` 是**全会话累计**，分句在 `result.utterances[]`
//!   （带 `definite` 标志）—— 所以实时文本要从 utterances 里取，不能用 `result.text`。
//!
//! 音频：16 kHz / 16 bit / 单声道 PCM（与仓库内部轴一致 → 不重采样）。

use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::mpsc as tokio_mpsc;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

use super::{Capabilities, CloudEvent, CloudProvider, EndpointKind, PartialSemantics};

mod frame;
use frame::ServerFrame;

/// 默认端点（双向流式优化版）。
pub const DEFAULT_BASE_URL: &str = "wss://openspeech.bytedance.com/api/v3/sauc/bigmodel_async";
/// 默认资源 ID（模型 2.0，按小时计费）。用户可在设置里改。
pub const DEFAULT_RESOURCE_ID: &str = "volc.seedasr.sauc.duration";
/// 双向流式的热词上限（官方文档：100 tokens）。
pub const MAX_KEYWORD_TOKENS: usize = 100;
/// 发包节奏：官方建议单包 100~200ms，双向流式 200ms 最优。
/// 16 kHz × 16 bit × 0.2 s = 6400 字节。
pub const AUDIO_PACKET_BYTES: usize = 6400;

/// 火山 provider 的配置。
#[derive(Clone)]
pub struct VolcengineConfig {
    /// 端点（留空 = 用默认）。
    pub base_url: String,
    /// `X-Api-Key`（留空 = 用默认）。
    pub api_key: String,
    /// `X-Api-Resource-Id`（留空 = 用默认）。
    pub resource_id: String,
    /// 术语表（作为 `corpus.context` 直传）。
    pub keywords: Vec<String>,
}

/// 后台任务收到的命令。
enum Command {
    /// 一段音频（原始 PCM16 字节）。
    Audio(Vec<u8>),
    /// 收尾：发末包并等最终响应。
    Finish(Sender<()>),
}

/// 火山 provider。
pub struct VolcengineProvider {
    config: VolcengineConfig,
    commands: Option<tokio_mpsc::Sender<Command>>,
    errors: Receiver<String>,
    error_tx: Sender<String>,
    /// 攒够一包（200ms）再发。
    packet: Vec<u8>,
}

impl VolcengineProvider {
    /// 能力声明（**唯一来源**）。
    ///
    /// 服务端断句 → 不需要本地 VAD、静音照上传；中间结果是「确认前缀加草稿」
    /// 形态（utterance 文本本身就是当前句的累计串）。
    pub const CAPABILITIES: Capabilities = Capabilities {
        endpoint_kind: EndpointKind::ServerEndpoint,
        needs_local_vad: false,
        partial_semantics: PartialSemantics::Preview,
        supports_keywords: true,
        target_sample_rate: 16_000,
    };

    pub fn new(config: VolcengineConfig) -> Self {
        let (error_tx, errors) = channel();
        Self {
            config,
            commands: None,
            errors,
            error_tx,
            packet: Vec::new(),
        }
    }

    /// 后台任务报的错（如果有）。
    ///
    /// **不叫 `take_error`**：trait 上有同名默认方法，固有方法会被它遮蔽
    /// （调用落回默认实现 → 恒为 `None` → 后台错误永远读不出来）。
    fn take_background_error(&self) -> Option<String> {
        self.errors.try_recv().ok()
    }
}

impl CloudProvider for VolcengineProvider {
    fn capabilities(&self) -> Capabilities {
        Self::CAPABILITIES
    }

    fn connect(
        &mut self,
        on_event: Box<dyn Fn(CloudEvent) + Send + Sync>,
    ) -> Result<(), String> {
        let (commands_tx, commands_rx) = tokio_mpsc::channel(64);
        let (ready_tx, ready_rx) = channel::<Result<(), String>>();
        let config = self.config.clone();
        let error_tx = self.error_tx.clone();
        tauri::async_runtime::spawn(async move {
            run(config, commands_rx, on_event, ready_tx, error_tx).await;
        });
        self.commands = Some(commands_tx);
        match ready_rx.recv_timeout(Duration::from_secs(15)) {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(crate::code("cloud.connectFailed")),
        }
    }

    fn send_audio(&mut self, pcm: &[i16]) -> Result<usize, String> {
        if let Some(error) = self.take_background_error() {
            return Err(error);
        }
        let Some(commands) = self.commands.as_ref() else {
            return Err(crate::code("cloud.disconnected"));
        };
        // 攒够 200ms 一包再发（官方建议的发包节奏）。
        self.packet
            .extend(pcm.iter().flat_map(|sample| sample.to_le_bytes()));
        while self.packet.len() >= AUDIO_PACKET_BYTES {
            let rest = self.packet.split_off(AUDIO_PACKET_BYTES);
            let packet = std::mem::replace(&mut self.packet, rest);
            if commands
                .try_send(Command::Audio(packet))
                .is_err()
            {
                return Err(crate::code("cloud.disconnected"));
            }
        }
        Ok(0)
    }

    fn commit(&mut self) -> Result<(), String> {
        // 服务端断句：没有客户端 commit 这回事。
        Ok(())
    }

    /// 后台错误：转发到 provider 自己的错误通道（**不递归**：这里调的
    /// 是固有方法 `take_background_error`，不是 trait 的同名方法）。
    fn take_error(&mut self) -> Option<String> {
        self.take_background_error()
    }

    fn finish(&mut self) -> Result<(), String> {
        if let Some(error) = self.take_background_error() {
            return Err(error);
        }
        let Some(commands) = self.commands.as_ref() else {
            return Err(crate::code("cloud.disconnected"));
        };
        let (done_tx, done_rx) = channel();
        commands
            .try_send(Command::Finish(done_tx))
            .map_err(|_| crate::code("cloud.disconnected"))?;
        match done_rx.recv_timeout(Duration::from_secs(10)) {
            Ok(()) => self.take_background_error().map_or(Ok(()), Err),
            Err(_) => Err(self
                .take_background_error()
                .unwrap_or_else(|| crate::code("cloud.timeout"))),
        }
    }
}

/// 热词按火山协议清洗：转义 JSON 里的引号与反斜杠、去掉控制字符。
///
/// **火山不禁止 `<` `>`**（那是 OpenAI Realtime 的限制）—— 但热词要序列化进
/// JSON 字符串，控制字符与引号必须转义，否则整个 `session.update`（这里是
/// 首次全量请求）会被拒。
pub(crate) fn clean_volcengine_keywords(words: &[String]) -> (Vec<String>, usize) {
    let mut clean = Vec::new();
    let mut ignored = 0;
    for word in words {
        if word.chars().any(|ch| ch.is_control()) {
            ignored += 1;
            continue;
        }
        clean.push(word.clone());
    }
    // 官方上限 100 tokens（双向流式）：超了截断。
    if clean.len() > MAX_KEYWORD_TOKENS {
        ignored += clean.len() - MAX_KEYWORD_TOKENS;
        clean.truncate(MAX_KEYWORD_TOKENS);
    }
    (clean, ignored)
}

/// 术语表 → `corpus.context`（JSON 字符串）。
fn encode_corpus_context(words: &[String]) -> Option<String> {
    if words.is_empty() {
        return None;
    }
    let hotwords: Vec<Value> = words
        .iter()
        .map(|word| json!({ "word": word }))
        .collect();
    Some(json!({ "hotwords": hotwords }).to_string())
}

/// 首次全量请求的 JSON 负载。
fn encode_full_request(config: &VolcengineConfig) -> Vec<u8> {
    let mut request = json!({
        "user": { "uid": "voicenotes" },
        "audio": { "format": "pcm", "rate": 16000, "bits": 16, "channel": 1 },
        "request": {
            "model_name": "bigmodel",
            "enable_itn": true,
            "enable_punc": true,
            // 分句信息（含 definite）必须显式开启。
            "show_utterances": true,
            "result_type": "full",
            // 服务端断句：静音多久判停。
            "end_window_size": 800,
        }
    });
    if let Some(context) = encode_corpus_context(&config.keywords) {
        request["request"]["corpus"] = json!({ "context": context });
    }
    serde_json::to_vec(&request).unwrap_or_default()
}

/// 把服务端的识别结果映射成事件。
///
/// - `utterances[]` 里 `definite=true` 的句子 → 定稿（Final）；
/// - 最后一条 `definite=false` 的句子 → 当前句的累计串（Partial）。
///
/// **`result.text` 不用**：它是全会话累计，当实时行会让整场文本滚动。
fn map_response(payload: &[u8], emitted: &mut usize) -> Vec<CloudEvent> {
    let Ok(value) = serde_json::from_slice::<Value>(payload) else {
        return Vec::new();
    };
    let Some(utterances) = value
        .get("result")
        .and_then(|result| result.get("utterances"))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    let mut events = Vec::new();
    for (index, utterance) in utterances.iter().enumerate() {
        if index < *emitted {
            continue;
        }
        let definite = utterance
            .get("definite")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let text = utterance
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if definite {
            *emitted = index + 1;
            events.push(CloudEvent::Final {
                item_id: None,
                text,
                start_ms: None,
                end_ms: None,
            });
        } else {
            // 当前句的累计串（草稿）：确认前缀为空，整句都是草稿。
            events.push(CloudEvent::Partial(text));
        }
    }
    events
}

/// 后台任务：建连 → 发首次请求 → 收音频 → 收结果 → 收尾。
async fn run(
    config: VolcengineConfig,
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
    // 首次全量请求：音频参数 + 断句参数 + 热词。
    let frame = frame::encode_full_request(&encode_full_request(&config));
    if let Err(error) = socket.send(Message::Binary(frame.into())).await {
        let error = crate::code_detail("cloud.connectFailed", error.to_string());
        let _ = ready.send(Err(error.clone()));
        let _ = errors.send(error);
        return;
    }
    let _ = ready.send(Ok(()));

    let mut emitted = 0usize;
    let mut finishing: Option<Sender<()>> = None;
    // 收尾：末包发出去后，等服务端的最终响应（sequence 为负）才算完。
    let mut pending_final = false;
    loop {
        tokio::select! {
            command = commands.recv() => {
                let Some(command) = command else { break };
                match command {
                    Command::Audio(packet) => {
                        let frame = frame::encode_audio(&packet, false);
                        if let Err(error) = socket.send(Message::Binary(frame.into())).await {
                            let _ = errors.send(crate::code_detail("cloud.sendFailed", error.to_string()));
                            break;
                        }
                    }
                    Command::Finish(done) => {
                        let frame = frame::encode_audio(&[], true);
                        if let Err(error) = socket.send(Message::Binary(frame.into())).await {
                            let _ = errors.send(crate::code_detail("cloud.sendFailed", error.to_string()));
                            let _ = done.send(());
                            break;
                        }
                        finishing = Some(done);
                        pending_final = true;
                    }
                }
            }
            message = socket.next() => {
                let Some(message) = message else {
                    let _ = errors.send(crate::code("cloud.disconnected"));
                    if let Some(done) = finishing.take() { let _ = done.send(()); }
                    break;
                };
                let raw = match message {
                    Ok(Message::Binary(raw)) => raw,
                    Ok(Message::Close(_)) => {
                        let _ = errors.send(crate::code("cloud.disconnected"));
                        if let Some(done) = finishing.take() { let _ = done.send(()); }
                        break;
                    }
                    Ok(_) => continue,
                    Err(error) => {
                        let _ = errors.send(crate::code_detail("cloud.disconnected", error.to_string()));
                        if let Some(done) = finishing.take() { let _ = done.send(()); }
                        break;
                    }
                };
                match frame::decode_server_frame(&raw) {
                    Ok(ServerFrame::Response { sequence, payload }) => {
                        for event in map_response(&payload, &mut emitted) {
                            on_event(event);
                        }
                        // 最终响应：sequence 为负数。
                        if pending_final && sequence < 0 {
                            pending_final = false;
                            if let Some(done) = finishing.take() { let _ = done.send(()); break; }
                        }
                    }
                    Ok(ServerFrame::Error { code, message }) => {
                        let message = sanitize(&message, &config.api_key);
                        let error = match code {
                            45000002 => crate::code("cloud.noResult"),
                            45000081 => crate::code("cloud.timeout"),
                            _ => crate::code_detail("cloud.protocolError", format!("{code}: {message}")),
                        };
                        let _ = errors.send(error);
                        if let Some(done) = finishing.take() { let _ = done.send(()); }
                        break;
                    }
                    Err(error) => {
                        let _ = errors.send(crate::code_detail("cloud.protocolError", error));
                        if let Some(done) = finishing.take() { let _ = done.send(()); }
                        break;
                    }
                }
            }
        }
    }
    if let Some(done) = finishing.take() {
        let _ = done.send(());
    }
}

/// 连上火山：请求头带鉴权信息（凭据只进请求头，URL 与日志里都不出现）。
async fn connect_socket(
    config: &VolcengineConfig,
) -> Result<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    String,
> {
    let base_url = if config.base_url.trim().is_empty() {
        DEFAULT_BASE_URL
    } else {
        config.base_url.trim()
    };
    let mut request = base_url
        .into_client_request()
        .map_err(|error| crate::code_detail("cloud.connectFailed", error.to_string()))?;
    let resource_id = if config.resource_id.trim().is_empty() {
        DEFAULT_RESOURCE_ID
    } else {
        config.resource_id.trim()
    };
    let request_id = new_request_id();
    let connect_id = new_request_id();
    for (name, value) in [
        ("X-Api-Key", config.api_key.as_str()),
        ("X-Api-Resource-Id", resource_id),
        ("X-Api-Request-Id", request_id.as_str()),
        ("X-Api-Connect-Id", connect_id.as_str()),
    ] {
        let header = value
            .parse()
            .map_err(|_| crate::code("cloud.connectFailed"))?;
        request.headers_mut().insert(name, header);
    }
    let (socket, _) = tokio_tungstenite::connect_async(request)
        .await
        .map_err(|error| {
            crate::code_detail("cloud.connectFailed", sanitize(&error.to_string(), &config.api_key))
        })?;
    Ok(socket)
}

/// 请求 ID：UUID v4（官方建议用 UUID 追踪调用）。
fn new_request_id() -> String {
    let mut bytes = [0u8; 16];
    // 用时间与进程 id 拼一个够用的随机串（不必是密码学随机）。
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let pid = std::process::id() as u64;
    let mut state = now ^ (pid << 32);
    for byte in bytes.iter_mut() {
        // xorshift：够随机即可（只是调用追踪标识，不是凭据）。
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        *byte = (state & 0xff) as u8;
    }
    // 版本 4 + 变体位。
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// 把凭据从文本里抹掉。
fn sanitize(text: &str, secret: &str) -> String {
    if secret.is_empty() {
        return text.to_string();
    }
    text.replace(secret, "***")
}

#[cfg(test)]
mod tests;
