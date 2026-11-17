//! 云端识别：provider 抽象 + 云端流式会话。
//!
//! 两条边界：
//! - **provider 只管协议**：连上、发音频、提交、收尾，事件通过回调上抛。
//! - **会话只管时间轴**：句子时间、缺口换算一律按本地 16 kHz 采样数算，
//!   provider 给的时间戳只是可选增强，**不覆盖**本地轴（见 `Final` 的说明）。
//!
//! provider 的实现放在 `cloud/realtime.rs`（OpenAI Realtime JSON 族）。

use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::Instant;

use sherpa_onnx::LinearResampler;

use super::{AsrSession, Segment, StreamEvent, Timeline, Transcript, SAMPLE_RATE};

// provider 的实现：`realtime.rs`（OpenAI Realtime JSON 族）、`volcengine.rs`
// （火山 / 豆包，自研二进制帧协议）、`aliyun.rs`（阿里云百炼 Qwen-Audio，
// JSON 控制帧 + 裸二进制音频帧）。
pub mod aliyun;
pub mod realtime;
// 中间结果归一器：把各家的协议形态归一成「当前句的累计串」。
pub mod partial;
// 火山（豆包）双向流式 ASR provider：自研二进制帧协议。
pub mod volcengine;
// 云端日志的**唯一出口**：写入前脱敏（见 `logging.rs`）。
pub(crate) mod logging;


/// 句标识：协议给的句子 id。没有 id 的协议（服务端断句族）用 `None`。
pub type SentenceId = Option<String>;

/// 一次音频发送的结果。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SendOutcome {
    /// 没送出去的 16 kHz 采样数（正常是 0）。
    pub dropped: usize,
    /// 这次是否真的把音频交出去了（攒包不算）。
    pub delivered: bool,
}

impl SendOutcome {
    /// 全部送出去了。
    pub fn delivered() -> Self {
        Self { dropped: 0, delivered: true }
    }
}

/// provider 声明的中间结果语义形态。
///
/// 各家协议给中间结果的方式不同，这里显式列出来 —— 归一器按它分派，
/// 上层（会话层 / 前端）只认一种语义：当前未定稿句的累计文本。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PartialSemantics {
    /// 增量拼接：每次追加一段（OpenAI Realtime 的 `.delta`）。
    Delta,
    /// 确认前缀加草稿：整体替换成「确认前缀 + 草稿」。阿里的
    /// `sentence.text` + `stash` 是标准形态；火山的「确认前缀为空、整句快照」
    /// 是它的退化形态。
    Preview,
    /// 区间替换：按区间改写已识别片段（讯飞的 `pgs` + `rg`）。
    Range,
}

/// provider 的能力声明。
///
/// **必须无堆分配**（`Clone + Copy`）：它要能做编译期常量
/// （`impl XProvider { pub const CAPABILITIES: Capabilities = …; }`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capabilities {
    /// 句边界由谁定：客户端提交还是服务端断句。
    pub endpoint_kind: EndpointKind,
    /// 是否需要本地 VAD 做断句（客户端提交族为真）。
    pub needs_local_vad: bool,
    /// 中间结果的语义形态。
    pub partial_semantics: PartialSemantics,
    /// 是否支持请求级术语表。
    pub supports_keywords: bool,
    /// provider 要求的上传采样率（内部轴恒为 16 kHz）。
    pub target_sample_rate: u32,
}

/// 能力组合是否合法。
///
/// 只允许两种组合：客户端提交 + 需要本地 VAD；服务端断句 + 不需要本地 VAD。
/// 其余组合会让「缺 VAD」被误读成「服务端断句」→ 直接拒绝。
pub fn valid_capabilities(caps: &Capabilities) -> bool {
    match caps.endpoint_kind {
        EndpointKind::ClientCommit => caps.needs_local_vad,
        EndpointKind::ServerEndpoint => !caps.needs_local_vad,
    }
}

