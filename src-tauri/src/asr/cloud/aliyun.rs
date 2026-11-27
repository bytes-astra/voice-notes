//! 阿里云百炼（DashScope / Model Studio）实时语音识别
//! `qwen-audio-3.1-asr-flash-streaming`。
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

use crate::error_code::ErrorCode;
use std::collections::HashMap;
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
/// 默认模型：Qwen-Audio 3.1 实时流式版（百炼当前首推的实时 ASR 模型）。
pub const DEFAULT_MODEL: &str = "qwen-audio-3.1-asr-flash-streaming";
/// 支持的模型白名单。**协议骨架相同、但参数与结果结构不同**（本模型有
/// `stash` 草稿、即时热词、方言开关），所以不按「名字随便填」处理。
pub const SUPPORTED_MODELS: [&str; 1] = [DEFAULT_MODEL];
/// 即时热词条数上限（官方：与预编译热词合并后不超过 2000 个）。
pub const MAX_VOCABULARY: usize = 2000;
/// 即时热词的默认权重（官方建议从 4 起调）。
pub const DEFAULT_HOTWORD_WEIGHT: i32 = 4;
/// 热词文本上限（含非 ASCII 时，ASCII 与非 ASCII 合计的字符数）。
const MAX_HOTWORD_CHARS: usize = 15;
/// 纯 ASCII 热词的片段上限（按空格切分）。
const MAX_HOTWORD_ASCII_PARTS: usize = 7;
/// 发包节奏：官方建议每包 100 ms 左右。16 kHz × 16 bit × 0.1 s = 3200 字节
/// （官方 WebSocket 接入示例就是 3200 字节一包）。
pub const AUDIO_PACKET_BYTES: usize = 3200;
/// 断句静音阈值（毫秒）。本模型官方默认 **800**，
/// **显式设**，不依赖默认值。
const MAX_SENTENCE_SILENCE_MS: u32 = 800;
/// 保留方言表达。官方默认 `false`（把方言转写为普通话文本）—— 我们是录音工具，
/// 用户要的是「他实际说了什么」，所以保留方言写法。
const KEEP_DIALECT: bool = true;
/// VAD 模型：桌面麦克风是近场（官方默认是远场会议场景）。
const VAD_MODEL: &str = "near_meeting_16k";
/// 控制帧排队等待的上限。
const CONTROL_SEND_TIMEOUT: Duration = Duration::from_secs(3);

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
    /// 即时热词：`(词, 权重)`。请求级直传，不需要预建词表。
    pub vocabulary: Vec<(String, i32)>,
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
///
/// 本模型（Qwen-Audio-ASR-Streaming）的结果结构是**「已确认句子 + 草稿」**：
/// `text` 是已确认的句子文本，`stash.text` 是当前还在变的草稿（属于下一句的
/// `sentence_id`）。实时行文本 = `text` + `stash.text`。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sentence {
    /// 服务端给的句标识。**缺字段时是 `None`** —— 不能拿 0 顶替：官方口径里
    /// 0 是心跳的 id，两条都缺 id 的句子会共用 0，被当成同一句（第二句被吞）。
    pub id: Option<i64>,
    /// 这句是否已结束（`sentence_end`）。
    pub end: bool,
    /// 已确认的句子文本。
    pub text: String,
    /// 草稿文本（`stash.text`）—— 还在变的那部分。
    pub draft: String,
    /// 草稿所属的句标识（`stash.sentence_id`）。
    pub draft_id: Option<i64>,
    pub heartbeat: bool,
}

/// 服务端事件。
#[derive(Debug)]
pub enum ServerEvent {
    TaskStarted,
    Sentence(Sentence),
    TaskFinished,
    TaskFailed {
        code: String,
        message: String,
    },
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

