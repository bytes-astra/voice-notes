//! 错误码：后端只返回字典 key（前端 i18n 翻译），不返回展示文案。
//!
//! **枚举，不是字符串**：`code("record.empty")` 拼错 key 编译能过，运行时才发现
//! 界面上显示「未知错误」（`check:i18n` 查得了「key 在不在字典里」，查不了
//! **比较逻辑用错了 key** —— 比如 `is_stop_failed` 拼成 `record.stopFaild`，
//! 守卫不红，真机上超时文案换不上）。
//!
//! 变体 → key 的映射写在 [`ErrorCode::key`] 的 match 里（**唯一真值源**）：
//! `Display` 与 [`ErrorCode::from_key`] 都走它，两份表不会漂。
//!
//! 门禁 `check:i18n` 查的就是 [`ALL_ERROR_CODES`] —— 每个变体的 key 必须在字典里。

use crate::log;

/// 后端的错误码。
///
/// **枚举，不是字符串**：`code(ErrorCode::RecordEmpty)` 拼错 key 编译能过，
/// 运行时才发现界面上显示「未知错误」（`check:i18n` 查得了「key 在不在
/// 字典里」，查不了**比较逻辑用错了 key** —— 比如 `is_stop_failed` 拼成
/// `record.stopFaild`，守卫不红，真机上超时文案换不上）。
///
/// 变体 → key 的映射写在 [`ErrorCode::key`] 的 match 里（**唯一真值源**）：
/// `Display` 与 [`ErrorCode::from_key`] 都走它，两份表不会漂。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorCode {
    /// `cloud.authFailed`
    CloudAuthFailed,
    /// `cloud.connectFailed`
    CloudConnectFailed,
    /// `cloud.disconnected`
    CloudDisconnected,
    /// `cloud.noResult`
    CloudNoResult,
    /// `cloud.notConfigured`
    CloudNotConfigured,
    /// `cloud.profileNotFound`
    CloudProfileNotFound,
    /// `cloud.protocolError`
    CloudProtocolError,
    /// `cloud.sendFailed`
    CloudSendFailed,
    /// `cloud.timeout`
    CloudTimeout,
    /// `cloud.unsupportedModel`
    CloudUnsupportedModel,
    /// `cloud.unsupportedProvider`
    CloudUnsupportedProvider,
    /// `cloud.vadInvalid`
    CloudVadInvalid,
    /// `cloud.vadMissing`
    CloudVadMissing,
    /// `decode.failed`
    DecodeFailed,
    /// `decode.noAudioTrack`
    DecodeNoAudioTrack,
    /// `decode.openFailed`
    DecodeOpenFailed,
    /// `decode.unknownFormat`
    DecodeUnknownFormat,
    /// `error.appConfigDir`
    ErrorAppConfigDir,
    /// `error.appDataDirFailed`
    ErrorAppDataDirFailed,
    /// `error.cloudState`
    ErrorCloudState,
    /// `error.cloudThreadFailed`
    ErrorCloudThreadFailed,
    /// `error.connectFailed`
    ErrorConnectFailed,
    /// `error.createDirFailed`
    ErrorCreateDirFailed,
    /// `error.createModelDir`
    ErrorCreateModelDir,
    /// `error.createOutputDir`
    ErrorCreateOutputDir,
    /// `error.createPunctDir`
    ErrorCreatePunctDir,
    /// `error.createSettingsDir`
    ErrorCreateSettingsDir,
    /// `error.createStreamingDir`
    ErrorCreateStreamingDir,
    /// `error.downloadFailed`
    ErrorDownloadFailed,
    /// `error.downloadInterrupted`
    ErrorDownloadInterrupted,
    /// `error.engineState` —— 引擎槽的锁被 poison。
    ErrorEngineState,
    /// `error.menuFailed`
    ErrorMenuFailed,
    /// `error.offlineThreadFailed`
    ErrorOfflineThreadFailed,
    /// `error.punctEngineState`
    ErrorPunctEngineState,
    /// `error.readFileFailed`
    ErrorReadFileFailed,
    /// `error.recordState`
    ErrorRecordState,
    /// `error.recordThreadFailed`
    ErrorRecordThreadFailed,
    /// `error.saveSettingsFailed`
    ErrorSaveSettingsFailed,
    /// `error.settingsState`
    ErrorSettingsState,
    /// `error.shaMismatch`
    ErrorShaMismatch,
    /// `error.sizeMismatch`
    ErrorSizeMismatch,
    /// `error.streamState`
    ErrorStreamState,
    /// `error.streamEngineState` —— 流式引擎槽的锁被 poison。
    ErrorStreamEngineState,
    /// `error.streamThreadFailed`
    ErrorStreamThreadFailed,
    /// `error.unsupportedLanguage`
    ErrorUnsupportedLanguage,
    /// `error.unsupportedMode`
    ErrorUnsupportedMode,
    /// `error.verifyFailed`
    ErrorVerifyFailed,
    /// `error.writeCacheFailed`
    ErrorWriteCacheFailed,
    /// `error.writeFailed`
    ErrorWriteFailed,
    /// `error.writeFileFailed`
    ErrorWriteFileFailed,
    /// `model.loadFailed`
    ModelLoadFailed,
    /// `model.needOffline`
    ModelNeedOffline,
    /// `model.needStreaming`
    ModelNeedStreaming,
    /// `model.punctLoadFailed`
    ModelPunctLoadFailed,
    /// `model.punctNotReady`
    ModelPunctNotReady,
    /// `model.streamLoadFailed`
    ModelStreamLoadFailed,
    /// `model.vadMissing`
    ModelVadMissing,
    /// `record.alreadyRecording`
    RecordAlreadyRecording,
    /// `record.cloudTimeout`
    RecordCloudTimeout,
    /// `record.configLocked`
    RecordConfigLocked,
    /// `record.deviceNotFound`
    RecordDeviceNotFound,
    /// `record.empty`
    RecordEmpty,
    /// `record.micConfigFailed`
    RecordMicConfigFailed,
    /// `record.micError`
    RecordMicError,
    /// `record.micFormatUnsupported` —— 麦克风的采样格式不受支持。
    RecordMicFormatUnsupported,
    /// `record.micOpenFailed`
    RecordMicOpenFailed,
    /// `record.micTimeout`
    RecordMicTimeout,
    /// `record.micWarmupFailed`
    RecordMicWarmupFailed,
    /// `record.noInputDevice`
    RecordNoInputDevice,
    /// `record.notRecording`
    RecordNotRecording,
    /// `record.offlineTimeout`
    RecordOfflineTimeout,
    /// `record.streamTimeout`
    RecordStreamTimeout,
    /// `record.stopFailed` —— 停止录音时识别线程没交回结果。
    RecordStopFailed,
    /// `download.cancelled` —— **内部哨兵**，不进字典：用户看到的是
    /// `setup.cancelled`（见 models.rs 的 `download_asset`）。
    DownloadCancelled,
    /// `record.tooShort` —— 录音太短，识别不出内容。
    RecordTooShort,
    /// `result.noSave`
    ResultNoSave,
    /// `settings.openDirFailed`
    SettingsOpenDirFailed,
    /// `setup.busy`
    SetupBusy,
    /// `setup.cancelled`
    SetupCancelled,
}

