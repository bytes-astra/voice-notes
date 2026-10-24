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
    /// 使用的输入设备名（None = 系统默认）
    pub input_device: Option<String>,
    /// 流式模式的热词（术语表）：只在 streaming 模式生效，空 = 不启用
    pub hotwords: Vec<String>,
    pub output_dir: Option<String>,
    pub model_dir: Option<String>,
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
    let text = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
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

#[cfg(test)]
mod hotword_tests {
    use super::*;

    #[test]
    fn cleans_hotwords() {
        let cleaned = clean_hotwords(vec![
            " 产品评审会 ".into(),
            "".into(),
            "   ".into(),
            "产品评审会".into(),
            "玄戒芯片".into(),
            "x".repeat(MAX_HOTWORD_CHARS + 1),
        ]);
        assert_eq!(cleaned, vec!["产品评审会", "玄戒芯片"]);
    }

    #[test]
    fn caps_hotword_count() {
        let many: Vec<String> = (0..MAX_HOTWORDS + 20).map(|i| format!("词{i}")).collect();
        assert_eq!(clean_hotwords(many).len(), MAX_HOTWORDS);
    }

    #[test]
    fn settings_round_trip() {
        let raw = r#"{"language":"zh","itn":false,"output_dir":"/tmp/out","model_dir":null}"#;
        let settings: Settings = serde_json::from_str(raw).unwrap();
        assert_eq!(settings.language, "zh");
        assert!(!settings.itn);
        let normalized = Settings {
            language: "klingon".into(),
            ..settings
        }
        .normalize();
        assert_eq!(normalized.language, "auto");
    }
}