    fn connect(&mut self, on_event: Box<dyn Fn(CloudEvent) + Send + Sync>) -> Result<(), String> {
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
            Ok(Err(error)) => {
                // 失败时把命令通道清掉：留着的话 provider 是「半可用」状态，
                // 调用方若忽略 Err 继续 send_audio，音频会在 task-started 之前
                // 发给服务端（违反协议）。
                self.commands = None;
                Err(error)
            }
            Err(_) => {
                self.commands = None;
                Err(crate::code(ErrorCode::CloudConnectFailed))
            }
        }
    }

    fn send_audio(&mut self, pcm: &[i16]) -> Result<SendOutcome, String> {
        if let Some(error) = self.take_background_error() {
            return Err(error);
        }
        let Some(commands) = self.commands.as_ref() else {
            return Err(crate::code(ErrorCode::CloudDisconnected));
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
                    return Err(crate::code(ErrorCode::CloudDisconnected));
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
            return Err(crate::code(ErrorCode::CloudDisconnected));
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
                .unwrap_or_else(|| crate::code(ErrorCode::CloudTimeout))),
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
                    return Err(crate::code(ErrorCode::CloudSendFailed));
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(tokio_mpsc::error::TrySendError::Closed(_)) => {
                return Err(crate::code(ErrorCode::CloudDisconnected));
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
/// 只返回一个值：官方说「最多支持设置 4 个值，超出时仅前 4 个生效」，
/// 我们只给一个，别传一串。
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
        // 保留方言表达（官方默认 false = 转成普通话文本）。
        "keep_dialect": KEEP_DIALECT,
        // 近场 VAD（官方默认远场会议场景；我们是桌面麦克风）。
        "vad_model": VAD_MODEL,
    });
    if let Some(hint) = &config.language_hint {
        // **只放一个值**：官方说最多 4 个值、只取前 4 个；我们只传一个。
        parameters["language_hints"] = json!([hint]);
    }
    // 即时热词：键值对（词 → 权重），请求级直传，不需要预建词表。
    if !config.vocabulary.is_empty() {
        let mut map = serde_json::Map::new();
        for (word, weight) in &config.vocabulary {
            map.insert(word.clone(), json!(weight));
        }
        parameters["vocabulary"] = serde_json::Value::Object(map);
    }
    // 本 provider 不带上下文：`input` 恒为 `{}`。
    let input = json!({});
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

/// 术语表 → 即时热词（`词, 权重`）。
///
/// **请求级直传**（官方 `parameters.vocabulary`）：键为热词文本、值为权重
/// （1–5，50 是超级热词且最多 50 个）。不需要预建词表、不需要额外 HTTP API。
///
/// 权重统一取 4（官方建议从 4 起调；界面没有权重输入，先给一个中间值）。
/// 超上限时**整条丢弃**（不截半个词），返回被忽略的条数。
pub fn build_vocabulary(words: &[String]) -> (Vec<(String, i32)>, usize) {
    let mut vocabulary: Vec<(String, i32)> = Vec::new();
    let mut ignored = 0usize;
    for word in words {
        let word = word.trim();
        if word.is_empty() {
            continue;
        }
        // 官方热词文本规范：含非 ASCII 字符时合计不超过 15 个字符；纯 ASCII
        // 按空格切分不超过 7 个片段。超了服务端可能整条作废 → 直接丢掉。
        if !hotword_text_fits(word) {
            ignored += 1;
            continue;
        }
        vocabulary.push((word.to_string(), DEFAULT_HOTWORD_WEIGHT));
    }
    if vocabulary.len() > MAX_VOCABULARY {
        ignored += vocabulary.len() - MAX_VOCABULARY;
        vocabulary.truncate(MAX_VOCABULARY);
    }
    (vocabulary, ignored)
}

/// 热词文本是否符合官方长度规范。
///
/// 官方「热词文本规范」：含非 ASCII 字符时，ASCII 与非 ASCII 合计不超过
/// **15 个字符**（空格计入）；仅 ASCII 时，按空格切分不超过 **7 个片段**。
fn hotword_text_fits(word: &str) -> bool {
    let has_non_ascii = !word.is_ascii();
    if has_non_ascii {
        word.chars().count() <= MAX_HOTWORD_CHARS
    } else {
        word.split_whitespace().count() <= MAX_HOTWORD_ASCII_PARTS
    }
}