/// 按 provider id 静态查能力（`realtime` / `volcengine` / `aliyun`）。
///
/// 表项的唯一来源是各 provider 实现自报的常量（`XProvider::CAPABILITIES`），
/// 不手写第二份 —— 两处漂移会让「状态回传的能力」与「实际行为」分叉。
pub fn provider_capabilities(id: &str) -> Option<Capabilities> {
    match id {
        "realtime" => Some(realtime::RealtimeProvider::CAPABILITIES),
        "volcengine" => Some(volcengine::VolcengineProvider::CAPABILITIES),
        "aliyun" => Some(aliyun::AliyunProvider::CAPABILITIES),
        _ => None,
    }
}

/// 云端 provider 上抛的事件。
#[derive(Clone, Debug)]
pub enum CloudEvent {
    /// 中间结果：还在变，同一段文字可能后面被改写。
    Partial(String),
    /// 服务端确认了一次提交：会话层靠它把句子区间绑到 `item_id` 上。
    ///
    /// **必须按提交顺序到达** —— 会话层按顺序把区间绑到 id 上，
    /// 顺序乱了区间就会绑错。
    CommitAck {
        item_id: Option<String>,
    },
    /// 一句定稿。
    Final {
        /// 服务端给这一轮的 `item_id`：会话层靠它找到对应的句子区间。
        /// 没有 id 时退回按提交顺序配对。
        item_id: Option<String>,
        text: String,
        /// provider 给的起点（毫秒）。**可选增强**：只作参考，
        /// 不覆盖本地轴算出来的 `Segment.start` / `Segment.end`。
        start_ms: Option<u64>,
        /// provider 给的终点（毫秒）。同上。
        end_ms: Option<u64>,
    },
}

/// provider 怎么断句。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndpointKind {
    /// 客户端提交：静音时客户端发 commit，服务端回一条 final。
    ClientCommit,
    /// 服务端自己断句：服务端按自己的规则发 final。
    ServerEndpoint,
}

/// 云端识别服务。
///
/// 实现方在**自己的线程**上跑网络 IO：事件通过 `connect` 拿到的回调上抛。
/// 音频 / 提交 / 收尾是同步调用（内部走 channel）：音频帧从不阻塞；提交与
/// 收尾在队列背压时有界等待（最多 3 秒），等不到就报发送失败。
pub trait CloudProvider: Send {
    /// 能力声明（**唯一来源**）：其余能力方法都从它取。
    fn capabilities(&self) -> Capabilities;
    /// 这个 provider 怎么断句。
    fn endpoint_kind(&self) -> EndpointKind {
        self.capabilities().endpoint_kind
    }
    /// provider 要求的采样率（Hz）。
    ///
    /// 会话内部始终是 16 kHz：不一样时在上传前升采样（见 `CloudStreamingSession`）。
    fn target_sample_rate(&self) -> u32 {
        self.capabilities().target_sample_rate
    }
    /// 是否需要本地 VAD 做断句。
    fn needs_local_vad(&self) -> bool {
        self.capabilities().needs_local_vad
    }
    /// 是否支持请求级术语表。
    fn supports_keywords(&self) -> bool {
        self.capabilities().supports_keywords
    }
    /// 中间结果的语义形态。
    fn partial_semantics(&self) -> PartialSemantics {
        self.capabilities().partial_semantics
    }
    /// 建连。`on_event` 是事件回调：provider 在自己的线程上调用它。
    fn connect(&mut self, on_event: Box<dyn Fn(CloudEvent) + Send + Sync>) -> Result<(), String>;
    /// 发一段音频（provider 采样率、16bit、单声道）。
    ///
    /// 返回值要说清两件事：**没送出去多少**、**这次是否真的把音频交出去了**。
    /// 只回一个「丢了多少」不够 —— provider 可能只是把音频攒进自己的包
    /// 里（一个包都没发），会话层若把「没丢」当成「送出去了」，就会把正在
    /// 累积的丢包段提前闭合，连续丢包永远并不成一条。
    fn send_audio(&mut self, pcm: &[i16]) -> Result<SendOutcome, String>;
    /// 提交当前这一段（`client_commit` 家族用）。
    fn commit(&mut self) -> Result<(), String>;
    /// 收尾：等最后的结果回来。
    fn finish(&mut self) -> Result<(), String>;
    /// 后台异步发现的错误（默认只在下一次发送时暴露）。
    fn take_error(&mut self) -> Option<String> {
        None
    }
}

