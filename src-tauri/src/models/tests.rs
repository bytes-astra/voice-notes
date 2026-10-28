//! `models` 模块的单元测试（从 `models.rs` 拆出）。

use super::*;

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
