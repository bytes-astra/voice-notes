//! 模型文件定位与首次运行下载（SenseVoiceSmall INT8 + Silero VAD）。

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use serde::Serialize;
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Manager};

pub const MODEL_FILE: &str = "model.int8.onnx";
pub const TOKENS_FILE: &str = "tokens.txt";
pub const VAD_FILE: &str = "silero_vad.onnx";

/// 官方源（直连慢，但最权威）
const HF: &str = "https://huggingface.co";
/// 国内镜像（默认走这个：国内直连 huggingface.co 经常超时）
const MIRROR: &str = "https://hf-mirror.com";

pub struct Asset {
    pub name: &'static str,
    pub path: &'static str,
    pub bytes: u64,
    pub sha256: &'static str,
}

pub const MODEL: Asset = Asset {
    name: MODEL_FILE,
    path: "/csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17/resolve/2365baeacb507f821a0c8120fcee3d484dba7a07/model.int8.onnx",
    bytes: 239_233_841,
    sha256: "c71f0ce00bec95b07744e116345e33d8cbbe08cef896382cf907bf4b51a2cd51",
};

pub const TOKENS: Asset = Asset {
    name: TOKENS_FILE,
    path: "/csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17/resolve/2365baeacb507f821a0c8120fcee3d484dba7a07/tokens.txt",
    bytes: 315_894,
    sha256: "f449eb28dc567533d7fa59be34e2abca8784f771850c78a47fb731a31429a1dc",
};

pub const VAD: Asset = Asset {
    name: VAD_FILE,
    path: "/csukuangfj/vad/resolve/fba88cd2e921609e7675c3aaf51e0b9b295da4bc/silero_vad.onnx",
    bytes: 1_807_522,
    sha256: "a35ebf52fd3ce5f1469b2a36158dba761bc47b973ea3382b3186ca15b1f5af28",
};

pub const ASSETS: [&Asset; 3] = [&MODEL, &TOKENS, &VAD];

// ── 流式模型：sherpa-onnx-streaming-zipformer-zh-2025-06-30 (int8) ──
// 中文，真流式 transducer。文件固定到 commit ad658fa。
pub const STREAM_DIR: &str = "streaming";

pub const STREAM_ENCODER: Asset = Asset {
    name: "encoder.int8.onnx",
    path: "/csukuangfj/sherpa-onnx-streaming-zipformer-zh-int8-2025-06-30/resolve/ad658fa0201659a09ea3c176129a191c77ecae8f/encoder.int8.onnx",
    bytes: 161_141_793,
    sha256: "5ac51e27981bb4dab01bb9be4958453ba50c3b61c063ddda0eab23fd3671aa4f",
};

pub const STREAM_DECODER: Asset = Asset {
    name: "decoder.onnx",
    path: "/csukuangfj/sherpa-onnx-streaming-zipformer-zh-int8-2025-06-30/resolve/ad658fa0201659a09ea3c176129a191c77ecae8f/decoder.onnx",
    bytes: 5_165_083,
    sha256: "06522ad63cec0fdf6809f4e1db9bb4f7d710c34582e3b35db62ac60eccafac7e",
};

pub const STREAM_JOINER: Asset = Asset {
    name: "joiner.int8.onnx",
    path: "/csukuangfj/sherpa-onnx-streaming-zipformer-zh-int8-2025-06-30/resolve/ad658fa0201659a09ea3c176129a191c77ecae8f/joiner.int8.onnx",
    bytes: 1_033_416,
    sha256: "b34584dc6f561089e1d747fedebb3765f2caa72c927ef54d7ca55e5ae40a814b",
};

pub const STREAM_TOKENS: Asset = Asset {
    name: "tokens.txt",
    path: "/csukuangfj/sherpa-onnx-streaming-zipformer-zh-int8-2025-06-30/resolve/ad658fa0201659a09ea3c176129a191c77ecae8f/tokens.txt",
    bytes: 20_628,
    sha256: "6193c7ea1c96d0d9a1e9652789b40d13a8a913b434a5451e93158f5a09fd6652",
};

pub const STREAM_VAD: Asset = Asset {
    name: "silero_vad.onnx",
    path: "/csukuangfj/vad/resolve/fba88cd2e921609e7675c3aaf51e0b9b295da4bc/silero_vad.onnx",
    bytes: 1_807_522,
    sha256: "a35ebf52fd3ce5f1469b2a36158dba761bc47b973ea3382b3186ca15b1f5af28",
};