/// 按设置造一个 provider。
///
/// provider 名字来自 `settings.cloud_provider`；地址与模型留空时用该 provider
/// 的默认值。凭据直接进配置 —— 它不会进日志，也不会回传前端。
pub fn create_provider(
    settings: &crate::settings::Settings,
) -> Result<Box<dyn CloudProvider>, String> {
    let id = settings.cloud_provider.as_str();
    // 能力先查表：这是「要不要下 VAD」「状态回传什么能力」的同一个来源。
    let caps = provider_capabilities(id).ok_or_else(|| logging::code_detail(
        "cloud.unsupportedProvider",
        id,
        Some(&settings.cloud_api_key),
    ))?;
    if !valid_capabilities(&caps) {
        return Err(logging::code_detail("cloud.unsupportedProvider", id, Some(&settings.cloud_api_key)));
    }
    match id {
        "realtime" => {
            let base_url = if settings.cloud_base_url.trim().is_empty() {
                realtime::DEFAULT_BASE_URL.to_string()
            } else {
                settings.cloud_base_url.trim().to_string()
            };
            let model = if settings.cloud_model.trim().is_empty() {
                realtime::DEFAULT_MODEL.to_string()
            } else {
                settings.cloud_model.trim().to_string()
            };
            realtime::validate_realtime_model(&model)?;
            // 语言：auto = 不指定，让服务自己判断。
            let languages = match settings.language.as_str() {
                "auto" => Vec::new(),
                other => vec![other.to_string()],
            };
            // 术语表两层过滤：能力层决定「要不要带」，协议层决定「哪些字符能带」。
            let (words, cap_ignored) = cloud_keywords(settings, &caps);
            let (keywords, proto_ignored) = realtime::clean_cloud_keywords(&words);
            let ignored = cap_ignored + proto_ignored;
            if ignored > 0 {
                log_ignored_hotwords(ignored, &settings.cloud_api_key);
            }
            Ok(Box::new(realtime::RealtimeProvider::new(
                realtime::RealtimeConfig {
                    base_url,
                    api_key: settings.cloud_api_key.clone(),
                    model,
                    languages,
                    keywords,
                },
            )))
        }
        "volcengine" => {
            let base_url = if settings.cloud_base_url.trim().is_empty() {
                volcengine::DEFAULT_BASE_URL.to_string()
            } else {
                settings.cloud_base_url.trim().to_string()
            };
            let resource_id = if settings.cloud_resource_id.trim().is_empty() {
                volcengine::DEFAULT_RESOURCE_ID.to_string()
            } else {
                settings.cloud_resource_id.trim().to_string()
            };
            // 术语表：能力层决定要不要带，火山协议层决定哪些能带（上限 100 条（MAX_KEYWORDS））。
            let (words, cap_ignored) = cloud_keywords(settings, &caps);
            let (keywords, proto_ignored) = volcengine::clean_volcengine_keywords(&words);
            let ignored = cap_ignored + proto_ignored;
            if ignored > 0 {
                log_ignored_hotwords(ignored, &settings.cloud_api_key);
            }
            Ok(Box::new(volcengine::VolcengineProvider::new(
                volcengine::VolcengineConfig {
                    base_url,
                    api_key: settings.cloud_api_key.clone(),
                    resource_id,
                    keywords,
                    itn: settings.itn,
                    punct: settings.punct,
                },
            )))
        }
        "aliyun" => {
            let model = if settings.cloud_model.trim().is_empty() {
                aliyun::DEFAULT_MODEL.to_string()
            } else {
                settings.cloud_model.trim().to_string()
            };
            // 模型白名单：协议骨架相同，但参数与结果结构不同（本模型有 stash
            // 草稿、即时热词、方言开关），不按「名字随便填」处理。
            if !aliyun::SUPPORTED_MODELS.contains(&model.as_str()) {
                return Err(logging::code_detail("cloud.unsupportedModel", &model, Some(&settings.cloud_api_key)));
            }
            // 术语表：能力层决定要不要带，协议层决定能带多少（2000 条，带权重）。
            let (words, cap_ignored) = cloud_keywords(settings, &caps);
            let (vocabulary, proto_ignored) = aliyun::build_vocabulary(&words);
            let ignored = cap_ignored + proto_ignored;
            if ignored > 0 {
                log_ignored_hotwords(ignored, &settings.cloud_api_key);
            }
            Ok(Box::new(aliyun::AliyunProvider::new(aliyun::AliyunConfig {
                base_url: settings.cloud_base_url.clone(),
                api_key: settings.cloud_api_key.clone(),
                model,
                language_hint: aliyun::language_hint(&settings.language),
                vocabulary,
            })))
        }
        other => Err(logging::code_detail("cloud.unsupportedProvider", other, Some(&settings.cloud_api_key))),
    }
}

