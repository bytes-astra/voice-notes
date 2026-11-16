//! 阿里云百炼（DashScope / Model Studio）实时语音识别 `fun-asr-realtime`。
//!
//! 协议（官方文档 2026-11 核对）：**JSON 文本帧做控制 + 裸二进制帧做音频**。
//! 与火山的区别是没有自研二进制协议头 —— `Message::Text` / `Message::Binary`
//! 直接对应，不需要帧编解码模块。
//!
//! ```text
//! 建连（Authorization: Bearer <key>）
//!   → run-task（文本帧）→ task-started
//!   → 持续发裸 PCM（二进制帧）
//!   → result-generated（多次）
//!   → finish-task（文本帧）
//!   → 继续 result-generated（尾部结果）→ task-finished → 关连接
//! ```
//!
//! 三条必须照做，否则会出错：必须等 `task-started` 才发音频；`finish-task`
//! 之后要继续收到 `task-finished`（发完就关会丢尾部结果）；`sentence_end=true`
//! 只表示句子结束，任务结束以 `task-finished` 为准。

use std::collections::HashSet;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::mpsc as tokio_mpsc;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

use super::{
    sanitize, Capabilities, CloudEvent, CloudProvider, EndpointKind, PartialSemantics, SendOutcome,
};

/// 默认端点（公共地址，开箱可用；可换成业务空间专属域名）。
pub const DEFAULT_BASE_URL: &str = "wss://dashscope.aliyuncs.com/api-ws/v1/inference";
/// 默认模型。
pub const DEFAULT_MODEL: &str = "fun-asr-realtime";
/// 术语表上下文的上限：每轮 `input_text` 与 `text` 合计不超过 400 字符
/// （官方 `fun-asr-client-events` 的 `input` 字段说明）。
pub const MAX_CONTEXT_CHARS: usize = 400;
/// 发包节奏：官方建议每包 100 ms 左右。16 kHz × 16 bit × 0.1 s = 3200 字节。
pub const AUDIO_PACKET_BYTES: usize = 3200;
/// 断句静音阈值（毫秒）。官方两处文档的默认值不一致（1300 / 800），**显式设**。
const MAX_SENTENCE_SILENCE_MS: u32 = 1300;
/// 控制帧排队等待的上限。
const CONTROL_SEND_TIMEOUT: Duration = Duration::from_secs(3);
/// 8k 模型的已知前缀（要求 8000 Hz 采样率，与静态能力表冲突 → 拒绝）。
pub const EIGHT_K_MODEL_PREFIX: &str = "fun-asr-flash-8k-realtime";
/// 术语表上下文的前缀。
const CONTEXT_PREFIX: &str = "术语表：";

/// 阿里 provider 的配置。
#[derive(Clone)]
pub struct AliyunConfig {
    /// 端点（留空 = 用默认）。
    pub base_url: String,
    /// API key（`Authorization: Bearer`）。
    pub api_key: String,
    /// 模型名（留空 = 用默认）。
    pub model: String,
    /// `language_hints` 的单个值（`None` = 不传，让模型自动识别）。
    pub language_hint: Option<String>,
    /// 术语表拼成的上下文文本（`None` = 不带 `input.context`）。
    pub context_text: Option<String>,
}

impl AliyunConfig {
    fn base_url(&self) -> &str {
        if self.base_url.trim().is_empty() {
            DEFAULT_BASE_URL
        } else {
            self.base_url.trim()
        }
    }

    fn model(&self) -> &str {
        if self.model.trim().is_empty() {
            DEFAULT_MODEL
        } else {
            self.model.trim()
        }
    }
}

/// 后台任务收到的命令。
enum Command {
    /// 一段音频（原始 PCM16 字节）。
    Audio(Vec<u8>),
    /// 收尾：发 `finish-task` 并等 `task-finished`。
    Finish(Sender<()>),
}

/// 一条服务端 `sentence`。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sentence {
    pub id: i64,
    pub begin: bool,
    pub end: bool,
    pub heartbeat: bool,
    pub text: String,
}

/// 服务端事件。
#[derive(Debug)]
pub enum ServerEvent {
    TaskStarted,
    Sentence(Sentence),
    TaskFinished,
    TaskFailed { code: String, message: String },
    /// 未知事件：向前兼容，不报错。
    Other,
}