pub const STREAM_ASSETS: [&Asset; 5] = [
    &STREAM_ENCODER,
    &STREAM_DECODER,
    &STREAM_JOINER,
    &STREAM_TOKENS,
    &STREAM_VAD,
];

// ── 标点模型：CT-Transformer zh-en（fp32，约 281 MiB） ──
pub const PUNCT_DIR: &str = "punct";

pub const PUNCT: Asset = Asset {
    name: "model.onnx",
    path: "/csukuangfj/sherpa-onnx-punct-ct-transformer-zh-en-vocab272727-2024-04-12/resolve/432aeba669265e7aeb06b9359753419683b38597/model.onnx",
    bytes: 294_372_519,
    sha256: "e93593a6dbd69a07f8734ef269dbe861a379755f8d1c8354719432116f2c44bd",
};

pub fn resolve_punctuation(app: &AppHandle) -> Option<String> {
    let path = app
        .path()
        .app_data_dir()
        .ok()?
        .join("models")
        .join(PUNCT_DIR)
        .join(PUNCT.name);
    exists(&path).then(|| path.to_string_lossy().into_owned())
}

pub fn punctuation_install_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("无法定位应用数据目录: {e}"))?
        .join("models")
        .join(PUNCT_DIR);
    fs::create_dir_all(&dir).map_err(|e| format!("无法创建标点模型目录: {e}"))?;
    Ok(dir)
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamingPaths {
    pub dir: String,
    pub encoder: String,
    pub decoder: String,
    pub joiner: String,
    pub tokens: String,
    pub vad: Option<String>,
}

pub fn streaming_total_bytes() -> u64 {
    STREAM_ASSETS.iter().map(|a| a.bytes).sum()
}

/// 流式模型固定放在应用数据目录的 models/streaming 下。
pub fn resolve_streaming(app: &AppHandle) -> Option<StreamingPaths> {
    let dir = app
        .path()
        .app_data_dir()
        .ok()?
        .join("models")
        .join(STREAM_DIR);
    let encoder = dir.join(STREAM_ENCODER.name);
    let decoder = dir.join(STREAM_DECODER.name);
    let joiner = dir.join(STREAM_JOINER.name);
    let tokens = dir.join(STREAM_TOKENS.name);
    if !(exists(&encoder) && exists(&decoder) && exists(&joiner) && exists(&tokens)) {
        return None;
    }
    Some(StreamingPaths {
        dir: dir.to_string_lossy().into_owned(),
        encoder: encoder.to_string_lossy().into_owned(),
        decoder: decoder.to_string_lossy().into_owned(),
        joiner: joiner.to_string_lossy().into_owned(),
        tokens: tokens.to_string_lossy().into_owned(),
        vad: Some(dir.join(STREAM_VAD.name))
            .filter(|path| exists(path))
            .map(|path| path.to_string_lossy().into_owned()),
    })
}

pub fn streaming_install_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("无法定位应用数据目录: {e}"))?
        .join("models")
        .join(STREAM_DIR);
    fs::create_dir_all(&dir).map_err(|e| format!("无法创建流式模型目录: {e}"))?;
    Ok(dir)
}

/// 下载流式模型 **和标点模型**（一起下，用户不用再单独操作一次）。
///
/// 标点模型是可选功能：它下载失败不算整体失败 —— 流式模型已经可用，
/// 只是句子没有标点。设置页会显示「标点模型未下载」。
pub async fn download_streaming(
    dest_dir: &Path,
    punct_dir: &Path,
    cancel: Arc<AtomicBool>,
    sink: &dyn ProgressSink,
) -> Result<(), String> {
    for asset in STREAM_ASSETS {
        download_asset(asset, dest_dir, cancel.clone(), sink).await?;
    }
    if let Err(err) = download_asset(&PUNCT, punct_dir, cancel.clone(), sink).await {
        // 用户按了取消：不能把「取消」当成「标点降级」报成功。
        if cancel.load(Ordering::Relaxed) {
            return Err("已取消下载".into());
        }
        eprintln!("[voice-notes] 标点模型下载失败，降级为无标点：{err}");
    }
    Ok(())
}

