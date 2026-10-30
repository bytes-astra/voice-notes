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

#[test]
fn 词表只收单字符号() {
    let tokens = "\
<blk> 0
<sos/eos> 1
<unk> 2
<0x00> 3
<0xFF> 258
你 259
好 260
番 261
";
    let dir = std::env::temp_dir().join("voice-notes-vocab-test");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("tokens.txt");
    std::fs::write(&path, tokens).unwrap();

    let vocab = streaming_vocab_chars(&path.display().to_string());
    assert!(vocab.contains(&'你'));
    assert!(vocab.contains(&'好'));
    // 特殊符号不是能用的字
    assert!(!vocab.contains(&'<'));
    assert!(!vocab.contains(&'0'));
    std::fs::remove_file(&path).ok();
}

#[test]
fn 词表外的字整条热词作废() {
    let mut vocab = HashSet::new();
    vocab.insert('你');
    vocab.insert('好');
    vocab.insert('帮');
    vocab.insert('我');
    vocab.insert('个');
    vocab.insert('忙');

    let hotwords = vec![
        "你好".to_string(),
        "帮我个忙".to_string(),
        // 「渴」不在表里 —— 整条作废（sherpa 也是逐字查表）
        "渴了".to_string(),
        "GDP".to_string(),
    ];
    let (usable, ignored) = split_hotwords_by_vocab(&hotwords, &vocab);
    assert_eq!(usable, vec!["你好".to_string(), "帮我个忙".to_string()]);
    assert_eq!(ignored, vec!["渴了".to_string(), "GDP".to_string()]);
}

#[test]
fn 读不到词表时热词全部保留() {
    let hotwords = vec!["你好".to_string(), "渴了".to_string()];
    let (usable, ignored) = split_hotwords_by_vocab(&hotwords, &HashSet::new());
    assert_eq!(usable, hotwords);
    assert!(ignored.is_empty());
}
