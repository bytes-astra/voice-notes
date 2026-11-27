//! 应用日志：**一直开着，不需要用户配置**。
//!
//! - 写到系统标准日志目录（Tauri 的 `app_log_dir`）：
//!   macOS `~/Library/Logs/com.github.bytes-astra.voicenotes/voice-notes.log`，
//!   Windows `%LOCALAPPDATA%\com.github.bytes-astra.voicenotes\logs\voice-notes.log`
//! - 超过 1 MB **或跨天**时轮转一次，历史保留 5 份
//!   （`voice-notes.log.2026-11-26-14-03-22` —— 文件名带时间戳，
//!   用户反馈问题时能直接说「11 月 24 日那次」并把对应文件发过来）
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
use std::time::SystemTime;

use chrono::{DateTime, Local, TimeZone};
use tauri::{AppHandle, Manager};

/// 日志文件名。
const FILE_NAME: &str = "voice-notes.log";
/// 超过这个大小就轮转一次。
const MAX_BYTES: u64 = 1024 * 1024;
/// 历史日志保留几份（不含当前文件）。
const KEEP_ROTATED: usize = 5;
/// 轮转后的历史文件名前缀（当前文件名没有这个尾部的点，不会被当成历史文件）。
const ROTATED_PREFIX: &str = "voice-notes.log.";
/// 历史文件名里的时间戳格式。
const ROTATED_STAMP: &str = "%Y-%m-%d-%H-%M-%S";
/// 时间戳在文件名里的长度（`2026-11-26-14-03-22`）。
const ROTATED_STAMP_LEN: usize = 19;

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
    rotate(&file, Local::now());
    if let Ok(mut handle) = OpenOptions::new().create(true).append(true).open(&file) {
        let _ = writeln!(handle, "{line}");
    }
}

/// 该不该轮转：超限、或跨天。
///
/// `modified` 是当前日志文件最后一次写入的时间 —— 拿不到就只看超限
/// （拿不到 mtime 不等于跨天，不能因此误轮转）。
fn should_rotate(size: u64, modified: Option<SystemTime>, now: DateTime<Local>) -> bool {
    if size >= MAX_BYTES {
        return true;
    }
    let Some(modified) = modified else {
        return false;
    };
    let modified: DateTime<Local> = modified.into();
    modified.date_naive() != now.date_naive()
}

/// 轮转后的历史文件名：`voice-notes.log.2026-11-26-14-03-22`。
fn rotated_name(stamp: DateTime<Local>) -> String {
    format!("{ROTATED_PREFIX}{}", stamp.format(ROTATED_STAMP))
}

/// 从历史文件名里取时间戳。取不出来（旧格式 `voice-notes.log.1`、或后缀不是
/// 计数器）→ `None` —— 排序时**当作最旧**，升级前的旧文件靠这条自然被淘汰。
fn rotated_key(name: &str) -> Option<DateTime<Local>> {
    let rest = name.strip_prefix(ROTATED_PREFIX)?;
    if rest.len() < ROTATED_STAMP_LEN {
        return None;
    }
    let (head, tail) = rest.split_at(ROTATED_STAMP_LEN);
    if !tail.is_empty() && !tail.starts_with('-') {
        return None;
    }
    let naive = chrono::NaiveDateTime::parse_from_str(head, ROTATED_STAMP).ok()?;
    Local.from_local_datetime(&naive).earliest()
}

/// 目录里的历史日志文件，按「最旧在前」排序。
///
/// 排序键是文件名里的时间戳（`None` 最小 = 最旧）；同键按文件名排，保证顺序稳定。
fn rotated_files(dir: &Path) -> Vec<(Option<DateTime<Local>>, String, PathBuf)> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        // 当前文件 `voice-notes.log` 没有尾部的点，不会被当成历史文件。
        if !name.starts_with(ROTATED_PREFIX) {
            continue;
        }
        let key = rotated_key(&name);
        files.push((key, name, entry.path()));
    }
    files.sort();
    files
}

/// 删掉超出保留份数的历史文件（从最旧开始）。删不掉不报错。
fn prune_rotated(dir: &Path) {
    let files = rotated_files(dir);
    let excess = files.len().saturating_sub(KEEP_ROTATED);
    for (_, _, path) in files.into_iter().take(excess) {
        let _ = fs::remove_file(path);
    }
}

/// 轮转：当前文件改名成带时间戳的历史文件，然后清理超额的旧文件。
fn rotate(file: &Path, now: DateTime<Local>) {
    let Ok(meta) = fs::metadata(file) else { return };
    if !should_rotate(meta.len(), meta.modified().ok(), now) {
        return;
    }
    let Some(dir) = file.parent() else { return };
    let target = unique_rotated_path(dir, now);
    if fs::rename(file, &target).is_err() {
        return;
    }
    prune_rotated(dir);
}

/// 轮转的目标路径：默认是时间戳名；**已存在就加计数器**，不覆盖既有文件。
fn unique_rotated_path(dir: &Path, now: DateTime<Local>) -> PathBuf {
    let base = dir.join(rotated_name(now));
    if !base.exists() {
        return base;
    }
    for n in 1..=100 {
        let candidate = dir.join(format!("{ROTATED_PREFIX}{}-{n}", now.format(ROTATED_STAMP)));
        if !candidate.exists() {
            return candidate;
        }
    }
    // 同一秒轮转 100 次以上 —— 拿纳秒兜底。
    dir.join(format!(
        "{ROTATED_PREFIX}{}-{}",
        now.format(ROTATED_STAMP),
        now.timestamp_subsec_nanos()
    ))
}

/// 本地时间戳（用户看日志时不用自己换算时区）。
fn timestamp() -> String {
    chrono::Local::now()
        .format("%Y-%m-%d %H:%M:%S%.3f")
        .to_string()
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
mod tests;
