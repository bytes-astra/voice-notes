//! `lib` 模块的单元测试（从 `lib.rs` 拆出）。

use std::sync::atomic::Ordering;

use super::*;
use crate::settings::Settings;

#[test]
fn recognition_config_is_locked_while_recording() {
    assert!(super::ensure_config_unlocked(false).is_ok());
    assert_eq!(
        super::ensure_config_unlocked(true),
        Err("record.configLocked".into())
    );
}

/// 认不出来的 kind 不能猜着取消 —— 猜错会把另一个正在跑的下载停掉。
#[test]
fn unknown_download_kind_cancels_nothing() {
    let state = AppState::new(Settings::default());
    super::cancel_download_of(&state, Some("whatever"));
    assert!(
        !state.cancel_download.load(Ordering::SeqCst),
        "认不出的 kind 不该取消整段下载"
    );
    assert!(
        !state.cancel_download_streaming.load(Ordering::SeqCst),
        "认不出的 kind 不该取消流式下载"
    );
}

/// 指名取消就只停那一条线。
#[test]
fn cancel_download_targets_the_named_line() {
    let state = AppState::new(Settings::default());
    super::cancel_download_of(&state, Some("streaming"));
    assert!(state.cancel_download_streaming.load(Ordering::SeqCst));
    assert!(!state.cancel_download.load(Ordering::SeqCst));

    let state = AppState::new(Settings::default());
    super::cancel_download_of(&state, Some("offline"));
    assert!(state.cancel_download.load(Ordering::SeqCst));
    assert!(!state.cancel_download_streaming.load(Ordering::SeqCst));
}

/// 取消录音不能动「正在启动」标志。
///
/// 那个标志是 start 命令的 guard 独占的：取消时清掉它的话，
/// start 还没落定的窗口里第二个 start 就能通过检查 ——
/// 两个命令各开一次麦克风、各起一个识别线程。
#[test]
fn cancel_does_not_release_the_starting_flag() {
    let state = AppState::new(Settings::default());
    state.recording_starting.store(true, Ordering::SeqCst);
    super::cancel_recording_impl(&state).expect("取消录音");
    assert!(
        state.recording_starting.load(Ordering::SeqCst),
        "取消录音不该动「正在启动」标志 —— 那是 start 命令的 guard 独占的"
    );
}

/// 本地 VAD 的前置条件：不需要就放行，需要且缺失就报错。
///
/// 「缺 VAD」不能被当成「改用服务端断句」—— 那是另一个能力。
#[test]
fn vad_precondition_follows_capability() {
    use crate::asr::cloud::{Capabilities, EndpointKind, PartialSemantics};
    let client_commit = Capabilities {
        endpoint_kind: EndpointKind::ClientCommit,
        needs_local_vad: true,
        partial_semantics: PartialSemantics::Delta,
        supports_keywords: true,
        target_sample_rate: 16_000,
    };
    let server_endpoint = Capabilities {
        endpoint_kind: EndpointKind::ServerEndpoint,
        needs_local_vad: false,
        ..client_commit
    };
    assert!(
        vad_precondition(&client_commit, true).is_ok(),
        "已就绪应放行"
    );
    assert_eq!(
        vad_precondition(&client_commit, false).unwrap_err(),
        "cloud.vadMissing"
    );
    assert!(
        vad_precondition(&server_endpoint, false).is_ok(),
        "不需要本地 VAD 不该报错"
    );
}
