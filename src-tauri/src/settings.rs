//! 应用设置：语言、ITN、输出目录、模型目录，持久化到应用配置目录。

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::error_code::{code, code_detail, ErrorCode};

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
    /// 云端连接列表：每条是一**整套**配置（provider / 地址 / 凭据 / 模型 / 备注）。
    ///
    /// **不做旧格式迁移**：旧版本把云端配置平铺成 5 个字段，改成列表后那 5 个
    /// 字段被忽略（用户重新填一遍）。两套真值来源迟早不一致 —— 不留。
    #[serde(default)]
    pub cloud_profiles: Vec<CloudProfile>,
    /// 正在使用的那条连接（`cloud_profiles` 里的 `id`）。
    #[serde(default)]
    pub cloud_active_profile: String,
}

/// 一条云端连接：一整套配置。
///
/// **为什么一条 = 一整套**：地址、密钥、模型 / 资源标识是**同一家**的 —— 换
/// provider 换的是整套，不是单个字段（拆开会让用户在两条半配置之间来回切）。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CloudProfile {
    /// 稳定 id（`p-1` 那种）：删除某条后活动 id 仍要能对上。
    pub id: String,
    /// 用户自己起的名字（区分「工作 / 个人」「火山·小时版」）。
    pub label: String,
    /// provider：`realtime` / `volcengine` / `aliyun`。
    pub provider: String,
    /// base URL（空 = 用该 provider 的默认端点）。
    pub base_url: String,
    /// API key（明文，只存 settings；**不回传前端、不写日志**）。
    pub api_key: String,
    /// 模型 / 资源标识 —— **按 provider 解释**：
    /// Realtime 与阿里是模型名，火山是 `X-Api-Resource-Id`。
    pub model: String,
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
            cloud_profiles: Vec::new(),
            cloud_active_profile: String::new(),
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
        self.clean_cloud_profiles();
        self
    }

    /// 清洗连接列表：provider 取值合法、id 不重复、活动项指向真实存在的连接。
    fn clean_cloud_profiles(&mut self) {
        // 清洗**前**的 id：用来判断「这个 id 是不是被别人也用了」。
        let originals: Vec<String> = self
            .cloud_profiles
            .iter()
            .map(|profile| profile.id.trim().to_string())
            .filter(|id| !id.is_empty())
            .collect();
        // 已经定下来的 id（前面几条清洗完的结果）。
        let mut taken: Vec<String> = Vec::new();
        for profile in self.cloud_profiles.iter_mut() {
            if !CLOUD_PROVIDERS.contains(&profile.provider.as_str()) {
                profile.provider = "realtime".into();
            }
            profile.id = profile.id.trim().to_string();
            profile.label = profile.label.trim().to_string();
            profile.base_url = profile.base_url.trim().to_string();
            profile.model = profile.model.trim().to_string();
            // id 为空、跟前面那条重了、或**原始列表里本来就有两条同名** → 换一个新 id。
            //
            // 新 id 的候选集 = 「已经定下来的」∪「原始列表里的全部」——
            // 原始列表也要避开：不然这条抢走的 id 可能是**后面**某条本来要留着的
            // （两条空 id 曾因此拿到同一个 id → 按 id 匹配的操作全部串台）。
            let unique_original = originals.iter().filter(|id| **id == profile.id).count() == 1;
            if profile.id.is_empty() || taken.contains(&profile.id) || !unique_original {
                let mut avoid = taken.clone();
                avoid.extend(originals.iter().cloned());
                profile.id = next_profile_id(&avoid);
            }
            taken.push(profile.id.clone());
        }
        // 活动项必须指向真实存在的连接；否则落到第一条；一条都没有就清空。
        self.cloud_active_profile = self.cloud_active_profile.trim().to_string();
        if !self
            .cloud_profiles
            .iter()
            .any(|profile| profile.id == self.cloud_active_profile)
        {
            self.cloud_active_profile = self
                .cloud_profiles
                .first()
                .map(|profile| profile.id.clone())
                .unwrap_or_default();
        }
    }

    /// 正在使用的那条连接（没有就 `None`）。
    pub fn active_cloud_profile(&self) -> Option<&CloudProfile> {
        self.cloud_profiles
            .iter()
            .find(|profile| profile.id == self.cloud_active_profile)
    }
}