/// 阿里 provider。
pub struct AliyunProvider {
    config: AliyunConfig,
    commands: Option<tokio_mpsc::Sender<Command>>,
    errors: Receiver<String>,
    error_tx: Sender<String>,
    /// 攒够一包（100 ms）再发。
    packet: Vec<u8>,
}

impl AliyunProvider {
    /// 能力声明（**唯一来源**）。
    ///
    /// 服务端断句 → 不需要本地 VAD、静音照上传。中间结果是「确认前缀为空、
    /// 整句是草稿」—— 即 `Preview` 的退化形态。
    ///
    /// **注意**：这个声明当前没有运行时消费者（阿里与火山都不走 `partial.rs`
    /// 的归一器，`Status` 也不回传它）。它的用处是让「声明形态 = 实际协议
    /// 形态」可被单测核对。
    pub const CAPABILITIES: Capabilities = Capabilities {
        endpoint_kind: EndpointKind::ServerEndpoint,
        needs_local_vad: false,
        partial_semantics: PartialSemantics::Preview,
        supports_keywords: true,
        target_sample_rate: 16_000,
    };

    pub fn new(config: AliyunConfig) -> Self {
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

impl CloudProvider for AliyunProvider {
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
        // 等 `task-started` 才算就绪：不等就发音频会被服务端丢弃。
        match ready_rx.recv_timeout(Duration::from_secs(15)) {
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
        // 攒够 100ms 一包再发（官方建议的发包节奏）。
        self.packet
            .extend(pcm.iter().flat_map(|sample| sample.to_le_bytes()));
        let mut dropped_samples = 0usize;
        let mut delivered = false;
        while self.packet.len() >= AUDIO_PACKET_BYTES {
            let rest = self.packet.split_off(AUDIO_PACKET_BYTES);
            let packet = std::mem::replace(&mut self.packet, rest);
            match commands.try_send(Command::Audio(packet)) {
                Ok(()) => delivered = true,
                // 队列满 = 网络背压，不是断连：这一包丢掉，但要**报回去**，
                // 让会话把它记成缺口（静默丢音频是项目明令禁止的）。
                Err(tokio_mpsc::error::TrySendError::Full(_)) => {
                    dropped_samples += AUDIO_PACKET_BYTES / 2;
                }
                Err(tokio_mpsc::error::TrySendError::Closed(_)) => {
                    return Err(crate::code("cloud.disconnected"));
                }
            }
        }
        // **只是攒进包里、一个包都没发出去 ≠ 投递了**：会话层靠这个信号判断
        // 「丢包段是否结束」—— 攒包时若当成投递，正在累积的丢包段会被提前闭合。
        Ok(SendOutcome {
            dropped: dropped_samples,
            delivered,
        })
    }

    fn commit(&mut self) -> Result<(), String> {
        // 服务端断句：没有客户端 commit 这回事。
        Ok(())
    }

    /// 后台错误：转发到固有方法（**不递归**）。
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
        // 收尾前把不足一包的残包发出去：留在缓冲里等于最后 0~99 ms 永不上传
        // → 句尾少字。控制帧不能丢 → 有界等待。
        if !self.packet.is_empty() {
            let packet = std::mem::take(&mut self.packet);
            send_control(commands, Command::Audio(packet), CONTROL_SEND_TIMEOUT)?;
        }
        let (done_tx, done_rx) = channel();
        send_control(commands, Command::Finish(done_tx), CONTROL_SEND_TIMEOUT)?;
        match done_rx.recv_timeout(Duration::from_secs(10)) {
            Ok(()) => self.take_background_error().map_or(Ok(()), Err),
            Err(_) => Err(self
                .take_background_error()
                .unwrap_or_else(|| crate::code("cloud.timeout"))),
        }
    }
}

/// 控制帧（残包 / 收尾命令）的有界等待。
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

// ── 协议层：纯函数，可单测 ──────────────────────────────────────────

/// 把应用的语言设置映射成 `language_hints` 的单个值。
///
/// - `auto` → 不传（官方：不设置时模型自动识别）
/// - `yue` → `zh`（**阿里没有 `yue` 这个 code**，粤语靠 `zh` 模型自动识别）
/// - `en` / `ja` / `ko` → 同名 code
/// - 其它 → 不传（宁可让模型自动判断，也不要传一个非法值）
///
/// 只返回一个值：官方说「设置多个时仅第一个生效」，传多个是误导。
pub fn language_hint(language: &str) -> Option<String> {
    match language.trim() {
        "zh" => Some("zh".into()),
        "yue" => Some("zh".into()),
        "en" => Some("en".into()),
        "ja" => Some("ja".into()),
        "ko" => Some("ko".into()),
        _ => None,
    }
}

/// `run-task` 的请求体。
pub fn encode_run_task(config: &AliyunConfig, task_id: &str) -> String {
    let mut parameters = json!({
        "format": "pcm",
        "sample_rate": 16_000,
        "max_sentence_silence": MAX_SENTENCE_SILENCE_MS,
        // 长静音也保活：官方说 false 时即使持续发静音也会在一定时间后断开。
        "heartbeat": true,
    });
    if let Some(hint) = &config.language_hint {
        // **只放一个值**：官方说「设置多个时仅第一个生效」。
        parameters["language_hints"] = json!([hint]);
    }
    // 上下文（术语表）：只有带内容时才出现这个字段，否则 `input` 是 `{}`。
    let input = match &config.context_text {
        Some(text) => json!({
            "context": [{
                "role": "user",
                "content": [{ "type": "input_text", "text": text }],
            }],
        }),
        None => json!({}),
    };
    json!({
        "header": {
            "action": "run-task",
            "task_id": task_id,
            "streaming": "duplex",
        },
        "payload": {
            "task_group": "audio",
            "task": "asr",
            "function": "recognition",
            "model": config.model(),
            "input": input,
            "parameters": parameters,
        },
    })
    .to_string()
}

/// `finish-task` 的请求体（**复用同一个 `task_id`**，`payload.input` 为空对象）。
pub fn encode_finish_task(task_id: &str) -> String {
    json!({
        "header": {
            "action": "finish-task",
            "task_id": task_id,
            "streaming": "duplex",
        },
        "payload": { "input": {} },
    })
    .to_string()
}

/// 术语表 → `input.context` 的文本。
///
/// **按字符数（不是字节数）** 度量：中文一个字算一个字符。上限 400 含前缀与
/// 分隔符，超限时**在术语边界上丢整条**（不把一个词截成半个）。
///
/// 返回 `(文本, 被忽略的条数)`；词表为空时返回 `(None, 0)`。
pub fn build_context(words: &[String]) -> (Option<String>, usize) {
    let usable: Vec<&str> = words
        .iter()
        .map(|word| word.trim())
        .filter(|word| !word.is_empty())
        .collect();
    if usable.is_empty() {
        return (None, 0);
    }
    let mut text = String::from(CONTEXT_PREFIX);
    let mut used = 0usize;
    for (index, word) in usable.iter().enumerate() {
        // 分隔符按「不是第一条」算，所以第一条不加「、」。
        let piece = if index == 0 {
            word.to_string()
        } else {
            format!("、{word}")
        };
        if text.chars().count() + piece.chars().count() > MAX_CONTEXT_CHARS {
            break;
        }
        text.push_str(&piece);
        used = index + 1;
    }
    // 一条都放不下：不带上下文（总比带个只有前缀的空壳好）。
    if used == 0 {
        return (None, usable.len());
    }
    (Some(text), usable.len() - used)
}

/// 解码一条服务端文本帧。
pub fn decode_server_event(raw: &str) -> Result<ServerEvent, String> {
    let value: Value = serde_json::from_str(raw)
        .map_err(|error| crate::code_detail("cloud.protocolError", error.to_string()))?;
    let header = value.get("header").cloned().unwrap_or(Value::Null);
    let event = header.get("event").and_then(Value::as_str).unwrap_or("");
    Ok(match event {
        "task-started" => ServerEvent::TaskStarted,
        "task-finished" => ServerEvent::TaskFinished,
        "task-failed" => ServerEvent::TaskFailed {
            code: header
                .get("error_code")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            message: header
                .get("error_message")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        },
        "result-generated" => match value
            .get("payload")
            .and_then(|payload| payload.get("output"))
            .and_then(|output| output.get("sentence"))
        {
            Some(sentence) => ServerEvent::Sentence(parse_sentence(sentence)),
            // 没有 sentence 的 result-generated（理论上不该有）：当成未知事件。
            None => ServerEvent::Other,
        },
        // 未知事件不报错：向前兼容。
        _ => ServerEvent::Other,
    })
}

fn parse_sentence(value: &Value) -> Sentence {
    Sentence {
        // `sentence_id` 是 integer，正常结果从 1 递增，心跳固定 0。
        id: value.get("sentence_id").and_then(Value::as_i64).unwrap_or(0),
        begin: value
            .get("sentence_begin")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        end: value
            .get("sentence_end")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        heartbeat: value
            .get("heartbeat")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        text: value
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
    }
}

/// 把 `sentence` 归一成 `CloudEvent` —— **有状态**，因为要跨事件跟踪
/// 「当前未定稿句」与「已定稿的句 id」。
///
/// 不做成纯函数的原因见 design D2b/D2c：新句开始而上一句还没定稿时，必须
/// 先把上一句按最后快照定稿 —— 否则会话层的 `Partial` 替换语义会让上一句
/// 的文字永久消失。
#[derive(Debug, Default)]
pub struct AliyunMapper {
    /// 当前未定稿句：`(sentence_id, 最后一次非空快照)`。
    open: Option<(i64, String)>,
    /// 已定稿的句 id：它们的后续事件一律忽略（防重复 Final / 迟到 end
    /// 把新句的实时行清掉）。
    finalized: HashSet<i64>,
}

impl AliyunMapper {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn map_sentence(&mut self, sentence: Sentence) -> Vec<CloudEvent> {
        let mut events = Vec::new();
        // ① 心跳先判：它的 `sentence_id` 固定为 0，若先判「新 id」会误把
        //    上一句提前定稿。
        if sentence.heartbeat {
            return events;
        }
        // ② 已定稿的句：后续事件一律忽略（文本不同说明服务端改了口径，记 WARN）。
        if self.finalized.contains(&sentence.id) {
            if !sentence.text.trim().is_empty() {
                crate::log::warn("阿里云端：已定稿的句子又收到了新的文本，已忽略");
            }
            return events;
        }
        let text = sentence.text.trim();
        // ③ 新句开始而上一句还没定稿 → 先按最后快照定稿上一句。
        if let Some((open_id, open_text)) = self.open.as_ref() {
            if *open_id != sentence.id {
                let (id, snapshot) = (*open_id, open_text.clone());
                self.finalized.insert(id);
                self.open = None;
                events.push(final_event(snapshot));
            }
        }
        // ④ 空文本：不产 Partial；若这句就此结束，用已有快照定稿（没有就什么都不产）。
        if text.is_empty() {
            if sentence.end {
                let snapshot = self
                    .open
                    .take()
                    .filter(|(open_id, _)| *open_id == sentence.id)
                    .map(|(_, snapshot)| snapshot);
                self.finalized.insert(sentence.id);
                if let Some(snapshot) = snapshot {
                    events.push(final_event(snapshot));
                }
            }
            return events;
        }
        if sentence.end {
            self.finalized.insert(sentence.id);
            self.open = None;
            events.push(final_event(text.to_string()));
        } else {
            // 整句快照：替换不追加。
            self.open = Some((sentence.id, text.to_string()));
            events.push(CloudEvent::Partial(text.to_string()));
        }
        events
    }
}

fn final_event(text: String) -> CloudEvent {
    // **不传 `start_ms` / `end_ms`**：会话层的 `Final` 分支不读这两个字段，
    // 句子区间一律按本地 16 kHz 轴算。传了不产生效果，反而会让人以为
    // provider 的时间戳被采用了。
    CloudEvent::Final {
        item_id: None,
        text,
        start_ms: None,
        end_ms: None,
    }
}

// ── 后台任务 ────────────────────────────────────────────────────────

/// 后台任务：建连 → 发 `run-task` → 等 `task-started` → 收音频 → 收结果
/// → 收尾等 `task-finished`。
async fn run(
    config: AliyunConfig,
    mut commands: tokio_mpsc::Receiver<Command>,
    on_event: Box<dyn Fn(CloudEvent) + Send + Sync>,
    ready: Sender<Result<(), String>>,
    errors: Sender<String>,
) {
    let task_id = super::new_uuid();
    let mut socket = match connect_socket(&config).await {
        Ok(socket) => socket,
        Err(error) => {
            let _ = ready.send(Err(error.clone()));
            let _ = errors.send(error);
            return;
        }
    };
    // 立刻发 run-task；**等 task-started 才算就绪**（发音频前必须就绪）。
    if let Err(error) = socket
        .send(Message::Text(encode_run_task(&config, &task_id).into()))
        .await
    {
        let error = crate::code_detail("cloud.sendFailed", error.to_string());
        let _ = ready.send(Err(error.clone()));
        let _ = errors.send(error);
        return;
    }

    let mut mapper = AliyunMapper::new();
    let mut finishing: Option<Sender<()>> = None;
    let mut started = false;
    loop {
        tokio::select! {
            command = commands.recv() => {
                let Some(command) = command else { break };
                match command {
                    Command::Audio(packet) => {
                        if let Err(error) = socket.send(Message::Binary(packet.into())).await {
                            let _ = errors.send(crate::code_detail("cloud.sendFailed", error.to_string()));
                            break;
                        }
                    }
                    Command::Finish(done) => {
                        let payload = encode_finish_task(&task_id);
                        if let Err(error) = socket.send(Message::Text(payload.into())).await {
                            let _ = errors.send(crate::code_detail("cloud.sendFailed", error.to_string()));
                            let _ = done.send(());
                            break;
                        }
                        // 发完**不能关连接**：要继续收尾部结果，直到 task-finished。
                        finishing = Some(done);
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
                    Ok(Message::Text(raw)) => raw,
                    Ok(Message::Close(_)) => {
                        // 没收到 task-finished 就关了 → 异常结束。
                        if finishing.is_some() {
                            let _ = errors.send(crate::code("cloud.disconnected"));
                        }
                        if let Some(done) = finishing.take() { let _ = done.send(()); }
                        break;
                    }
                    // 服务端不该发二进制帧；忽略而不是报错。
                    Ok(_) => continue,
                    Err(error) => {
                        let _ = errors.send(crate::code_detail("cloud.disconnected", error.to_string()));
                        if let Some(done) = finishing.take() { let _ = done.send(()); }
                        break;
                    }
                };
                match decode_server_event(&raw) {
                    Ok(ServerEvent::TaskStarted) => {
                        started = true;
                        let _ = ready.send(Ok(()));
                    }
                    Ok(ServerEvent::Sentence(sentence)) => {
                        for event in mapper.map_sentence(sentence) {
                            on_event(event);
                        }
                    }
                    Ok(ServerEvent::TaskFinished) => {
                        if let Some(done) = finishing.take() { let _ = done.send(()); }
                        break;
                    }
                    Ok(ServerEvent::TaskFailed { code, message }) => {
                        let detail = format!("{code}: {}", sanitize(&message, &config.api_key));
                        let _ = errors.send(crate::code_detail("cloud.protocolError", detail));
                        if let Some(done) = finishing.take() { let _ = done.send(()); }
                        break;
                    }
                    Ok(ServerEvent::Other) => {}
                    Err(error) => {
                        let _ = errors.send(error);
                        if let Some(done) = finishing.take() { let _ = done.send(()); }
                        break;
                    }
                }
            }
        }
    }
    // 连上了但一直没等到 task-started：别让 connect() 白等 15 秒才报错。
    if !started {
        let _ = ready.send(Err(crate::code("cloud.connectFailed")));
    }
    if let Some(done) = finishing.take() {
        let _ = done.send(());
    }
}

/// 连上阿里：`Authorization: Bearer <key>`（凭据只进请求头）。
async fn connect_socket(
    config: &AliyunConfig,
) -> Result<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    String,
> {
    let mut request = config
        .base_url()
        .into_client_request()
        .map_err(|error| crate::code_detail("cloud.connectFailed", error.to_string()))?;
    let header = format!("Bearer {}", config.api_key)
        .parse()
        .map_err(|_| crate::code("cloud.connectFailed"))?;
    request.headers_mut().insert("Authorization", header);
    let (socket, _) = tokio_tungstenite::connect_async(request)
        .await
        .map_err(|error| {
            // 握手 401/403 也走这里：官方没给出更细的判据，统一映射成鉴权失败
            // 之外的连接失败会掩盖「key 不对」这个最常见的原因。
            crate::code_detail(
                "cloud.connectFailed",
                sanitize(&error.to_string(), &config.api_key),
            )
        })?;
    Ok(socket)
}

#[cfg(test)]
mod tests;
