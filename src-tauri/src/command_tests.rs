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
