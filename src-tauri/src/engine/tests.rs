//! `engine` 模块的单元测试（从 `engine.rs` 拆出）。

use super::*;
use std::sync::atomic::Ordering;

#[test]
fn 录音启动预留锁配置且退出复位() {
    let state = AppState::new(Settings::default());
    assert!(!state.recording_starting.swap(true, Ordering::SeqCst));
    {
        let _starting = RecordingStartGuard(&state.recording_starting);
        assert!(state.recording_starting.load(Ordering::SeqCst));
    }
    assert!(!state.recording_starting.load(Ordering::SeqCst));
}

#[test]
fn 下载守卫在提前返回时复位() {
    let state = AppState::new(Settings::default());
    state.downloading.store(true, Ordering::SeqCst);
    {
        let _guard = DownloadFlagGuard::new(state.downloading.clone());
    }
    assert!(!state.downloading.load(Ordering::SeqCst));
}

/// 整段在下载时，流式的下载按钮不该被判成 busy —— 两个下载各用各的标志。
#[test]
fn 下载标志互不阻塞() {
    let state = AppState::new(Settings::default());
    // 整段开始下载
    assert!(!state.downloading.swap(true, Ordering::SeqCst));
    // 流式照样能开始
    assert!(!state.downloading_streaming.swap(true, Ordering::SeqCst));
    // 但同一个下载不能重复进入
    assert!(state.downloading.swap(true, Ordering::SeqCst));
    assert!(state.downloading_streaming.swap(true, Ordering::SeqCst));
}