/// 热词被协议层丢掉时记一条。
///
/// 凭据在设置里 —— 顺手交给关口脱敏（这行文案本身没有 provider 文本，
/// 但关口是唯一出口，别在这里另起一条）。
fn log_ignored_hotwords(ignored: usize, api_key: &str) {
    logging::warn(
        &format!("云端热词有 {ignored} 条未通过协议校验，已忽略"),
        Some(api_key),
    );
}

/// 云端 provider 在**协议层**丢掉多少条术语（按当前选中的 provider 算）。
///
/// 术语表是本地流式与云端共用的设置，但**各家的协议限制不同**（长度、条数、
/// 字符）—— 被丢掉的用户得知道，不能只记一条日志（`cloudKeywords` 那些）。
///
/// 用的是 `create_provider` 里**同一批过滤函数**（不写第二份规则）。漂移风险
/// 只有「加了 provider 忘了加分支」→ 计数是 0（**不出声**，方向安全）。
pub fn cloud_hotwords_ignored(settings: &crate::settings::Settings) -> usize {
    let words = &settings.hotwords;
    match settings.cloud_provider.as_str() {
        "realtime" => realtime::clean_cloud_keywords(words).1,
        "volcengine" => volcengine::clean_volcengine_keywords(words).1,
        "aliyun" => aliyun::build_vocabulary(words).1,
        _ => 0,
    }
}

