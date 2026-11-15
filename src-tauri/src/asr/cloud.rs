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

// provider 的实现放在 `cloud/realtime.rs`（OpenAI Realtime JSON 族）。
pub mod realtime;
// 中间结果归一器：把各家的协议形态归一成「当前句的累计串」。
pub mod partial;


/// 句标识：协议给的句子 id。没有 id 的协议（服务端断句族）用 `None`。
pub type SentenceId = Option<String>;

/// provider 声明的中间结果语义形态。
///
/// 各家协议给中间结果的方式不同，这里显式列出来 —— 归一器按它分派，
/// 上层（会话层 / 前端）只认一种语义：当前未定稿句的累计文本。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PartialSemantics {
    /// 增量拼接：每次追加一段（OpenAI Realtime 的 `.delta`）。
    Delta,
    /// 确认前缀加草稿：整体替换成「确认前缀 + 草稿」（百炼的 `.text` + `stash`）。
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

/// 按 provider id 静态查能力。
///
/// 表项的唯一来源是各 provider 实现自报的常量（`XProvider::CAPABILITIES`），
/// 不手写第二份 —— 两处漂移会让「状态回传的能力」与「实际行为」分叉。
pub fn provider_capabilities(id: &str) -> Option<Capabilities> {
    match id {
        "realtime" => Some(realtime::RealtimeProvider::CAPABILITIES),
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
    /// 返回值是**没送出去**的 16 kHz 采样数（正常是 0）：上传队列满时
    /// 这段音频会被丢掉，调用方据此在结果里留可见缺口。
    fn send_audio(&mut self, pcm: &[i16]) -> Result<usize, String>;
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
    let caps = provider_capabilities(id).ok_or_else(|| crate::code_detail("cloud.unsupportedProvider", id))?;
    if !valid_capabilities(&caps) {
        return Err(crate::code_detail("cloud.unsupportedProvider", id));
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
                crate::log::warn(format!("云端热词有 {ignored} 条未通过协议校验，已忽略"));
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
        other => Err(crate::code_detail("cloud.unsupportedProvider", other)),
    }
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
    /// 上传队列满载时丢掉的音频：(起点秒, 丢掉的 16 kHz 采样数)。
    ///
    /// 这些采样**已经喂进会话**（时钟里算过了），所以不能再推进时钟 ——
    /// 但它们没送到服务端，识别结果里要有可见缺口。
    upload_gaps: Vec<(f32, usize)>,
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

    /// 取回上传丢包的缺口（起点秒, 丢掉的 16 kHz 采样数）。
    pub fn take_upload_gaps(&mut self) -> Vec<(f32, usize)> {
        std::mem::take(&mut self.upload_gaps)
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
            Ok(dropped) => {
                self.fed_end = end;
                self.fed_since_commit = true;
                if dropped > 0 {
                    // 没送出去：缺口起点就是这段音频的起点（整段都没发出去）。
                    let start = audio_start as f32 / SAMPLE_RATE as f32;
                    self.upload_gaps.push((start, dropped));
                    crate::log::warn(format!(
                        "云端上传队列满，丢掉 {dropped} 个 16kHz 采样"
                    ));
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
                if let Err(error) = self.provider.send_audio(&to_i16(&tail)) {
                    self.error = Some(error);
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
        self.drain(&mut events);
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
