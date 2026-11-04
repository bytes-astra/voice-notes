//! `engine` 模块的单元测试（从 `engine.rs` 拆出）。

use super::*;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::time::Duration;

// ──────────────────── 引擎身份：复用还是重建 ────────────────────

/// 假的整段引擎：只带身份，够验「复用还是重建」。
struct FakeEngine {
    key: OfflineEngineKey,
}

/// 假的流式引擎。
struct FakeStreaming {
    key: StreamingEngineKey,
}

/// 假整段引擎的身份：照着设置里的模型目录 / 语言 / ITN 算。
fn fake_offline_key(settings: &Settings) -> OfflineEngineKey {
    OfflineEngineKey {
        model_dir: settings.model_dir.clone().unwrap_or_default(),
        language: settings.language.clone(),
        itn: settings.itn,
    }
}

/// 装一个假整段引擎（走生产同一条 `install_engine` 路径）。
fn 装假整段(
    slot: &Mutex<Option<Arc<FakeEngine>>>,
    settings: &Mutex<Settings>,
) -> Result<Arc<FakeEngine>, String> {
    install_engine(
        slot,
        settings,
        |engine: &FakeEngine| engine.key.clone(),
        |s: &Settings| {
            let key = fake_offline_key(s);
            Ok((key.clone(), key))
        },
        |key: OfflineEngineKey| Ok(FakeEngine { key }),
        "error.engineState",
    )
}

/// 装一个假流式引擎（走生产同一条 `install_engine` 路径）。
fn 装假流式(
    slot: &Mutex<Option<Arc<FakeStreaming>>>,
    settings: &Mutex<Settings>,
) -> Result<Arc<FakeStreaming>, String> {
    install_engine(
        slot,
        settings,
        |engine: &FakeStreaming| engine.key.clone(),
        |s: &Settings| {
            let key = StreamingEngineKey {
                hotwords: s.hotwords.clone(),
            };
            Ok((key.clone(), key))
        },
        |key: StreamingEngineKey| Ok(FakeStreaming { key }),
        "error.streamEngineState",
    )
}

/// 设置没变就该复用同一个引擎 —— 不该每次都重新加载 239 MB 模型。
#[test]
fn 设置没变就复用引擎() {
    let slot: Mutex<Option<Arc<FakeEngine>>> = Mutex::new(None);
    let settings = Mutex::new(Settings::default());
    let first = 装假整段(&slot, &settings).expect("装引擎");
    let second = 装假整段(&slot, &settings).expect("装引擎");
    assert!(
        Arc::ptr_eq(&first, &second),
        "设置没变就该复用同一个引擎，不该重新加载模型"
    );
}

/// 换了模型目录，旧引擎必须作废。
///
/// 陈旧判定原来只看语言和 ITN：换了模型目录照样复用旧引擎，
/// 新模型目录被静默吞掉 —— 界面上看不出任何异常。
#[test]
fn 换了模型目录就重建引擎() {
    let slot: Mutex<Option<Arc<FakeEngine>>> = Mutex::new(None);
    let settings = Mutex::new(Settings::default());
    let first = 装假整段(&slot, &settings).expect("装引擎");
    settings.lock().unwrap().model_dir = Some("/models/other".into());
    let second = 装假整段(&slot, &settings).expect("装引擎");
    assert!(
        !Arc::ptr_eq(&first, &second),
        "换了模型目录必须重建引擎 —— 否则新模型目录被静默吞掉"
    );
}

/// 语言变了必须重建（这条原来就有，守住别改回去）。
#[test]
fn 换了语言就重建引擎() {
    let slot: Mutex<Option<Arc<FakeEngine>>> = Mutex::new(None);
    let settings = Mutex::new(Settings::default());
    let first = 装假整段(&slot, &settings).expect("装引擎");
    settings.lock().unwrap().language = "en".into();
    let second = 装假整段(&slot, &settings).expect("装引擎");
    assert!(
        !Arc::ptr_eq(&first, &second),
        "换了语言必须重建引擎 —— 否则新语言被静默吞掉"
    );
}

/// 热词变了必须重建流式引擎 —— 词表是建解码器时定死的。
#[test]
fn 换了热词就重建流式引擎() {
    let slot: Mutex<Option<Arc<FakeStreaming>>> = Mutex::new(None);
    let settings = Mutex::new(Settings::default());
    let first = 装假流式(&slot, &settings).expect("装引擎");
    settings.lock().unwrap().hotwords = vec!["番茄".to_string()];
    let second = 装假流式(&slot, &settings).expect("装引擎");
    assert!(
        !Arc::ptr_eq(&first, &second),
        "换了热词必须重建流式引擎 —— 否则新术语表被静默吞掉"
    );
}

/// 装引擎必须用**当前**设置，不能用进锁前读到的旧快照。
///
/// 设置是在锁外被改的（set_model_dir / set_hotwords）：读设置和装引擎之间
/// 只要有缝，旧快照就会把新设置顶掉，而且陈旧判定不自愈 —— 新设置被吞掉。
#[test]
fn 等锁期间设置改了也要用新设置() {
    let slot = Arc::new(Mutex::new(None::<Arc<FakeEngine>>));
    let settings = Arc::new(Mutex::new(Settings::default()));
    let old_language = settings.lock().unwrap().language.clone();

    // 线程 A 持住引擎槽锁：后面的装引擎进不来。
    let held = slot.clone();
    let (tx, rx) = mpsc::channel();
    let holder = std::thread::spawn(move || {
        let _guard = held.lock().unwrap();
        tx.send(()).unwrap();
        std::thread::sleep(Duration::from_millis(300));
    });
    rx.recv().expect("线程 A 持锁");

    // 线程 B 去装引擎（阻塞在槽锁上）。
    let slot2 = slot.clone();
    let settings2 = settings.clone();
    let loader = std::thread::spawn(move || 装假整段(&slot2, &settings2));

    std::thread::sleep(Duration::from_millis(100)); // 让 B 真的堵在锁上
    settings.lock().unwrap().language = "en".into(); // 等锁期间改设置
    holder.join().expect("线程 A 放锁");

    let built = loader.join().expect("线程 B 结束").expect("装引擎");
    assert_ne!(
        built.key.language, old_language,
        "装引擎必须用当前设置，不能用进锁前读到的旧快照"
    );
    assert_eq!(built.key.language, "en", "装引擎必须用当前设置");
}

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