#[derive(Clone, Debug, Serialize)]
pub struct ModelPaths {
    pub dir: String,
    pub model: String,
    pub tokens: String,
    pub vad: Option<String>,
    pub source: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct Progress {
    pub resource: &'static str,
    pub done: u64,
    pub total: u64,
    pub phase: &'static str,
    pub source: String,
    pub message: Option<String>,
}

fn exists(path: &Path) -> bool {
    path.is_file()
}

/// 应用数据目录里的文件是我们自己下载的：大小必须和常量对得上。
///
/// 只判断「存在」的话，被截断/覆盖的文件也会被当成「模型已就绪」，
/// 一录音才报「文件可能损坏」。自定义目录不校验（用户可能用别的版本）。
fn app_data_file_ok(path: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let expected = match name {
        MODEL_FILE => Some(MODEL.bytes),
        TOKENS_FILE => Some(TOKENS.bytes),
        VAD_FILE => Some(VAD.bytes),
        // 别处带来的 model.onnx 没有已知大小，放行。
        _ => None,
    };
    match expected {
        Some(bytes) => fs::metadata(path).map(|meta| meta.len() == bytes).unwrap_or(false),
        None => true,
    }
}

fn candidate_dirs(app: &AppHandle, settings_dir: Option<&str>) -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    if let Some(dir) = settings_dir {
        out.push((PathBuf::from(dir), "custom".to_string()));
    }
    if let Ok(data) = app.path().app_data_dir() {
        out.push((data.join("models"), "app_data".to_string()));
    }
    out
}

fn find_vad(dir: &Path) -> Option<PathBuf> {
    let candidates = [
        dir.join(VAD_FILE),
        dir.join("silero").join(VAD_FILE),
        dir.parent()
            .map(|p| p.join("silero").join(VAD_FILE))
            .unwrap_or_default(),
        dir.parent().map(|p| p.join(VAD_FILE)).unwrap_or_default(),
    ];
    candidates
        .into_iter()
        .find(|p| !p.as_os_str().is_empty() && exists(p))
}

/// 单个目录里的可用模型。VAD 是必需的：整段模式的边录边识别和文件转写的分句
/// 都靠它，没有 VAD 时录完只会得到空结果。
fn model_paths_in(dir: &Path, source: &str) -> Option<ModelPaths> {
    let model = [dir.join(MODEL_FILE), dir.join("model.onnx")]
        .into_iter()
        .find(|path| exists(path))?;
    let tokens = dir.join(TOKENS_FILE);
    if !exists(&tokens) {
        return None;
    }
    // 应用数据目录里的文件是自己下的，大小对不上就是坏文件，不能报「已就绪」
    if source == "app_data" && !(app_data_file_ok(&model) && app_data_file_ok(&tokens)) {
        return None;
    }
    let vad = find_vad(dir).filter(|path| source != "app_data" || app_data_file_ok(path))?;
    Some(ModelPaths {
        dir: dir.to_string_lossy().into_owned(),
        model: model.to_string_lossy().into_owned(),
        tokens: tokens.to_string_lossy().into_owned(),
        vad: Some(vad.to_string_lossy().into_owned()),
        source: source.to_string(),
    })
}

/// 按「自定义目录 > 应用数据」的顺序找出可用模型。
pub fn resolve(app: &AppHandle, settings_dir: Option<&str>) -> Option<ModelPaths> {
    candidate_dirs(app, settings_dir)
        .into_iter()
        .find_map(|(dir, source)| model_paths_in(&dir, &source))
}

/// 应用数据目录下的模型落盘位置。
pub fn install_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("无法定位应用数据目录: {e}"))?
        .join("models");
    fs::create_dir_all(&dir).map_err(|e| format!("无法创建模型目录: {e}"))?;
    Ok(dir)
}

/// 下载进度回调；Tauri 层发射事件，测试里直接收集。
pub trait ProgressSink: Send + Sync {
    fn report(&self, progress: Progress);
}

