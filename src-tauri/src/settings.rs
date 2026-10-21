//! 应用设置：语言、ITN、输出目录、模型目录，持久化到应用配置目录。

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

pub const LANGUAGES: [&str; 6] = ["auto", "zh", "en", "yue", "ja", "ko"];
pub const MODES: [&str; 2] = ["offline", "streaming"];

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
    pub output_dir: Option<String>,
    pub model_dir: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            language: "auto".into(),
            mode: "offline".into(),
            itn: true,
            punct: false,
            output_dir: None,
            model_dir: None,
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
    let path = config_path(app).ok_or("无法定位应用配置目录")?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("无法创建设置目录: {e}"))?;
    }
    let text = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    fs::write(&path, text).map_err(|e| format!("保存设置失败: {e}"))
}

/// 默认输出目录：文档/语音转文字。
pub fn default_output_dir(app: &AppHandle) -> PathBuf {
    app.path()
        .document_dir()
        .map(|dir| dir.join("语音转文字"))
        .unwrap_or_else(|_| PathBuf::from(".").join("语音转文字"))
}
