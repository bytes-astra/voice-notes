//! 应用设置：语言、ITN、输出目录、模型目录，持久化到应用配置目录。

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::{code, code_detail};

pub const LANGUAGES: [&str; 6] = ["auto", "zh", "en", "yue", "ja", "ko"];
/// 术语表上限：条数太多会明显增加热词误插与解码耗时。
pub const MAX_HOTWORDS: usize = 100;
pub const MAX_HOTWORD_CHARS: usize = 32;
pub const MODES: [&str; 3] = ["offline", "streaming", "cloud"];
/// 云端 provider 取值（`cloud_provider`）。默认 `realtime`（Realtime JSON 族）。
/// 另两家：`volcengine`（火山 / 豆包，自研二进制帧）、`aliyun`（阿里云百炼 Qwen-Audio）。
pub const CLOUD_PROVIDERS: [&str; 3] = ["realtime", "volcengine", "aliyun"];

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// auto | zh | en | yue | ja | ko
    pub language: String,
    /// offline（SenseVoice 整段）| streaming（Zipformer 实时）
    pub mode: String,
    /// 逆文本归一化（数字、标点）
    pub itn: bool,
    /// 流式结果是否用标点模型补标点
    pub punct: bool,
    /// 使用的输入设备名（None = 系统默认）
    pub input_device: Option<String>,
    /// 热词（术语表）：流式与云端模式使用（整段模式不支持），空 = 不启用
    pub hotwords: Vec<String>,
    pub output_dir: Option<String>,
    pub model_dir: Option<String>,
    /// 云端 base URL（空 = 用 provider 默认端点）
    pub cloud_base_url: String,
    /// 云端 API key（不回传明文、不写日志）
    pub cloud_api_key: String,
    /// 云端模型名（provider 相关）
    pub cloud_model: String,
    /// 云端资源标识（火山的 `X-Api-Resource-Id`；空 = 用该 provider 的默认）。
    pub cloud_resource_id: String,
    /// 云端 provider：`realtime`（Realtime JSON 族）/ `volcengine`（火山二进制帧族）
    pub cloud_provider: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            language: "auto".into(),
            mode: "offline".into(),
            itn: true,
            punct: true,
            input_device: None,
            hotwords: Vec::new(),
            output_dir: None,
            model_dir: None,
            cloud_base_url: String::new(),
            cloud_api_key: String::new(),
            cloud_model: String::new(),
            cloud_resource_id: String::new(),
            cloud_provider: "realtime".into(),
        }
    }
}

impl Settings {
    pub fn normalize(mut self) -> Self {
        if !LANGUAGES.contains(&self.language.as_str()) {
            self.language = "auto".into();
        }
        if !MODES.contains(&self.mode.as_str()) {
            self.mode = "offline".into();
        }
        if !CLOUD_PROVIDERS.contains(&self.cloud_provider.as_str()) {
            self.cloud_provider = "realtime".into();
        }
        self.hotwords = clean_hotwords(self.hotwords);
        self
    }
}

fn config_path(app: &AppHandle) -> Option<PathBuf> {
    app.path()
        .app_config_dir()
        .ok()
        .map(|dir| dir.join("settings.json"))
}

pub fn load(app: &AppHandle) -> Settings {
    let Some(path) = config_path(app) else {
        return Settings::default();
    };
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Settings>(&text).ok())
        .unwrap_or_default()
        .normalize()
}

pub fn save(app: &AppHandle, settings: &Settings) -> Result<(), String> {
    let path = config_path(app).ok_or_else(|| code("error.appConfigDir"))?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| code_detail("error.createSettingsDir", e))?;
    }
    let text = serde_json::to_string_pretty(settings)
        .map_err(|e| code_detail("error.saveSettingsFailed", e))?;
    fs::write(&path, text).map_err(|e| code_detail("error.saveSettingsFailed", e))
}