/// 新的连接 id（`p-1`、`p-2`…）—— 避开已有的。
fn next_profile_id(existing: &[String]) -> String {
    let mut index = existing.len() + 1;
    loop {
        let candidate = format!("p-{index}");
        if !existing.contains(&candidate) {
            return candidate;
        }
        index += 1;
    }
}

/// provider 的默认显示名（迁移时当连接的默认备注）。
fn provider_label(provider: &str) -> String {
    match provider {
        "volcengine" => "火山引擎 · 豆包".into(),
        "aliyun" => "阿里云百炼 · Qwen-Audio".into(),
        _ => "OpenAI Realtime".into(),
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
    let path = config_path(app).ok_or_else(|| code(ErrorCode::ErrorAppConfigDir))?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| code_detail(ErrorCode::ErrorCreateSettingsDir, e))?;
    }
    let text = serde_json::to_string_pretty(settings)
        .map_err(|e| code_detail(ErrorCode::ErrorSaveSettingsFailed, e))?;
    fs::write(&path, text).map_err(|e| code_detail(ErrorCode::ErrorSaveSettingsFailed, e))
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
        .map_err(|e| code_detail(ErrorCode::ErrorCreateDirFailed, format!("{dir}: {e}")))?;
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

/// 保存（新增或改）一条云端连接，返回它的 id。
///
/// 凭据的处理只有一条规矩：`api_key` 为 `None` 表示「不改」。
/// 前端拿不到当前的值（凭据不回传），所以「清空」必须显式传空串 ——
/// 把空串当成「不改」的话，用户就没法删掉已经存进去的凭据。
pub fn upsert_cloud_profile(
    settings: &mut Settings,
    id: Option<String>,
    label: String,
    provider: String,
    base_url: String,
    api_key: Option<String>,
    model: String,
) -> String {
    let provider = provider.trim().to_string();
    let provider = if CLOUD_PROVIDERS.contains(&provider.as_str()) {
        provider
    } else {
        "realtime".to_string()
    };
    let model = model.trim().to_string();
    let base_url = base_url.trim().to_string();
    let label = label.trim().to_string();

    // 已有的连接 → 改它；没有 → 新建一条。
    let id = match id
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        Some(id)
            if settings
                .cloud_profiles
                .iter()
                .any(|profile| profile.id == id) =>
        {
            id
        }
        _ => {
            let id = next_profile_id(
                &settings
                    .cloud_profiles
                    .iter()
                    .map(|profile| profile.id.clone())
                    .collect::<Vec<_>>(),
            );
            settings.cloud_profiles.push(CloudProfile {
                id: id.clone(),
                label: label.clone(),
                provider: provider.clone(),
                base_url: base_url.clone(),
                api_key: String::new(),
                model: model.clone(),
            });
            id
        }
    };
    // 编辑已有连接时先记下它原来的 provider：换了 provider 就是换了一家服务，
    // 旧凭据不能留在新服务名下（RC-K21）。兜底放在后端 —— 前端绕不过去。
    let previous_provider = settings
        .cloud_profiles
        .iter()
        .find(|profile| profile.id == id)
        .map(|profile| profile.provider.clone());
    if let Some(profile) = settings
        .cloud_profiles
        .iter_mut()
        .find(|profile| profile.id == id)
    {
        let provider_changed = previous_provider
            .as_deref()
            .is_some_and(|old| old != provider);
        profile.provider = provider;
        // 备注留空就退回 provider 的默认名 —— 列表里总要有个能认出来的名字。
        // 用**新** provider 的名字：先取旧名字再改 provider，列表里会出现
        // 「provider 是阿里、名字却写着 OpenAI Realtime」这种对不上的连接。
        profile.label = if label.is_empty() {
            provider_label(&profile.provider)
        } else {
            label
        };
        profile.base_url = base_url;
        profile.model = model;
        match api_key {
            Some(key) => profile.api_key = key.trim().to_string(),
            // 换了 provider 而调用方没给新凭据 → 旧凭据作废：
            // 一家的 key 不能发给另一家的端点（`base_url` 还可自定义）。
            None if provider_changed => profile.api_key.clear(),
            None => {}
        }
    }
    // 列表里第一条就是活动项（新增时自动选中它）。
    if settings.cloud_active_profile.trim().is_empty() {
        settings.cloud_active_profile = id.clone();
    }
    settings.clean_cloud_profiles();
    id
}