impl ErrorCode {
    /// 这个错误码的字典 key（**唯一真值源**）。
    ///
    /// 显式的 match，不是「变体名小写加前缀」那种隐式规则 —— 隐式规则下
    /// `RecordMicError` 要靠大小写转换得到 `record.micError`，不可靠。
    pub fn key(self) -> &'static str {
        match self {
            ErrorCode::CloudAuthFailed => "cloud.authFailed",
            ErrorCode::CloudConnectFailed => "cloud.connectFailed",
            ErrorCode::CloudDisconnected => "cloud.disconnected",
            ErrorCode::CloudNoResult => "cloud.noResult",
            ErrorCode::CloudNotConfigured => "cloud.notConfigured",
            ErrorCode::CloudProfileNotFound => "cloud.profileNotFound",
            ErrorCode::CloudProtocolError => "cloud.protocolError",
            ErrorCode::CloudSendFailed => "cloud.sendFailed",
            ErrorCode::CloudTimeout => "cloud.timeout",
            ErrorCode::CloudUnsupportedModel => "cloud.unsupportedModel",
            ErrorCode::CloudUnsupportedProvider => "cloud.unsupportedProvider",
            ErrorCode::CloudVadInvalid => "cloud.vadInvalid",
            ErrorCode::CloudVadMissing => "cloud.vadMissing",
            ErrorCode::DecodeFailed => "decode.failed",
            ErrorCode::DecodeNoAudioTrack => "decode.noAudioTrack",
            ErrorCode::DecodeOpenFailed => "decode.openFailed",
            ErrorCode::DecodeUnknownFormat => "decode.unknownFormat",
            ErrorCode::ErrorAppConfigDir => "error.appConfigDir",
            ErrorCode::ErrorAppDataDirFailed => "error.appDataDirFailed",
            ErrorCode::ErrorCloudState => "error.cloudState",
            ErrorCode::ErrorCloudThreadFailed => "error.cloudThreadFailed",
            ErrorCode::ErrorConnectFailed => "error.connectFailed",
            ErrorCode::ErrorCreateDirFailed => "error.createDirFailed",
            ErrorCode::ErrorCreateModelDir => "error.createModelDir",
            ErrorCode::ErrorCreateOutputDir => "error.createOutputDir",
            ErrorCode::ErrorCreatePunctDir => "error.createPunctDir",
            ErrorCode::ErrorCreateSettingsDir => "error.createSettingsDir",
            ErrorCode::ErrorCreateStreamingDir => "error.createStreamingDir",
            ErrorCode::ErrorDownloadFailed => "error.downloadFailed",
            ErrorCode::ErrorDownloadInterrupted => "error.downloadInterrupted",
            ErrorCode::ErrorEngineState => "error.engineState",
            ErrorCode::ErrorMenuFailed => "error.menuFailed",
            ErrorCode::ErrorOfflineThreadFailed => "error.offlineThreadFailed",
            ErrorCode::ErrorPunctEngineState => "error.punctEngineState",
            ErrorCode::ErrorReadFileFailed => "error.readFileFailed",
            ErrorCode::ErrorRecordState => "error.recordState",
            ErrorCode::ErrorRecordThreadFailed => "error.recordThreadFailed",
            ErrorCode::ErrorSaveSettingsFailed => "error.saveSettingsFailed",
            ErrorCode::ErrorSettingsState => "error.settingsState",
            ErrorCode::ErrorShaMismatch => "error.shaMismatch",
            ErrorCode::ErrorSizeMismatch => "error.sizeMismatch",
            ErrorCode::ErrorStreamState => "error.streamState",
            ErrorCode::ErrorStreamEngineState => "error.streamEngineState",
            ErrorCode::ErrorStreamThreadFailed => "error.streamThreadFailed",
            ErrorCode::ErrorUnsupportedLanguage => "error.unsupportedLanguage",
            ErrorCode::ErrorUnsupportedMode => "error.unsupportedMode",
            ErrorCode::ErrorVerifyFailed => "error.verifyFailed",
            ErrorCode::ErrorWriteCacheFailed => "error.writeCacheFailed",
            ErrorCode::ErrorWriteFailed => "error.writeFailed",
            ErrorCode::ErrorWriteFileFailed => "error.writeFileFailed",
            ErrorCode::ModelLoadFailed => "model.loadFailed",
            ErrorCode::ModelNeedOffline => "model.needOffline",
            ErrorCode::ModelNeedStreaming => "model.needStreaming",
            ErrorCode::ModelPunctLoadFailed => "model.punctLoadFailed",
            ErrorCode::ModelPunctNotReady => "model.punctNotReady",
            ErrorCode::ModelStreamLoadFailed => "model.streamLoadFailed",
            ErrorCode::ModelVadMissing => "model.vadMissing",
            ErrorCode::RecordAlreadyRecording => "record.alreadyRecording",
            ErrorCode::RecordCloudTimeout => "record.cloudTimeout",
            ErrorCode::RecordConfigLocked => "record.configLocked",
            ErrorCode::RecordDeviceNotFound => "record.deviceNotFound",
            ErrorCode::RecordEmpty => "record.empty",
            ErrorCode::RecordMicConfigFailed => "record.micConfigFailed",
            ErrorCode::RecordMicError => "record.micError",
            ErrorCode::RecordMicFormatUnsupported => "record.micFormatUnsupported",
            ErrorCode::RecordMicOpenFailed => "record.micOpenFailed",
            ErrorCode::RecordMicTimeout => "record.micTimeout",
            ErrorCode::RecordMicWarmupFailed => "record.micWarmupFailed",
            ErrorCode::RecordNoInputDevice => "record.noInputDevice",
            ErrorCode::RecordNotRecording => "record.notRecording",
            ErrorCode::RecordOfflineTimeout => "record.offlineTimeout",
            ErrorCode::RecordStreamTimeout => "record.streamTimeout",
            ErrorCode::RecordStopFailed => "record.stopFailed",
            ErrorCode::DownloadCancelled => "download.cancelled",
            ErrorCode::RecordTooShort => "record.tooShort",
            ErrorCode::ResultNoSave => "result.noSave",
            ErrorCode::SettingsOpenDirFailed => "settings.openDirFailed",
            ErrorCode::SetupBusy => "setup.busy",
            ErrorCode::SetupCancelled => "setup.cancelled",
        }
    }

    /// 按字典 key 反查错误码（比较逻辑用）。
    ///
    /// `is_stop_failed` / `is_cancelled` 拿到的都是**字符串**（错误从
    /// 阻塞线程闭包里回来都是 String），所以比较要能「字符串 → 枚举」。
    /// 走同一张 key 表 —— [`ErrorCode::key`] 是它的逆。
    pub fn from_key(key: &str) -> Option<ErrorCode> {
        ALL_ERROR_CODES
            .iter()
            .copied()
            .find(|code| code.key() == key)
    }
}

