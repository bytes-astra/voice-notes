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

const HF: &str = "https://huggingface.co";
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

fn candidate_dirs(app: &AppHandle, settings_dir: Option<&str>) -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    if let Some(dir) = settings_dir {
        out.push((PathBuf::from(dir), "自定义目录".to_string()));
    }
    if let Ok(data) = app.path().app_data_dir() {
        out.push((data.join("models"), "应用数据".to_string()));
    }
    if let Some(dsh) = dsh_model_dir() {
        out.push((dsh, "DSH 语音模型".to_string()));
    }
    out
}

/// DSH 已下载的 SenseVoice 模型目录（若存在）。
pub fn dsh_model_dir() -> Option<PathBuf> {
    let dir = dirs::home_dir()?.join(".dsh/speech-to-text/sensevoice/models/sensevoice-onnx");
    if exists(&dir.join(MODEL_FILE)) && exists(&dir.join(TOKENS_FILE)) {
        Some(dir)
    } else if exists(&dir.join("model.onnx")) && exists(&dir.join(TOKENS_FILE)) {
        Some(dir)
    } else {
        None
    }
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

/// 按「自定义目录 > 应用数据 > DSH 目录」的顺序找出可用模型。
pub fn resolve(app: &AppHandle, settings_dir: Option<&str>) -> Option<ModelPaths> {
    for (dir, source) in candidate_dirs(app, settings_dir) {
        let Some(model) = [dir.join(MODEL_FILE), dir.join("model.onnx")]
            .into_iter()
            .find(|p| exists(p))
        else {
            continue;
        };
        let tokens = dir.join(TOKENS_FILE);
        if !exists(&tokens) {
            continue;
        }
        return Some(ModelPaths {
            dir: dir.to_string_lossy().into_owned(),
            model: model.to_string_lossy().into_owned(),
            tokens: tokens.to_string_lossy().into_owned(),
            vad: find_vad(&dir).map(|p| p.to_string_lossy().into_owned()),
            source,
        });
    }
    None
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
    let digest = hash_file(&part)?;
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
    for base in [HF, MIRROR] {
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