fn hash_file(path: &Path) -> Result<String, String> {
    let mut file = fs::File::open(path).map_err(|e| format!("无法读取 {path:?}: {e}"))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

async fn fetch_one(
    client: &reqwest::Client,
    base: &str,
    asset: &Asset,
    dest: &Path,
    sink: &dyn ProgressSink,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let part = dest.with_extension("part");
    let offset = fs::metadata(&part)
        .map(|m| m.len())
        .unwrap_or(0)
        .min(asset.bytes);
    let url = format!("{base}{}", asset.path);
    let mut last_emit = Instant::now();
    let mut done = offset;

    if offset < asset.bytes {
        let resp = client
            .get(&url)
            .header(reqwest::header::RANGE, format!("bytes={offset}-"))
            .header(reqwest::header::USER_AGENT, "voice-notes/0.1")
            .send()
            .await
            .map_err(|e| format!("连接失败: {e}"))?;

        let status = resp.status();
        if status == reqwest::StatusCode::RANGE_NOT_SATISFIABLE {
            // 已经有完整文件，直接进入校验。
        } else if !status.is_success() {
            return Err(format!("HTTP {status}"));
        } else {
            let partial = status == reqwest::StatusCode::PARTIAL_CONTENT;
            if !partial {
                done = 0;
            }
            let mut file = fs::OpenOptions::new()
                .create(true)
                .append(partial)
                .write(true)
                .truncate(!partial)
                .open(&part)
                .map_err(|e| format!("无法写入缓存文件: {e}"))?;
            let mut stream = resp.bytes_stream();
            while let Some(chunk) = stream.next().await {
                if cancel.load(Ordering::Relaxed) {
                    return Err("cancelled".into());
                }
                let chunk = chunk.map_err(|e| format!("下载中断: {e}"))?;
                file.write_all(&chunk)
                    .map_err(|e| format!("写入失败: {e}"))?;
                done += chunk.len() as u64;
                if last_emit.elapsed() > Duration::from_millis(120) {
                    last_emit = Instant::now();
                    sink.report(Progress {
                        resource: asset.name,
                        done,
                        total: asset.bytes,
                        phase: "downloading",
                        source: base.to_string(),
                        message: None,
                    });
                }
            }
            file.flush().ok();
            drop(file);
        }
    }

    let size = fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    if size != asset.bytes {
        return Err(format!("大小不符：{size} / {}", asset.bytes));
    }

    sink.report(Progress {
        resource: asset.name,
        done: size,
        total: asset.bytes,
        phase: "verifying",
        source: base.to_string(),
        message: None,
    });
    // 239 MB 的哈希在阻塞线程里算，别堵住 async 执行器。
    let part_for_hash = part.clone();
    let digest = tauri::async_runtime::spawn_blocking(move || hash_file(&part_for_hash))
        .await
        .map_err(|e| format!("校验失败: {e}"))??;
    if digest != asset.sha256 {
        let _ = fs::remove_file(&part);
        return Err("sha256 校验失败，文件已丢弃".into());
    }
    if dest.exists() {
        let _ = fs::remove_file(dest);
    }
    fs::rename(&part, dest).map_err(|e| format!("落盘失败: {e}"))?;
    sink.report(Progress {
        resource: asset.name,
        done: size,
        total: asset.bytes,
        phase: "complete",
        source: base.to_string(),
        message: None,
    });
    Ok(())
}

/// 下载单个资源到目录；已存在且大小正确则跳过。返回是否走了网络。
pub async fn download_asset(
    asset: &'static Asset,
    dest_dir: &Path,
    cancel: Arc<AtomicBool>,
    sink: &dyn ProgressSink,
) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())?;
    let dest = dest_dir.join(asset.name);
    if fs::metadata(&dest)
        .map(|m| m.len() == asset.bytes)
        .unwrap_or(false)
    {
        sink.report(Progress {
            resource: asset.name,
            done: asset.bytes,
            total: asset.bytes,
            phase: "complete",
            source: "cache".into(),
            message: None,
        });
        return Ok(());
    }

    let mut errors = Vec::new();
    // 镜像优先：国内直连 huggingface.co 经常连不上，镜像失败再回官方源
    for base in [MIRROR, HF] {
        if cancel.load(Ordering::Relaxed) {
            return Err("已取消下载".into());
        }
        sink.report(Progress {
            resource: asset.name,
            done: 0,
            total: asset.bytes,
            phase: "connecting",
            source: base.to_string(),
            message: None,
        });
        match fetch_one(&client, base, asset, &dest, sink, &cancel).await {
            Ok(()) => return Ok(()),
            Err(err) => {
                if err == "cancelled" {
                    return Err("已取消下载".into());
                }
                errors.push(format!("{base}: {err}"));
            }
        }
    }
    let message = errors.join("; ");
    sink.report(Progress {
        resource: asset.name,
        done: 0,
        total: asset.bytes,
        phase: "error",
        source: String::new(),
        message: Some(message.clone()),
    });
    Err(format!("下载 {} 失败（{message}）", asset.name))
}

/// 按 MODEL → TOKENS → VAD 的顺序补齐模型文件。
pub async fn download_all(
    dest_dir: &Path,
    cancel: Arc<AtomicBool>,
    sink: &dyn ProgressSink,
) -> Result<(), String> {
    for asset in ASSETS {
        download_asset(asset, dest_dir, cancel.clone(), sink).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
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
    fn assets_are_consistent() {
        for asset in ASSETS {
            assert!(asset.bytes > 0);
            assert_eq!(asset.sha256.len(), 64);
            assert!(asset.path.starts_with('/'));
        }
    }
}
