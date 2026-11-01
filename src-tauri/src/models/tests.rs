//! `models` 模块的单元测试（从 `models.rs` 拆出）。

use super::*;

/// 「下载被取消」只有一个哨兵串，不跟字典 key 混。
#[test]
fn cancelled_sentinel_is_only_the_download_cancelled_marker() {
    assert!(is_cancelled(DOWNLOAD_CANCELLED));
    assert!(!is_cancelled("setup.cancelled"));
    assert!(!is_cancelled("error.downloadInterrupted"));
}

/// 哨兵带上细节（`code_detail` 的形状）也要认得出是「已取消」。
///
/// 整串比较会失配 → 内部哨兵以「未知错误」漏到界面上。
#[test]
fn cancelled_sentinel_with_detail_is_still_cancelled() {
    assert!(is_cancelled("download.cancelled:细节"));
    assert!(!is_cancelled("error.downloadInterrupted:细节"));
}

/// 下载被取消时，返回给界面的必须是字典 key `setup.cancelled`，
/// 不能是内部哨兵 `download.cancelled`。
///
/// 守的是 `download_asset` 里那行映射 —— 哨兵只活在内部，
/// 界面（字典查不到就显示「未知错误」）永远不该看到它。
#[test]
fn cancelled_download_returns_the_dictionary_key() {
    struct Null;
    impl ProgressSink for Null {
        fn report(&self, _: Progress) {}
    }
    let dir = std::env::temp_dir().join(format!("vn-models-cancel-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    // 取消标志一开始就置上：下载不会发起，直接走取消分支。
    let error = runtime
        .block_on(download_asset(
            &VAD,
            &dir,
            Arc::new(AtomicBool::new(true)),
            &Null,
        ))
        .expect_err("已取消的下载应该失败");
    assert_eq!(error, "setup.cancelled", "界面只该看到字典 key");
    assert_ne!(error, DOWNLOAD_CANCELLED, "内部哨兵不能漏到界面");
    let _ = fs::remove_dir_all(&dir);
}

/// 取消发生在**下载途中**时，也要翻译成字典 key。
///
/// 回归测试：之前只在「调用前就取消」时测过 —— 取消标志一开始就置上，
/// `download_asset` 在发起下载前就返回了，那行「哨兵 → 字典 key」的映射
/// 根本没被走到（把它换成 `if false`，测试照样全过）。
///
/// 这里让进度回调在收到第一个进度时置上取消标志：`fetch_one` 会在收 chunk 的
/// 循环里返回内部哨兵，由 `download_from_base` 翻译成字典 key。
#[test]
fn cancelling_during_download_is_translated_to_the_dictionary_key() {
    use std::io::{BufRead, BufReader};
    use std::net::TcpListener;

    // 本地假服务器：回一个响应，让 fetch_one 真的进入收 chunk 的循环
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();
        while reader.read_line(&mut line).unwrap() > 0 {
            if line.trim().is_empty() {
                break;
            }
            line.clear();
        }
        // 回一个 body，让客户端的 chunk 循环跑起来
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\nnew")
            .unwrap();
        stream.flush().unwrap();
        // 等客户端读完再关
        let mut sink = [0u8; 1];
        let _ = stream.read(&mut sink);
    });

    static ASSET: Asset = Asset {
        name: "tiny.onnx",
        path: "/tiny.onnx",
        bytes: 3,
        sha256: "11507a0e2f5e69d5dfa40a62a1bd7b6ee57e6bcd85c67c9b8431b36fff21c437",
    };

    // 进度回调一有动静就置上取消标志 → fetch_one 在收 chunk 的循环里返回哨兵
    struct CancelOnProgress {
        cancel: Arc<AtomicBool>,
    }
    impl ProgressSink for CancelOnProgress {
        fn report(&self, _: Progress) {
            self.cancel.store(true, Ordering::Relaxed);
        }
    }

    let dir = std::env::temp_dir().join(format!("vn-models-cancel-mid-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let cancel = Arc::new(AtomicBool::new(false));
    let sink = CancelOnProgress {
        cancel: cancel.clone(),
    };
    let outcome = runtime.block_on(download_from_base(
        &reqwest::Client::new(),
        &base,
        &ASSET,
        &dir.join(ASSET.name),
        &cancel,
        &sink,
    ));
    server.join().unwrap();
    assert!(
        matches!(outcome, DownloadOutcome::Cancelled),
        "下载途中取消应该被认出来: {outcome:?}"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// 只有 model + tokens、没有 silero_vad.onnx 的目录不能报「可用」。
///
/// 回归测试：之前这种目录会被当成已就绪，录完却一句话都识别不出来。
#[test]
fn model_without_vad_is_not_usable() {
    let dir = std::env::temp_dir().join(format!("vn-models-novad-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(MODEL_FILE), b"model").unwrap();
    fs::write(dir.join(TOKENS_FILE), b"tokens").unwrap();
    assert!(
        model_paths_in(&dir, "custom").is_none(),
        "没有 VAD 的目录不能报可用"
    );

    fs::write(dir.join(VAD_FILE), b"vad").unwrap();
    assert!(
        model_paths_in(&dir, "custom").is_some(),
        "model + tokens + silero_vad.onnx 齐全才可用"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// 应用数据目录里被截断的模型文件不能被当成「已就绪」。
#[test]
fn truncated_app_data_model_is_not_ready() {
    let dir = std::env::temp_dir().join(format!("vn-models-broken-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();

    let truncated = dir.join(MODEL_FILE);
    fs::write(&truncated, b"not a model").unwrap();
    assert!(!app_data_file_ok(&truncated), "截断的模型应该被判为不可用");
    // 别的来源的 model.onnx 没有已知大小，放行
    let unknown = dir.join("model.onnx");
    fs::write(&unknown, b"x").unwrap();
    assert!(app_data_file_ok(&unknown), "未知大小的文件名应该放行");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn streaming_model_requires_complete_assets() {
    let dir = std::env::temp_dir().join(format!("vn-streaming-ready-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    for asset in STREAM_ASSETS {
        fs::write(dir.join(asset.name), vec![0; asset.bytes as usize]).unwrap();
    }
    assert!(resolve_streaming_in(&dir).is_some(), "完整模型应可用");

    fs::write(dir.join(STREAM_ENCODER.name), b"truncated").unwrap();
    assert!(resolve_streaming_in(&dir).is_none(), "截断的模型不能报就绪");
    fs::write(
        dir.join(STREAM_ENCODER.name),
        vec![0; STREAM_ENCODER.bytes as usize],
    )
    .unwrap();
    fs::write(dir.join(STREAM_VAD.name), b"truncated").unwrap();
    assert!(
        resolve_streaming_in(&dir).is_none(),
        "截断的 VAD 不能报就绪"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// 真网络下载（只有 1.8MB 的 VAD），默认跳过：
///   VOICE_NOTES_DOWNLOAD_TEST=1 cargo test --lib downloads_vad -- --nocapture
#[test]
fn downloads_vad_from_network() {
    if std::env::var("VOICE_NOTES_DOWNLOAD_TEST").is_err() {
        eprintln!("跳过：未设置 VOICE_NOTES_DOWNLOAD_TEST");
        return;
    }
    struct Null;
    impl ProgressSink for Null {
        fn report(&self, _: Progress) {}
    }
    let dir = std::env::temp_dir().join(format!("vn-models-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime
        .block_on(download_asset(
            &VAD,
            &dir,
            Arc::new(AtomicBool::new(false)),
            &Null,
        ))
        .expect("下载 VAD");
    let size = fs::metadata(dir.join(VAD.name)).unwrap().len();
    assert_eq!(size, VAD.bytes);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn oversized_partial_file_is_replaced_by_a_fresh_download() {
    use std::io::{BufRead, BufReader};
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = String::new();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            if line == "\r\n" || line.is_empty() {
                break;
            }
            request.push_str(&line);
        }
        assert!(request.to_ascii_lowercase().contains("range: bytes=0-"), "{request}");
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\nnew")
            .unwrap();
    });
    struct Null;
    impl ProgressSink for Null {
        fn report(&self, _: Progress) {}
    }
    static ASSET: Asset = Asset {
        name: "tiny.onnx",
        path: "/tiny.onnx",
        bytes: 3,
        sha256: "11507a0e2f5e69d5dfa40a62a1bd7b6ee57e6bcd85c67c9b8431b36fff21c437",
    };
    let dir = std::env::temp_dir().join(format!("vn-oversized-part-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("tiny.part"), b"old-oversized").unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime
        .block_on(fetch_one(
            &reqwest::Client::new(),
            &base,
            &ASSET,
            &dir.join(ASSET.name),
            &Null,
            &AtomicBool::new(false),
        ))
        .expect("过大的 .part 应清理后重新下载");
    server.join().unwrap();
    assert_eq!(fs::read(dir.join("tiny.onnx")).unwrap(), b"new");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn assets_are_consistent() {
    for asset in ASSETS {
        assert!(asset.bytes > 0);
        assert_eq!(asset.sha256.len(), 64);
        assert!(asset.path.starts_with('/'));
    }
}