/// 生成一个 UUID v4 形状的唯一串。
///
/// 火山用它做请求 ID / 连接 ID（官方建议用 UUID 追踪调用），阿里用它做
/// `task_id`。**不是密码学随机**，只用于标识与关联。
pub(crate) fn new_uuid() -> String {
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

/// 把凭据从文本里抹掉：日志、错误详情、错误帧里都不该出现它。
///
/// 三家 provider 共用（火山与 Realtime 原来各有一份逐字相同的实现）。
pub(crate) fn sanitize(text: &str, secret: &str) -> String {
    if secret.is_empty() {
        return text.to_string();
    }
    text.replace(secret, "***")
}

/// 术语表：按 provider 能力决定要不要带。
///
/// **纯函数**（可单测）：能力声明不支持请求级术语表时返回空词表 ——
/// 系统不得向不支持它的服务发送术语表。
pub fn cloud_keywords(
    settings: &crate::settings::Settings,
    caps: &Capabilities,
) -> (Vec<String>, usize) {
    if !caps.supports_keywords {
        return (Vec::new(), settings.hotwords.len());
    }
    // 协议专属的字符过滤在 provider 内部做（各家的规则不同）。
    (settings.hotwords.clone(), 0)
}

/// 两个「没能单独提交」的句子区间合成一个：起点取早的，终点取晚的。
///
/// 提交失败时服务端把两段音频并成一条结果，句子时间也只能合成一段。
fn merge_span(earlier: Option<(f32, f32)>, later: (f32, f32)) -> (f32, f32) {
    match earlier {
        Some(span) => (span.0.min(later.0), span.1.max(later.1)),
        None => later,
    }
}

/// f32 → i16：clamp 后取整。
///
/// 幅度超过 ±1 的样本（AGC 之后仍可能越界）要夹住 —— 直接 `as i16`
/// 会绕回（Rust 的浮点转整数是饱和的，但自己夹一遍更明确）。
fn to_i16(samples: &[f32]) -> Vec<i16> {
    samples
        .iter()
        .map(|sample| (sample * 32767.0).round().clamp(-32768.0, 32767.0) as i16)
        .collect()
}

/// 云端流式会话：把本地 16 kHz 音频送出去，收回中间结果和定稿句子。
///
/// 句子时间**只**由本地时钟决定：provider 的时间戳不参与（`Final` 里的
/// `start_ms` / `end_ms` 只是留着给以后做对齐用的）。
pub struct CloudStreamingSession {
    provider: Box<dyn CloudProvider>,
    events: Receiver<CloudEvent>,
    /// 事件的出口：provider 的回调往这里塞。
    events_tx: Sender<CloudEvent>,
    /// 16 kHz → provider 采样率的重采样器（provider 收 16k 时是 None）。
    resampler: Option<LinearResampler>,
    timeline: Timeline,
    /// 喂给 provider 的最后一段音频的终点（16 kHz 采样数）。
    fed_end: usize,
    /// 上一次提交之后有没有喂过音频（没有就别提交空段）。
    fed_since_commit: bool,
    /// 已提交、还没回来的 final 的时间区间。
    ///
    /// **按 item_id 绑定，不按到达顺序**：服务端的 final 可能乱序到达，
    /// 也可能缺一条（识别失败 / 回执没到）。按到达顺序配对会让后面每一句
    /// 都抢到前一句的区间 —— 时间戳整段错位。
    ///
    /// - `pending`：还没绑到 id 的区间，按提交顺序。
    /// - `by_id`：已绑定的区间，按 id 查。
    pending: VecDeque<(f32, f32)>,
    by_id: HashMap<String, (f32, f32)>,
    /// 提交失败的区间：服务端没收到这次 commit，那一段音频会并进下一次
    /// 提交、只回一条 final —— 区间先攒着，等下一次提交成功时一起登记。
    uncommitted_span: Option<(f32, f32)>,
    /// 最近一次收到的中间结果。
    partial: String,
    segments: Vec<Segment>,
    /// 出错时的字典错误码：已定稿的句子留着，界面上另外提示。
    error: Option<String>,
    /// 上传队列满载时丢掉的音频：(起点 16 kHz 采样数, 丢掉的采样数)。
    ///
    /// 起点用采样数（不是秒）：连续丢包要判断「接得上」，只有同单位才能比。
    ///
    /// 这些采样**已经喂进会话**（时钟里算过了），所以不能再推进时钟 ——
    /// 但它们没送到服务端，识别结果里要有可见缺口。
    upload_gaps: Vec<(usize, usize)>,
    /// 正在累积的丢包段（还没闭合）。
    ///
    /// **必须跨 `take_upload_gaps()` 存活**：编排每轮循环都会取一次缺口，
    /// 累积状态若跟着被取走，连续丢包就永远并不成一条（每轮一条
    /// `[[gap:1]]`，刷屏且累计时长自相矛盾）。闭合时机 = 下一次成功发送。
    open_upload_gap: Option<(usize, usize)>,
    started: Instant,
}

impl CloudStreamingSession {
    pub fn new(provider: Box<dyn CloudProvider>) -> Self {
        let (events_tx, events) = channel();
        let resampler = if provider.target_sample_rate() as i32 == SAMPLE_RATE {
            None
        } else {
            LinearResampler::create(SAMPLE_RATE, provider.target_sample_rate() as i32)
        };
        Self {
            provider,
            events,
            events_tx,
            resampler,
            timeline: Timeline::default(),
            fed_end: 0,
            fed_since_commit: false,
            pending: VecDeque::new(),
            by_id: HashMap::new(),
            uncommitted_span: None,
            partial: String::new(),
            segments: Vec::new(),
            error: None,
            upload_gaps: Vec::new(),
            open_upload_gap: None,
            started: Instant::now(),
        }
    }

    /// 建连。失败时返回字典错误码（`cloud.connectFailed` 一类）。
    pub fn connect(&mut self) -> Result<(), String> {
        let tx = self.events_tx.clone();
        self.provider.connect(Box::new(move |event| {
            // 通道是无界的：定稿结果一条都不能丢，所以不能改成「满了就丢」。
            // 中间结果只是「最新的状态」，但它们和定稿走同一条道。
            let _ = tx.send(event);
        }))
    }

    /// 把 provider 递过来的事件变成界面事件。
    fn drain(&mut self, events: &mut Vec<StreamEvent>) {
        while let Ok(event) = self.events.try_recv() {
            match event {
                CloudEvent::Partial(text) => {
                    if text != self.partial {
                        self.partial = text.clone();
                        if !text.is_empty() { events.push(StreamEvent::Partial(text)); }
                    }
                }
                CloudEvent::CommitAck { item_id } => {
                    // 回执按提交顺序到达：把最靠前的未绑定区间绑到这个 id。
                    if let Some(item_id) = item_id {
                        if let Some(span) = self.pending.pop_front() {
                            self.by_id.insert(item_id, span);
                        }
                    }
                }
                CloudEvent::Final { item_id, text, .. } => {
                    // 提交时就把区间定下来（那时本地时钟还没被静音段推走）；
                    // final 回来得晚也不影响：按提交顺序配对。
                    // 先按 id 找区间；找不到（没有 id、或回执还没到）才退回
                    // 按提交顺序取最靠前的那个。
                    let span = match &item_id {
                        Some(item_id) => self.by_id.remove(item_id),
                        None => None,
                    }
                    .or_else(|| self.pending.pop_front());
                    let (start, end) = match span {
                        Some(span) => span,
                        None => {
                            let (start, end) = self.timeline.commit_at(self.fed_end, true);
                            (start as f32 / SAMPLE_RATE as f32, end as f32 / SAMPLE_RATE as f32)
                        }
                    };
                    // 空句不产句子，但区间已配掉；失败的 item 也走这里，防止后续句子全被卡住。
                    self.partial.clear();
                    if text.trim().is_empty() {
                        continue;
                    }
                    let segment = Segment { text, start, end };
                    events.push(StreamEvent::Segment(segment.clone()));
                    self.segments.push(segment);
                }
            }
        }
        // 后台断连 / 协议错误要立刻进 warning，不等下一次发送。
        if let Some(error) = self.provider.take_error() {
            self.error = Some(error);
        }
    }

    /// 能力声明（编排层据此决定门控与提交）。
    pub fn capabilities(&self) -> Capabilities {
        self.provider.capabilities()
    }

    /// 取回已闭合的上传丢包缺口（起点 16 kHz 采样数, 丢掉的采样数）。
    pub fn take_upload_gaps(&mut self) -> Vec<(usize, usize)> {
        std::mem::take(&mut self.upload_gaps)
    }

    /// 闭合当前累积的丢包段（下次成功发送、或收尾时调用）。
    fn close_upload_gap(&mut self) {
        if let Some((start, count)) = self.open_upload_gap.take() {
            logging::warn(
                &format!("云端上传队列满，丢掉 {count} 个 16kHz 采样（起点 {start}）"),
                None,
            );
            self.upload_gaps.push((start, count));
        }
    }

    /// 只收取后台事件，不喂音频：静音期也要让已返回的 final 出字。
    pub fn poll(&mut self) -> Vec<StreamEvent> {
        let mut events = Vec::new();
        self.drain(&mut events);
        events
    }

    /// 喂入 16 kHz 样本：升采样 → 转 i16 → 发给 provider。
    fn push_at(&mut self, samples: &[f32], end: usize) -> Vec<StreamEvent> {
        let mut events = Vec::new();
        if samples.is_empty() {
            return events;
        }
        // 这段音频在真实录音时间轴上的起点。
        let audio_start = end.saturating_sub(samples.len());
        self.timeline.note_audio(audio_start);
        // 升采样在 f32 域做，i16 转换在其后（provider 收 16k 时不升采样）。
        let resampled = match &self.resampler {
            Some(resampler) => resampler.resample(samples, false),
            None => samples.to_vec(),
        };
        let pcm = to_i16(&resampled);
        match self.provider.send_audio(&pcm) {
            Ok(outcome) => {
                let dropped = outcome.dropped;
                self.fed_end = end;
                self.fed_since_commit = true;
                if dropped > 0 {
                    // 没送出去：缺口起点就是这段音频的起点（整段都没发出去）。
                    let start = audio_start;
                    // **连续丢包并成一条**：网络持续背压时每一批都会丢一点，
                    // 逐条上报会刷出几百个 [[gap:1]]，而且每条都声称丢了
                    // 1 秒（累计时长自相矛盾）。相邻或重叠的丢包并进去。
                    let merged = match self.open_upload_gap {
                        Some((gap_start, gap_count)) if start <= gap_start + gap_count => {
                            self.open_upload_gap = Some((gap_start, gap_count + dropped));
                            true
                        }
                        _ => false,
                    };
                    if !merged {
                        // 上一段丢包到此为止 → 闭合它，再开新的一段。
                        self.close_upload_gap();
                        self.open_upload_gap = Some((start, dropped));
                    }
                } else if outcome.delivered {
                    // 这次真的送出去了 → 丢包段结束。
                    // **「只是攒进 provider 的包里」不算投递** —— 否则正在累积
                    // 的丢包段会被提前闭合，连续丢包永远并不成一条。
                    self.close_upload_gap();
                }
            }
            Err(error) => self.error = Some(error),
        }
        self.drain(&mut events);
        events
    }

    /// 静音时收尾：定下当前句子的区间，提交给 provider。
    fn force_end(&mut self) -> Vec<StreamEvent> {
        let mut events = Vec::new();
        if self.fed_since_commit {
            // 句子的终点是「最后喂进去的音频」的末尾，不是当前时钟
            // （时钟还在走静音段，算进去句子会拖长）。
            let (start, end) = self.timeline.commit_at(self.fed_end, true);
            let span = (
                start as f32 / SAMPLE_RATE as f32,
                end as f32 / SAMPLE_RATE as f32,
            );
            self.fed_since_commit = false;
            match self.provider.endpoint_kind() {
                // 客户端断句：**提交成功之后**才登记区间。提交失败时服务端不会
                // 为这一句回 final，先登记的话之后每条 final 都会配到上一句的
                // 区间上 —— 失败点以后整段时间戳错位一格。
                EndpointKind::ClientCommit => match self.provider.commit() {
                    Ok(()) => {
                        let span = merge_span(self.uncommitted_span.take(), span);
                        self.pending.push_back(span);
                    }
                    Err(error) => {
                        self.error = Some(error);
                        // 服务端没收到这次 commit，这段音频会并进下一次提交、
                        // 只回一条 final：区间攒着，等下次提交成功一起登记。
                        self.uncommitted_span =
                            Some(merge_span(self.uncommitted_span.take(), span));
                    }
                },
                // 服务端断句没有 commit：区间先登记，等 final 回来配。
                EndpointKind::ServerEndpoint => self.pending.push_back(span),
            }
        }
        self.drain(&mut events);
        events
    }

    /// 收尾：等 provider 把最后的结果发回来。
    fn finish(&mut self) -> Vec<StreamEvent> {
        let mut events = Vec::new();
        // 16k→24k 重采样器还有尾部状态：flush 后才算音频真正送完。
        if let Some(resampler) = &self.resampler {
            let tail = resampler.resample(&[], true);
            if !tail.is_empty() {
                match self.provider.send_audio(&to_i16(&tail)) {
                    Ok(outcome) => {
                        let dropped = outcome.dropped;
                        if dropped > 0 {
                            // 尾部重采样出来的一小段没送出去 → 并进当前丢包段。
                            let start = self.fed_end;
                            let merged = match self.open_upload_gap {
                                Some((gap_start, gap_count)) if start <= gap_start + gap_count => {
                                    self.open_upload_gap = Some((gap_start, gap_count + dropped));
                                    true
                                }
                                _ => false,
                            };
                            if !merged {
                                self.close_upload_gap();
                                self.open_upload_gap = Some((start, dropped));
                            }
                        } else if outcome.delivered {
                            self.close_upload_gap();
                        }
                    }
                    Err(error) => self.error = Some(error),
                }
            }
        }
        // 收尾这次提交会把**没提交出去的那段音频**一起带上，服务端为它回
        // 的 final 要有对应的区间。之前提交失败攒下的区间也在这一段里 ——
        // 服务端会把它们并成一条 final，区间也要并成一段。
        if self.fed_since_commit {
            let (start, end) = self.timeline.commit_at(self.fed_end, true);
            let span = (
                start as f32 / SAMPLE_RATE as f32,
                end as f32 / SAMPLE_RATE as f32,
            );
            self.uncommitted_span = Some(merge_span(self.uncommitted_span.take(), span));
        }
        if let Some(span) = self.uncommitted_span.take() {
            self.pending.push_back(span);
        }
        if let Err(error) = self.provider.finish() {
            self.error = Some(error);
        }
        // 收尾：把还没闭合的丢包段落账（编排在 finish 之后才取缺口）。
        self.close_upload_gap();
        self.drain(&mut events);
        // **最后一句还没定稿时的兜底**：实时行上已经显示了这段文字，
        // 停止后却不在最终转写里（用户会看到自己刚看到的字消失）。
        // 本地流式在 `finish()` 里会把 partial 落地，云端要有对等物。
        if !self.partial.trim().is_empty() {
            let text = self.partial.clone();
            let span = self.pending.pop_front();
            let (start, end) = match span {
                Some(span) => span,
                None => {
                    let (start, end) = self.timeline.commit_at(self.fed_end, true);
                    (
                        start as f32 / SAMPLE_RATE as f32,
                        end as f32 / SAMPLE_RATE as f32,
                    )
                }
            };
            self.partial.clear();
            let segment = Segment { text, start, end };
            events.push(StreamEvent::Segment(segment.clone()));
            self.segments.push(segment);
            logging::warn("云端最后一句没有定稿，按实时文本兜底落成句子", None);
        }
        events
    }
}

impl AsrSession for CloudStreamingSession {
    fn advance_samples(&mut self, samples: usize) -> usize {
        self.timeline.advance(samples);
        self.timeline.recorded
    }

    fn advance_dropped(&mut self, samples: usize) {
        self.timeline.advance_dropped(samples);
    }

    fn elapsed_seconds(&self) -> f32 {
        self.timeline.recorded as f32 / SAMPLE_RATE as f32
    }

    fn push(&mut self, samples: &[f32]) -> Vec<StreamEvent> {
        let end = self.timeline.recorded;
        self.push_at(samples, end)
    }

    fn push_at(&mut self, samples: &[f32], end: usize) -> Vec<StreamEvent> {
        CloudStreamingSession::push_at(self, samples, end)
    }

    fn force_end(&mut self) -> Vec<StreamEvent> {
        CloudStreamingSession::force_end(self)
    }

    fn poll(&mut self) -> Vec<StreamEvent> {
        CloudStreamingSession::poll(self)
    }

    fn finish(&mut self) -> Vec<StreamEvent> {
        CloudStreamingSession::finish(self)
    }

    fn transcript(self) -> Transcript {
        // 句子按**本地时间轴**排序：服务端的 final 可能乱序到达，
        // 不排序的话文本顺序会跟着乱（区间已经按 id 配对了，顺序没跟上）。
        let mut segments = self.segments;
        segments.sort_by(|a, b| {
            a.start
                .partial_cmp(&b.start)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Transcript {
            text: super::join_segments(segments.iter().map(|s| s.text.as_str())),
            segments,
            duration_seconds: self.timeline.recorded as f32 / SAMPLE_RATE as f32,
            elapsed_ms: self.started.elapsed().as_millis() as u64,
            language: "zh-en".into(),
            warning: self.error,
        }
    }
}

#[cfg(test)]
pub(crate) mod mock;

#[cfg(test)]
mod tests;