/// 全部错误码（`from_key` 的查找表）。
///
/// 门禁 `check:i18n` 查的就是这张表 —— 每个变体的 key 必须在字典里。
pub const ALL_ERROR_CODES: &[ErrorCode] = &[
    ErrorCode::CloudAuthFailed,
    ErrorCode::CloudConnectFailed,
    ErrorCode::CloudDisconnected,
    ErrorCode::CloudNoResult,
    ErrorCode::CloudNotConfigured,
    ErrorCode::CloudProfileNotFound,
    ErrorCode::CloudProtocolError,
    ErrorCode::CloudSendFailed,
    ErrorCode::CloudTimeout,
    ErrorCode::CloudUnsupportedModel,
    ErrorCode::CloudUnsupportedProvider,
    ErrorCode::CloudVadInvalid,
    ErrorCode::CloudVadMissing,
    ErrorCode::DecodeFailed,
    ErrorCode::DecodeNoAudioTrack,
    ErrorCode::DecodeOpenFailed,
    ErrorCode::DecodeUnknownFormat,
    ErrorCode::ErrorAppConfigDir,
    ErrorCode::ErrorAppDataDirFailed,
    ErrorCode::ErrorCloudState,
    ErrorCode::ErrorCloudThreadFailed,
    ErrorCode::ErrorConnectFailed,
    ErrorCode::ErrorCreateDirFailed,
    ErrorCode::ErrorCreateModelDir,
    ErrorCode::ErrorCreateOutputDir,
    ErrorCode::ErrorCreatePunctDir,
    ErrorCode::ErrorCreateSettingsDir,
    ErrorCode::ErrorCreateStreamingDir,
    ErrorCode::ErrorDownloadFailed,
    ErrorCode::ErrorDownloadInterrupted,
    ErrorCode::ErrorEngineState,
    ErrorCode::ErrorMenuFailed,
    ErrorCode::ErrorOfflineThreadFailed,
    ErrorCode::ErrorPunctEngineState,
    ErrorCode::ErrorReadFileFailed,
    ErrorCode::ErrorRecordState,
    ErrorCode::ErrorRecordThreadFailed,
    ErrorCode::ErrorSaveSettingsFailed,
    ErrorCode::ErrorSettingsState,
    ErrorCode::ErrorShaMismatch,
    ErrorCode::ErrorSizeMismatch,
    ErrorCode::ErrorStreamState,
    ErrorCode::ErrorStreamEngineState,
    ErrorCode::ErrorStreamThreadFailed,
    ErrorCode::ErrorUnsupportedLanguage,
    ErrorCode::ErrorUnsupportedMode,
    ErrorCode::ErrorVerifyFailed,
    ErrorCode::ErrorWriteCacheFailed,
    ErrorCode::ErrorWriteFailed,
    ErrorCode::ErrorWriteFileFailed,
    ErrorCode::ModelLoadFailed,
    ErrorCode::ModelNeedOffline,
    ErrorCode::ModelNeedStreaming,
    ErrorCode::ModelPunctLoadFailed,
    ErrorCode::ModelPunctNotReady,
    ErrorCode::ModelStreamLoadFailed,
    ErrorCode::ModelVadMissing,
    ErrorCode::RecordAlreadyRecording,
    ErrorCode::RecordCloudTimeout,
    ErrorCode::RecordConfigLocked,
    ErrorCode::RecordDeviceNotFound,
    ErrorCode::RecordEmpty,
    ErrorCode::RecordMicConfigFailed,
    ErrorCode::RecordMicError,
    ErrorCode::RecordMicFormatUnsupported,
    ErrorCode::RecordMicOpenFailed,
    ErrorCode::RecordMicTimeout,
    ErrorCode::RecordMicWarmupFailed,
    ErrorCode::RecordNoInputDevice,
    ErrorCode::RecordNotRecording,
    ErrorCode::RecordOfflineTimeout,
    ErrorCode::RecordStreamTimeout,
    ErrorCode::RecordStopFailed,
    ErrorCode::DownloadCancelled,
    ErrorCode::RecordTooShort,
    ErrorCode::ResultNoSave,
    ErrorCode::SettingsOpenDirFailed,
    ErrorCode::SetupBusy,
    ErrorCode::SetupCancelled,
];

