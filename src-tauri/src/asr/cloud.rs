//! 云端识别：provider 抽象 + 云端流式会话。
//!
//! 两条边界：
//! - **provider 只管协议**：连上、发音频、提交、收尾，事件通过回调上抛。
//! - **会话只管时间轴**：句子时间、缺口换算一律按本地 16 kHz 采样数算，
//!   provider 给的时间戳只是可选增强，**不覆盖**本地轴（见 `Final` 的说明）。
//!
//! provider 的实现放在 `cloud/realtime.rs`（OpenAI Realtime JSON 族）。

use std::collections::VecDeque;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::Instant;

use sherpa_onnx::LinearResampler;

use super::{AsrSession, Segment, StreamEvent, Timeline, Transcript, SAMPLE_RATE};

// provider 的实现放在 `cloud/realtime.rs`（OpenAI Realtime JSON 族）。
pub mod realtime;

/// 云端 provider 上抛的事件。
#[derive(Clone, Debug)]
pub enum CloudEvent {
    /// 中间结果：还在变，同一段文字可能后面被改写。
    Partial(String),
    /// 一句定稿。
    Final {
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
/// 实现方在**自己的线程**上跑网络 IO：事件通过 `connect` 拿到的回调上抛，
/// 音频 / 提交 / 收尾是同步调用（内部走 channel，调用方不会被网络卡住）。
pub trait CloudProvider: Send {
    /// 这个 provider 怎么断句。
    fn endpoint_kind(&self) -> EndpointKind;
    /// provider 要求的采样率（Hz）。
    ///
    /// 会话内部始终是 16 kHz：不一样时在上传前升采样（见 `CloudStreamingSession`）。
    fn target_sample_rate(&self) -> u32;
    /// 建连。`on_event` 是事件回调：provider 在自己的线程上调用它。
    fn connect(&mut self, on_event: Box<dyn Fn(CloudEvent) + Send + Sync>) -> Result<(), String>;
    /// 发一段音频（provider 采样率、16bit、单声道）。
    fn send_audio(&mut self, pcm: &[i16]) -> Result<(), String>;
    /// 提交当前这一段（`client_commit` 家族用）。
    fn commit(&mut self) -> Result<(), String>;
    /// 收尾：等最后的结果回来。
    fn finish(&mut self) -> Result<(), String>;
}

/// 按设置造一个 provider。
///
/// provider 名字来自 `settings.cloud_provider`；地址与模型留空时用该 provider
/// 的默认值。凭据直接进配置 —— 它不会进日志，也不会回传前端。
pub fn create_provider(
    settings: &crate::settings::Settings,
    local_vad: bool,
) -> Result<Box<dyn CloudProvider>, String> {
    match settings.cloud_provider.as_str() {
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
            // 语言：auto = 不指定，让服务自己判断。
            let languages = match settings.language.as_str() {
                "auto" => Vec::new(),
                other => vec![other.to_string()],
            };
            Ok(Box::new(realtime::RealtimeProvider::new(
                realtime::RealtimeConfig {
                    base_url,
                    api_key: settings.cloud_api_key.clone(),
                    model,
                    languages,
                    // 热词复用术语表：provider 不支持就忽略。
                    keywords: settings.hotwords.clone(),
                    local_vad,
                },
            )))
        }
        other => Err(crate::code_detail("cloud.unsupportedProvider", other)),
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
    /// 已提交、还没回来的 final 的时间区间（按提交顺序）。
    pending: VecDeque<(f32, f32)>,
    /// 最近一次收到的中间结果。
    partial: String,
    segments: Vec<Segment>,
    /// 出错时的字典错误码：已定稿的句子留着，界面上另外提示。
    error: Option<String>,
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
            partial: String::new(),
            segments: Vec::new(),
            error: None,
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
                        if !text.is_empty() {
                            events.push(StreamEvent::Partial(text));
                        }
                    }
                }
                CloudEvent::Final { text, .. } => {
                    // 提交时就把区间定下来（那时本地时钟还没被静音段推走）；
                    // final 回来得晚也不影响：按提交顺序配对。
                    //
                    // **空文本也要先把区间出队**：留在队列里的话，下一条
                    // 非空 final 会配到上一个句子的区间上，整段时间戳错位。
                    let (start, end) = match self.pending.pop_front() {
                        Some(span) => span,
                        None => {
                            // provider 自己断句（server_endpoint）：到喂进去的音频末尾为止。
                            let (start, end) = self.timeline.commit_at(self.fed_end, true);
                            (
                                start as f32 / SAMPLE_RATE as f32,
                                end as f32 / SAMPLE_RATE as f32,
                            )
                        }
                    };
                    // 空句（没有识别出内容）不产句子，但区间已经配掉了。
                    if text.trim().is_empty() {
                        continue;
                    }
                    let segment = Segment { text, start, end };
                    events.push(StreamEvent::Segment(segment.clone()));
                    self.segments.push(segment);
                    self.partial.clear();
                }
            }
        }
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
            Ok(()) => {
                self.fed_end = end;
                self.fed_since_commit = true;
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
            self.pending.push_back((
                start as f32 / SAMPLE_RATE as f32,
                end as f32 / SAMPLE_RATE as f32,
            ));
            self.fed_since_commit = false;
            if self.provider.endpoint_kind() == EndpointKind::ClientCommit {
                if let Err(error) = self.provider.commit() {
                    self.error = Some(error);
                }
            }
        }
        self.drain(&mut events);
        events
    }

    /// 收尾：等 provider 把最后的结果发回来。
    fn finish(&mut self) -> Vec<StreamEvent> {
        let mut events = Vec::new();
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

    fn finish(&mut self) -> Vec<StreamEvent> {
        CloudStreamingSession::finish(self)
    }

    fn transcript(self) -> Transcript {
        Transcript {
            text: super::join_segments(self.segments.iter().map(|s| s.text.as_str())),
            segments: self.segments,
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