/// 解码一条服务端文本帧。
pub fn decode_server_event(raw: &str) -> Result<ServerEvent, String> {
    let value: Value = serde_json::from_str(raw).map_err(|error| {
        super::logging::code_detail(ErrorCode::CloudProtocolError, error.to_string(), None)
    })?;
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
    let stash = value.get("stash");
    Sentence {
        // `sentence_id` 是 integer，正常结果从 1 递增，心跳固定 0。
        // **缺字段时留 None**（不用 0 顶替 —— 0 是心跳的 id）。
        id: value.get("sentence_id").and_then(Value::as_i64),
        end: value
            .get("sentence_end")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        text: value
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        // 草稿：`stash.text`（下一句还在变的那部分）。
        draft: stash
            .and_then(|stash| stash.get("text"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        draft_id: stash
            .and_then(|stash| stash.get("sentence_id"))
            .and_then(Value::as_i64),
        heartbeat: value
            .get("heartbeat")
            .and_then(Value::as_bool)
            .unwrap_or(false),
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
    /// 当前未定稿句：`(已确认句的 id, 已确认文本快照)`。
    ///
    /// **键必须是「快照文本所属的那一句」的 id** —— 草稿属于下一句，混用会导致
    /// 下一个事件被误判成「新句」→ 提前定稿 + 把下一句的 id 写进 `finalized`
    /// → 下一句整句被吞。
    open: Option<(Option<i64>, String)>,
    /// 当前草稿：`(草稿所属句的 id, 草稿文本)`。
    ///
    /// 草稿可能是**下一句**的（官方示例：句 1 结束、`stash.sentence_id = 2`），
    /// 也可能是**本句**的未确认部分；两种读法都要能工作，所以草稿单独记，
    /// 不与已确认文本揉在一起。
    draft: Option<(Option<i64>, String)>,
    /// 已定稿的句：id → 定稿文本。
    ///
    /// 存文本而不只是 id：重复到达的同一句**文本没变**时不该刷 WARN
    /// （服务端重发同一条 `sentence_end` 是正常的）。
    ///
    /// **只有带 id 的句子才进来** —— 缺 id 的句子无法可靠去重（见 `map_sentence`）。
    finalized: HashMap<i64, String>,
    /// 本次会话是否已经为「缺 `sentence_id`」记过 WARN（避免刷屏）。
    warned_missing_id: bool,
}

impl AliyunMapper {
    pub fn new() -> Self {
        Self::default()
    }

    /// 把一条 `sentence` 归一成 `CloudEvent`。
    ///
    /// ## 结果结构：已确认句子 + 草稿
    ///
    /// 服务端返回 `sentence`（已确认的句子，`sentence_end` 表示它结束了）与
    /// `stash`（草稿）。所以：
    ///
    /// - 实时行文本 = 已确认文本 + 草稿；
    /// - `sentence_end == true` → 该句定稿，用**本句**的已确认文本（没有就用
    ///   本句的快照）；随后到达的草稿属于**下一句**，转成新的实时行。
    ///
    /// ## 缺 `sentence_id` 怎么办
    ///
    /// 官方口径里 `sentence_id` 必有（正常结果从 1 递增，心跳固定 0）。若服务端
    /// 缺这个字段，两条**不同**的句子会共用同一个 id —— 用它做去重键会把第二句
    /// 当成重复句**静默丢掉**。所以：
    ///
    /// - **只有带 id 的句子才参与 `finalized` 去重**；
    /// - 缺 id 时不靠 id 去重，靠「句结束」与草稿切句；
    /// - `id == 0` 但没带 `heartbeat` 标记 → 当成缺 id 处理（0 是心跳的 id）；
    /// - 每次会话首次发现缺 id 时记一条 WARN —— 兼容路径能继续工作，但服务端
    ///   违约这件事要留在日志里，不能用一个静默兜底换掉另一个静默吞句。
    pub fn map_sentence(&mut self, sentence: Sentence) -> Vec<CloudEvent> {
        let mut events = Vec::new();
        // ① 心跳先判：它的 `sentence_id` 固定为 0，若先判「新 id」会误把
        //    上一句提前定稿。
        if sentence.heartbeat {
            return events;
        }
        // ② 句标识：缺字段、或「id 是 0 但没标心跳」都按**没有 id**处理。
        let raw_id = sentence.id.filter(|id| *id != 0);
        if raw_id.is_none() && !self.warned_missing_id {
            self.warned_missing_id = true;
            super::logging::warn(
                "阿里云端：服务端的句子没有 sentence_id（0 视为心跳），改用句结束标志切句",
                None,
            );
        }
        // ③ 已定稿的句：后续事件一律忽略。文本没变不刷 WARN（服务端重发同一条
        //    end 是正常的）。
        if let Some(id) = raw_id {
            if let Some(finalized) = self.finalized.get(&id) {
                if !sentence.text.trim().is_empty() && finalized != sentence.text.trim() {
                    super::logging::warn("阿里云端：已定稿的句子又收到了新的文本，已忽略", None);
                }
                return events;
            }
        }
        // 用原文（不 trim）拼接 —— trim 只用来判空：英文的词间空格不能被吃掉。
        let confirmed = sentence.text.as_str();
        let draft = sentence.draft.as_str();
        let confirmed_empty = confirmed.trim().is_empty();
        let draft_empty = draft.trim().is_empty();

        // ④ 句 id 变了 → 上一句没结束就开了新句：先按快照定稿它
        //    （否则会话层的 Partial 替换语义会让上一句的文字永久消失）。
        //
        //    **判定只看 id，与本句有没有文本无关** —— 服务端可能先发一个
        //    「新句 + 空文本 + sentence_end」的收尾事件（2026-11-17 外部复审
        //    发现的真实路径）。那时本句没文本，但**上一句的快照必须照样定稿** ——
        //    否则上一句只剩一条 Partial，下一个 Partial 一覆盖就彻底消失。
        let differs = match (&self.open, raw_id) {
            (None, _) => false,
            // 两边都有 id：id 不同就是新句。
            (Some((open_id, _)), Some(id)) => *open_id != Some(id),
            // 一边有 id 一边没有：无法核对是不是同一句 → 当新句
            //（宁可多切一句，也不要把上一句的文本丢掉）。
            // **但只在事件带文本时**：空事件不能定稿打开的句子 ——
            // 「id=0 但没标 heartbeat」正是这种形状（心跳）。
            (Some((open_id, _)), None) => open_id.is_some() && !confirmed_empty,
        };
        if differs {
            if let Some((open_id, snapshot)) = self.open.take() {
                if !snapshot.trim().is_empty() {
                    if let Some(id) = open_id {
                        self.finalized.insert(id, snapshot.clone());
                    }
                    events.push(final_event(snapshot));
                }
            }
        }
        if !confirmed_empty {
            // 本句的旧草稿作废（它已被这句的已确认文本取代）。
            //
            // **草稿没有 id 时按「属于本句」处理** —— 缺 id 就没法判断它属于
            // 哪一句；留着它，下一次更新会把过期草稿再拼一次（文本重复）。
            let draft_is_this_sentence = match self.draft.as_ref().and_then(|(id, _)| *id) {
                None => true,
                Some(id) => Some(id) == raw_id && raw_id.is_some(),
            };
            if draft_is_this_sentence {
                self.draft = None;
            }
            self.open = Some((raw_id, confirmed.to_string()));
        }

        // ⑤ 草稿：可能是下一句的，也可能是本句未确认的部分 —— 两种读法都要能
        //    工作，所以单独记。
        if !draft_empty {
            self.draft = Some((sentence.draft_id, draft.to_string()));
        }

        if sentence.end {
            // ⑥ 句结束 → 定稿。服务端没给文本时用**本句**的快照（别的句子的
            //    草稿不能拿来当本句的定稿）。
            let mut final_text = if !confirmed_empty {
                confirmed.to_string()
            } else {
                String::new()
            };
            // 快照归谁：
            // - **是本句的** → 用它当本句的定稿（服务端没给文本时的快照）；
            // - **是别的句子的** → 上一句还没定稿 → **先把上一句按快照定稿**。
            //   服务端可能发「缺 sentence_id + 空文本 + sentence_end」的收尾
            //   事件（违约路径）：丢掉它的话，上一句只剩一条 Partial，
            //   下一个 Partial 一覆盖就彻底消失。
            if let Some((open_id, snapshot)) = self.open.take() {
                if same_sentence(open_id, raw_id) {
                    if final_text.is_empty() {
                        final_text = snapshot.clone();
                    }
                } else if !snapshot.trim().is_empty() {
                    if let Some(id) = open_id {
                        self.finalized.insert(id, snapshot.clone());
                    }
                    events.push(final_event(snapshot));
                }
            }
            // **空文本不进去重表**：那张表是「这句已定稿**且有内容**」的凭据 ——
            // 空文本进去就把去重键占住了，同 id 的**后续真实文本**会被 ③ 的早退
            // 整段丢弃（只留一条 WARN）。2026-11-17 外部复审发现的真实路径。
            if !final_text.trim().is_empty() {
                if let Some(id) = raw_id {
                    self.finalized.insert(id, final_text.clone());
                }
            }
            if !final_text.trim().is_empty() {
                events.push(final_event(final_text));
            }
            // 本句的草稿作废；**下一句的草稿要留下**（官方示例：句 1 结束时
            // `stash.sentence_id = 2`，那是句 2 的草稿）。
            if self.draft.as_ref().and_then(|(id, _)| *id) == raw_id && raw_id.is_some() {
                self.draft = None;
            }
        }

        // ⑦ 实时行 = 已确认文本 + 草稿。
        let live = format!(
            "{}{}",
            self.open
                .as_ref()
                .map(|(_, text)| text.as_str())
                .unwrap_or(""),
            self.draft
                .as_ref()
                .map(|(_, text)| text.as_str())
                .unwrap_or("")
        );
        if !live.trim().is_empty() {
            events.push(CloudEvent::Partial(live));
        }
        events
    }
}

/// 两个句标识是不是同一句：都缺 id 时算同一句（无从核对）；一边缺 id 时
/// **不算**同一句（不能拿别的句子的快照当本句的定稿）。
fn same_sentence(open_id: Option<i64>, id: Option<i64>) -> bool {
    match (open_id, id) {
        (Some(a), Some(b)) => a == b,
        (None, None) => true,
        _ => false,
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
        let error = super::logging::code_detail(
            ErrorCode::CloudSendFailed,
            error.to_string(),
            Some(&config.api_key),
        );
        let _ = ready.send(Err(error.clone()));
        let _ = errors.send(error);
        return;
    }

    let mut mapper = AliyunMapper::new();
    let mut finishing: Option<Sender<()>> = None;
    let mut started = false;
    // 就绪之前收到的错误（例如鉴权失败被报成 `task-failed`）：`connect()`
    // 只能通过 ready 通道知道失败原因，别让它只拿到笼统的 connectFailed。
    let mut startup_error: Option<String> = None;
    loop {
        tokio::select! {
            command = commands.recv() => {
                let Some(command) = command else { break };
                match command {
                    Command::Audio(packet) => {
                        // 协议要求先 `task-started` 才能发音频。正常路径上
                        // `connect()` 成功才可能走到这里；这条门是纵深防御
                        // （调用方忽略 connect 的 Err 时不要发出违约帧）。
                        if !started {
                            super::logging::warn(
                                "阿里云端：task-started 之前收到音频，已丢弃",
                                Some(&config.api_key),
                            );
                            continue;
                        }
                        if let Err(error) = socket.send(Message::Binary(packet.into())).await {
                            let _ = errors.send(super::logging::code_detail(
                                ErrorCode::CloudSendFailed,
                                error.to_string(),
                                Some(&config.api_key),
                            ));
                            break;
                        }
                    }
                    Command::Finish(done) => {
                        let payload = encode_finish_task(&task_id);
                        if let Err(error) = socket.send(Message::Text(payload.into())).await {
                            let _ = errors.send(super::logging::code_detail(
                                ErrorCode::CloudSendFailed,
                                error.to_string(),
                                Some(&config.api_key),
                            ));
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
                    let _ = errors.send(crate::code(ErrorCode::CloudDisconnected));
                    if let Some(done) = finishing.take() { let _ = done.send(()); }
                    break;
                };
                let raw = match message {
                    Ok(Message::Text(raw)) => raw,
                    Ok(Message::Close(_)) => {
                        // 没收到 task-finished 就关了 → 异常结束。
                        if finishing.is_some() {
                            let _ = errors.send(crate::code(ErrorCode::CloudDisconnected));
                        }
                        if let Some(done) = finishing.take() { let _ = done.send(()); }
                        break;
                    }
                    // 服务端不该发二进制帧；忽略而不是报错。
                    Ok(_) => continue,
                    Err(error) => {
                        let _ = errors.send(super::logging::code_detail(
                            ErrorCode::CloudDisconnected,
                            error.to_string(),
                            Some(&config.api_key),
                        ));
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
                        // 3.1 按 token 计费：把这次任务的用量记下来（只读最终累计值，
                        // **不要**把各 result-generated 的 usage 相加）。
                        log_usage(&raw);
                        if let Some(done) = finishing.take() { let _ = done.send(()); }
                        break;
                    }
                    Ok(ServerEvent::TaskFailed { code, message }) => {
                        let detail = format!("{code}: {}", sanitize(&message, &config.api_key));
                        let error = super::logging::code_detail(ErrorCode::CloudProtocolError, detail, Some(&config.api_key));
                        if !started {
                            startup_error = Some(error.clone());
                        }
                        // 详情只进日志（已脱敏），不进界面。
                        super::logging::warn(&format!("阿里云端：{error}"), Some(&config.api_key));
                        let _ = errors.send(error);
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
    // 连上了但没等到 task-started（连接断了、或先收到了 task-failed）：
    // 别让 connect() 白等 15 秒，也要把真因带回去。
    if !started {
        let error = startup_error.unwrap_or_else(|| crate::code(ErrorCode::CloudConnectFailed));
        let _ = ready.send(Err(error));
    }
    if let Some(done) = finishing.take() {
        let _ = done.send(());
    }
}

/// 记一条任务用量的日志（只读 `task-finished` 的最终累计值）。
fn log_usage(raw: &str) {
    let Ok(value) = serde_json::from_str::<Value>(raw) else {
        return;
    };
    let usage = value
        .get("payload")
        .and_then(|payload| payload.get("usage"))
        .cloned()
        .unwrap_or(Value::Null);
    if usage.is_null() {
        return;
    }
    let duration = usage.get("duration").and_then(Value::as_i64).unwrap_or(0);
    let input = usage
        .get("input_tokens")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let output = usage
        .get("output_tokens")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    super::logging::info(
        &format!("阿里云端：任务完成 时长 {duration}s，输入 {input} token，输出 {output} token"),
        None,
    );
}

/// 连上阿里：`Authorization: Bearer <key>`（凭据只进请求头）。
async fn connect_socket(
    config: &AliyunConfig,
) -> Result<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    String,
> {
    let mut request = config.base_url().into_client_request().map_err(|error| {
        super::logging::code_detail(
            ErrorCode::CloudConnectFailed,
            error.to_string(),
            Some(&config.api_key),
        )
    })?;
    let header = format!("Bearer {}", config.api_key)
        .parse()
        .map_err(|_| crate::code(ErrorCode::CloudConnectFailed))?;
    request.headers_mut().insert("Authorization", header);
    let (socket, _) = tokio_tungstenite::connect_async(request)
        .await
        .map_err(|error| {
            // 握手阶段的 401 / 403 是**鉴权失败**（key 不对、业务空间没权限），
            // 与「网络连不上」是两回事 —— 前者的处置是改配置，后者才值得重试。
            if let tokio_tungstenite::tungstenite::Error::Http(response) = &error {
                let status = response.status().as_u16();
                if status == 401 || status == 403 {
                    return crate::code(ErrorCode::CloudAuthFailed);
                }
            }
            super::logging::code_detail(
                ErrorCode::CloudConnectFailed,
                sanitize(&error.to_string(), &config.api_key),
                Some(&config.api_key),
            )
        })?;
    Ok(socket)
}

#[cfg(test)]
mod tests;
