//! 应用日志：**一直开着，不需要用户配置**。
//!
//! - 写到系统标准日志目录（Tauri 的 `app_log_dir`）：
//!   macOS `~/Library/Logs/com.github.bytes-astra.voicenotes/voice-notes.log`，
//!   Windows `%LOCALAPPDATA%\com.github.bytes-astra.voicenotes\logs\voice-notes.log`
//! - 超过 1 MB 轮转一次，保留 `voice-notes.log.1`
//! - 同时镜像到 stderr：开发时终端直接看得到，macOS 上进系统日志
//! - **界面上的文案不放技术细节，细节都在这里**（见 AGENTS.md）
//!
//! 用法：`log::info("模型已加载")` / `log::error("下载失败", e)`。
//! 任何线程都能调（内部有锁），不需要 AppHandle。

use std::fmt::Display;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use tauri::{AppHandle, Manager};

/// 日志文件名。
const FILE_NAME: &str = "voice-notes.log";
/// 超过这个大小就轮转一次。
const MAX_BYTES: u64 = 1024 * 1024;

/// 日志目录（应用启动时定下来）。
static DIR: OnceLock<PathBuf> = OnceLock::new();
/// 写日志的锁：多个线程（识别线程、下载线程）会同时写。
static LOCK: Mutex<()> = Mutex::new(());

/// 启动时记住日志目录，并建出来。
pub fn init(app: &AppHandle) {
    let dir = match app.path().app_log_dir() {
        Ok(dir) => dir,
        Err(err) => {
            eprintln!("[VoiceNotes] 取不到日志目录：{err}");
            return;
        }
    };
    if let Err(err) = fs::create_dir_all(&dir) {
        eprintln!("[VoiceNotes] 建不了日志目录 {}：{err}", dir.display());
        return;
    }
    if DIR.set(dir).is_err() {
        // 只初始化一次：重复调用说明有第二次 init，忽略。
        return;
    }
    let path = path();
    info(format!("日志文件：{path}"));
}

/// 日志文件的完整路径（设置页/支持时告诉用户用哪个文件）。
pub fn path() -> String {
    match DIR.get() {
        Some(dir) => dir.join(FILE_NAME).to_string_lossy().into_owned(),
        None => String::new(),
    }
}

pub fn info(message: impl Display) {
    write("INFO", message)
}

pub fn warn(message: impl Display) {
    write("WARN", message)
}

pub fn error(message: impl Display) {
    write("ERROR", message)
}

/// 写一行：`2026-10-23 22:31:52.123 [INFO] 文本`
pub fn write(level: &str, message: impl Display) {
    let line = format!("{} [{}] {}", timestamp(), level, message);
    // stderr：开发时终端可见，macOS 上进系统日志。
    eprintln!("[VoiceNotes] {line}");
    let Some(dir) = DIR.get() else { return };
    let _guard = LOCK.lock();
    write_line(dir, &line);
}

/// 往目录里的日志文件追加一行（先轮转）。拆出来是为了能单测。
fn write_line(dir: &Path, line: &str) {
    let file = dir.join(FILE_NAME);
    rotate(&file);
    if let Ok(mut handle) = OpenOptions::new().create(true).append(true).open(&file) {
        let _ = writeln!(handle, "{line}");
    }
}

/// 超过上限就轮转：当前文件改名成 .1，旧的 .1 删掉。
fn rotate(file: &Path) {
    let Ok(meta) = fs::metadata(file) else { return };
    if meta.len() < MAX_BYTES {
        return;
    }
    let previous = file.with_extension("log.1");
    let _ = fs::remove_file(&previous);
    let _ = fs::rename(file, &previous);
}

/// 本地时间戳（用户看日志时不用自己换算时区）。
fn timestamp() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f").to_string()
}

/// 布尔开关在日志里写「开 / 关」。
pub fn on_off(value: bool) -> &'static str {
    if value {
        "开"
    } else {
        "关"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 日志文件真的写出来了、超限会轮转 —— 不依赖 AppHandle，临时目录里跑。
    #[test]
    fn writes_lines_and_rotates() {
        let dir = std::env::temp_dir().join(format!("voice-notes-log-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("建临时目录");

        write_line(&dir, "2026-01-01 00:00:00.000 [INFO] 第一行");
        write_line(&dir, "2026-01-01 00:00:01.000 [INFO] 第二行");
        let body = fs::read_to_string(dir.join(FILE_NAME)).expect("读日志");
        assert!(body.contains("第一行"), "第一行没写进去：{body}");
        assert!(body.contains("第二行"), "第二行没写进去：{body}");

        // 写爆上限 → 轮转：旧内容进 .1，新内容进当前文件。
        fs::write(dir.join(FILE_NAME), vec![b'x'; MAX_BYTES as usize + 16]).expect("写大文件");
        write_line(&dir, "2026-01-01 00:00:02.000 [INFO] 轮转之后");
        assert!(dir.join("voice-notes.log.1").exists(), "没有生成轮转文件");
        let current = fs::read_to_string(dir.join(FILE_NAME)).expect("读当前日志");
        assert!(current.contains("轮转之后"), "轮转后新行没写进去：{current}");
        assert!(!current.contains('x'), "轮转后当前文件还是旧内容");

        let _ = fs::remove_dir_all(&dir);
    }
}
