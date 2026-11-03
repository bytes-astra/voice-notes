//! `engine` 模块的单元测试（从 `engine.rs` 拆出）。

use super::*;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::time::Duration;

/// 快路径在锁被占着时不能干等。
///
/// 锁被占着 = 别的线程正在加载模型（239 MB / 161 MB / 294 MB，要好几秒）。
/// 在 async 执行器线程上死等，整个执行器就卡住了 —— 命令的响应会被
/// 拖到模型加载完，用户看着像界面冻住了。
#[test]
fn 快路径在锁被占着时不干等() {
    let slot = Arc::new(Mutex::new(Some(Arc::new(7u8))));
    let (tx, rx) = mpsc::channel();
    let worker = slot.clone();
    std::thread::spawn(move || {
        let _held = worker.lock().unwrap(); // 持锁不放
        let _ = tx.send(cached_engine(&worker));
        // 持锁睡一会儿：干等的实现会卡在这里面
        std::thread::sleep(Duration::from_secs(2));
    });

    let started = Instant::now();
    let cached = rx
        .recv_timeout(Duration::from_secs(5))
        .expect("快路径应该立刻返回，不该等锁");
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "快路径在锁被占着时干等了 {:?}",
        started.elapsed()
    );
    assert!(cached.is_none(), "锁被占着时快路径应该拿不到引擎");
}

/// 锁空着的时候快路径照常拿到引擎。
#[test]
fn 快路径在锁空时拿到引擎() {
    let slot: Mutex<Option<Arc<u8>>> = Mutex::new(Some(Arc::new(3)));
    assert_eq!(cached_engine(&slot).map(|engine| *engine), Some(3));
    let empty: Mutex<Option<Arc<u8>>> = Mutex::new(None);
    assert!(cached_engine(&empty).is_none());
}

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

/// 加载结果必须映射成 ready / error 两种状态事件。
///
/// 流式侧原来只发 loading + ready，失败时不发 error —— 前端 state.engine
/// 会永远停在 loading，徽标卡在「加载中」。抽成纯函数才能脱开 AppHandle 守住。
#[test]
fn 加载结果映射成引擎状态() {
    let ok = engine_state_for::<()>(&Ok(()));
    assert_eq!(ok.phase, "ready");
    assert!(ok.message.is_none());

    let failed = engine_state_for::<()>(&Err("model.streamLoadFailed".into()));
    assert_eq!(failed.phase, "error");
    assert_eq!(failed.message.as_deref(), Some("model.streamLoadFailed"));
}

/// 整段和流式的 VAD 参数必须完全一致 —— 除了 max_speech_duration。
///
/// 两边各写一份 VAD 配置字面量时，改一边忘了另一边，断句行为就会分叉：
/// 同一段音频在两个模式里切出不同数量的句子，而界面上看不出任何异常。
///
/// 这条守的是「两边走同一个 `vad_config`」这个结构事实：参数不同就一定
/// 是有人绕过 builder 手写了配置。
#[test]
fn 两模式_vad_参数一致() {
    let offline = vad_config(Some("/tmp/vad.onnx".to_string()), MAX_SEGMENT_SECONDS);
    let streaming = vad_config(Some("/tmp/vad.onnx".to_string()), STREAM_MAX_SPEECH_SECONDS);

    // 唯一允许不同的是「一句话最长」阈值（两条路径用它的地方不同）。
    assert_ne!(
        offline.silero_vad.max_speech_duration,
        streaming.silero_vad.max_speech_duration,
        "两模式的 max_speech_duration 本来就该不同（见两个常量的注释）"
    );

    // 其余全部必须一致。
    assert_eq!(
        offline.silero_vad.threshold, streaming.silero_vad.threshold,
        "VAD threshold 分叉了"
    );
    assert_eq!(
        offline.silero_vad.min_silence_duration, streaming.silero_vad.min_silence_duration,
        "VAD min_silence_duration 分叉了 —— 断句行为会不一样"
    );
    assert_eq!(
        offline.silero_vad.min_speech_duration, streaming.silero_vad.min_speech_duration,
        "VAD min_speech_duration 分叉了"
    );
    assert_eq!(
        offline.silero_vad.window_size, streaming.silero_vad.window_size,
        "VAD window_size 分叉了"
    );
    assert_eq!(offline.sample_rate, streaming.sample_rate, "VAD sample_rate 分叉了");
    assert_eq!(offline.num_threads, streaming.num_threads, "VAD num_threads 分叉了");
    assert_eq!(offline.provider, streaming.provider, "VAD provider 分叉了");
    assert_eq!(offline.debug, streaming.debug, "VAD debug 分叉了");
}
