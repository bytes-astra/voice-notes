//! 假 provider：把协议那头摘掉，供会话与编排的单测用。
//!
//! 只在测试里编译（`#[cfg(test)]`）。

use std::sync::{Arc, Mutex};

use crate::asr::cloud::{Capabilities, CloudEvent, CloudProvider, EndpointKind, PartialSemantics};
use crate::code;

/// 假 provider 的内部状态：测试拿着它就能检查发了什么、注入事件。
#[derive(Default)]
pub struct MockState {
    /// 收到的音频（i16）。
    pub sent: Vec<i16>,
    /// 提交次数。
    pub commits: usize,
    /// 收尾次数。
    pub finishes: usize,
    /// 发送失败（模拟断连）。
    pub fail: bool,
    /// 提交失败（模拟上传背压时 commit 没送出去）。
    pub fail_commit: bool,
    /// 第一次收到音频时回一条定稿（模拟 provider 的结果）。
    pub reply: Option<String>,
    /// 收尾时回一条定稿（模拟 provider 把剩余音频合成一条）。
    pub reply_on_finish: Option<String>,
    /// 上传队列满载时丢掉的 16 kHz 采样数（模拟有界队列的丢弃）。
    pub dropped_reply: usize,
    /// `connect` 交上来的事件回调。
    pub sink: Option<Box<dyn Fn(CloudEvent) + Send + Sync>>,
}

/// 假 provider。
pub struct MockProvider {
    kind: EndpointKind,
    rate: u32,
    state: Arc<Mutex<MockState>>,
}

impl MockProvider {
    /// 造一个假 provider，并返回它的状态句柄。
    pub fn new(kind: EndpointKind, rate: u32) -> (Self, Arc<Mutex<MockState>>) {
        let state = Arc::new(Mutex::new(MockState::default()));
        (
            Self {
                kind,
                rate,
                state: state.clone(),
            },
            state,
        )
    }
}

impl CloudProvider for MockProvider {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            endpoint_kind: self.kind,
            // 客户端提交族需要本地 VAD；服务端断句族不需要。
            needs_local_vad: self.kind == EndpointKind::ClientCommit,
            partial_semantics: PartialSemantics::Delta,
            supports_keywords: true,
            target_sample_rate: self.rate,
        }
    }

    fn connect(
        &mut self,
        on_event: Box<dyn Fn(CloudEvent) + Send + Sync>,
    ) -> Result<(), String> {
        self.state.lock().unwrap().sink = Some(on_event);
        Ok(())
    }

    fn send_audio(&mut self, pcm: &[i16]) -> Result<usize, String> {
        let mut state = self.state.lock().unwrap();
        // 先按约定回一条定稿：这样「provider 的结果」能进入会话。
        // 回调只往 mpsc 塞消息，不会回头锁 state —— 持锁调用是安全的。
        if let Some(text) = state.reply.take() {
            if let Some(sink) = state.sink.as_ref() {
                sink(CloudEvent::Final {
                    item_id: None,
                    text,
                    start_ms: None,
                    end_ms: None,
                });
            }
        }
        if state.fail {
            return Err(code("cloud.sendFailed"));
        }
        state.sent.extend_from_slice(pcm);
        // 模拟上传队列满：这一段没送出去（只发生一次）。
        Ok(std::mem::take(&mut state.dropped_reply))
    }

    fn commit(&mut self) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();
        if state.fail_commit {
            return Err(code("cloud.sendFailed"));
        }
        state.commits += 1;
        Ok(())
    }

    fn finish(&mut self) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();
        state.finishes += 1;
        if let Some(text) = state.reply_on_finish.take() {
            if let Some(sink) = state.sink.as_ref() {
                sink(CloudEvent::Final {
                    item_id: None,
                    text,
                    start_ms: None,
                    end_ms: None,
                });
            }
        }
        Ok(())
    }

}

/// 往假 provider 里塞一个事件。
pub fn emit(state: &Arc<Mutex<MockState>>, event: CloudEvent) {
    let guard = state.lock().unwrap();
    let sink = guard.sink.as_ref().expect("还没 connect");
    sink(event);
}