impl std::fmt::Display for ErrorCode {
    /// 输出**就是字典 key** —— 对外契约不变（前端 `core.js` 的 `fmtError()`
    /// 照旧查字典翻译）。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.key())
    }
}

/// 错误码：后端只返回字典 key（前端 i18n 翻译），不返回展示文案。
///
/// `code(ErrorCode::RecordEmpty)` → `"record.empty"`；
/// `code_detail(ErrorCode::RecordMicError, e)` → `"record.micError:<e>"`。
pub fn code(key: ErrorCode) -> String {
    key.to_string()
}

/// 带细节的错误码：细节是技术信息（路径、系统错误文本、格式名）。
///
/// **细节只进日志，不进界面**：用户看到的是字典里的干净文案
/// （`record.micError` → 「麦克风发生异常」），细节写进日志文件
/// （`~/Library/Logs/com.github.bytes-astra.voicenotes/voice-notes.log`，见 log.rs）。
/// 细节仍然跟着错误码一起回给前端，是为了填 `{value}` 这类
/// 用户看得懂的参数（比如「找不到输入设备：{value}」）。
pub fn code_detail(key: ErrorCode, detail: impl std::fmt::Display) -> String {
    let detail = detail.to_string();
    // 细节只进日志：用户看到的是字典里的干净文案。
    log::error(format!("{key}: {detail}"));
    format!("{key}:{detail}")
}

#[cfg(test)]
mod tests;