/// 默认输出目录：文档/语音转文字。
pub fn default_output_dir(app: &AppHandle) -> PathBuf {
    // 目录名用 ASCII：中文目录名在终端、脚本、跨平台工具里处处要转义。
    app.path()
        .document_dir()
        .map(|dir| dir.join("VoiceNotes"))
        .unwrap_or_else(|_| PathBuf::from(".").join("VoiceNotes"))
}

/// 校验输出目录：能建出来才算数。
///
/// 返回清洗后的目录（`None` = 用默认目录）。建不出来就返回 Err。
///
/// 调用方必须**先校验再改设置**：先写进设置再校验的话，失败时内存里的设置
/// 已经指向一个用不了的目录（还没落盘，界面显示的还是旧值），
/// 真正的报错会推迟到「保存转写」时才出现。
pub fn prepare_output_dir(dir: Option<String>) -> Result<Option<String>, String> {
    let Some(dir) = dir.filter(|d| !d.trim().is_empty()) else {
        return Ok(None);
    };
    fs::create_dir_all(&dir)
        .map_err(|e| code_detail("error.createDirFailed", format!("{dir}: {e}")))?;
    Ok(Some(dir))
}

/// 一次性迁移：旧版本的默认输出目录是中文的「语音转文字」，现在改成 ASCII 的
/// `VoiceNotes`（中文目录名在终端、脚本、跨平台工具里处处要转义）。
///
/// 只在旧目录还在、新的不存在时才搬 —— 不覆盖、不删除任何已有数据。
pub fn migrate_output_dir(app: &AppHandle) {
    let Some(dir) = app.path().document_dir().ok() else {
        return;
    };
    let old = dir.join("语音转文字");
    let new = dir.join("VoiceNotes");
    if old.is_dir() && !new.exists() {
        if let Err(err) = std::fs::rename(&old, &new) {
            crate::log::warn(format!("迁移输出目录失败：{err}"));
        } else {
            crate::log::info(format!("输出目录已迁移到 {}", new.display()));
        }
    }
}

/// 清洗术语表：去空白/空行/重复，单条 ≤ 32 字，最多 100 条。
///
/// 上限是实测出来的：热词加分会让模型把没说的词硬塞进结果，词表越长越容易误插。
pub fn clean_hotwords(hotwords: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for word in hotwords {
        let word = word.trim();
        if word.is_empty() || word.chars().count() > MAX_HOTWORD_CHARS {
            continue;
        }
        if out.iter().any(|existing| existing == word) {
            continue;
        }
        out.push(word.to_string());
        if out.len() >= MAX_HOTWORDS {
            break;
        }
    }
    out
}

/// 写入云端配置（服务地址 / 凭据 / 模型名）。
///
/// 凭据的处理只有一条规矩：`api_key` 为 `None` 表示「不改」。
/// 前端拿不到当前的值（凭据不回传），所以「清空」必须显式传空串 ——
/// 把空串当成「不改」的话，用户就没法删掉已经存进去的凭据。
pub fn apply_cloud_config(
    settings: &mut Settings,
    base_url: String,
    api_key: Option<String>,
    model: String,
    provider: Option<String>,
    resource_id: Option<String>,
) {
    settings.cloud_base_url = base_url.trim().to_string();
    settings.cloud_model = model.trim().to_string();
    if let Some(key) = api_key {
        settings.cloud_api_key = key.trim().to_string();
    }
    // provider 只接受已知值：非法值回退默认（normalize 也会做一次）。
    if let Some(provider) = provider {
        let provider = provider.trim();
        if CLOUD_PROVIDERS.contains(&provider) {
            settings.cloud_provider = provider.to_string();
        }
    }
    if let Some(resource_id) = resource_id {
        settings.cloud_resource_id = resource_id.trim().to_string();
    }
}

#[cfg(test)]
mod hotword_tests;
#[cfg(test)]
mod cloud_tests;