/// 删一条连接。删的是活动项 → 落到**下一条**（删的是最后一条则落到前一条）；
/// 删空了 → 回到「未配置」。
pub fn delete_cloud_profile(settings: &mut Settings, id: &str) -> Result<(), String> {
    let id = id.trim();
    let Some(index) = settings
        .cloud_profiles
        .iter()
        .position(|profile| profile.id == id)
    else {
        return Err(crate::code(ErrorCode::CloudProfileNotFound));
    };
    let was_active = settings.cloud_active_profile.trim() == id;
    settings.cloud_profiles.retain(|profile| profile.id != id);
    if was_active {
        // 删掉下标 index 后，同一下标就是原来的「下一条」；删的是最后一条
        // 时没有下一条 → 用前一条。一条都不剩 → 清空。
        settings.cloud_active_profile = settings
            .cloud_profiles
            .get(index)
            .or_else(|| settings.cloud_profiles.get(index.saturating_sub(1)))
            .map(|profile| profile.id.clone())
            .unwrap_or_default();
    }
    settings.clean_cloud_profiles();
    Ok(())
}

/// 清掉一条连接的凭据（**其它字段一个字都不动**）。
///
/// 凭据明文不回传前端 → 界面没法「读出来再改」，只能显式清。
/// 前后端各留一条路：保存时传空串是「清空」，这条命令是「只清凭据」
/// （不连带提交编辑区里其它还没保存的字段）。
pub fn clear_cloud_profile_key(settings: &mut Settings, id: &str) -> Result<(), String> {
    let id = id.trim();
    let Some(profile) = settings
        .cloud_profiles
        .iter_mut()
        .find(|profile| profile.id == id)
    else {
        return Err(crate::code(ErrorCode::CloudProfileNotFound));
    };
    profile.api_key = String::new();
    Ok(())
}

/// 指定正在使用的那条连接（id 不存在就不动）。
pub fn set_active_cloud_profile(settings: &mut Settings, id: &str) {
    let id = id.trim();
    if settings
        .cloud_profiles
        .iter()
        .any(|profile| profile.id == id)
    {
        settings.cloud_active_profile = id.to_string();
    }
}

/// 凭据的掩码：按**露出比例**分档，露出的字符数永远 ≤ 原文长度的一半。
///
/// **凭据明文不回传前端**（RC-K6）—— 掩码是唯一回传的形态。
///
/// 规则：头尾各露 `min(4, 长度 / 4)` 个字符。
/// - 官方 key 都 20+ 字符：仍是头 4 + 尾 4（露 8/20 = 40%），读起来最省事；
/// - 短凭据按比例缩小，不会在「9 个字符」处从露 2 跳到露 8；
/// - 1~3 个字符露任何一个都超过一半 → 一律用固定点数。
pub fn mask_secret(secret: &str) -> String {
    let chars: Vec<char> = secret.chars().collect();
    let length = chars.len();
    if length == 0 {
        return String::new();
    }
    let shown = (length / 4).min(4);
    if shown == 0 {
        return "••".to_string();
    }
    let head: String = chars[..shown].iter().collect();
    let tail: String = chars[length - shown..].iter().collect();
    format!("{head}…{tail}")
}

#[cfg(test)]
mod cloud_tests;
#[cfg(test)]
mod hotword_tests;
