//! 云端日志的**唯一出口**。
//!
//! 凭据可能出现在服务端回显的文本里 —— 有的网关会把「无效的 key: sk-xxx」
//! 原样退回来。所以凡是可能带 provider 文本的日志都走这里：**写入前先把凭据
//! 从文本里抹掉**。
//!
//! **这是结构约束，不是习惯**：`asr/cloud/` 下的代码不许直接调 `crate::log::`
//! —— 守卫 `asr::cloud::tests::cloud_logs_go_through_the_redacting_choke_point`
//! 盯着（它读源码找绕过关口的调用）。谁想写日志都得从这里过，脱敏就再也绕不过去。
//!
//! **ERROR 级走 `code_detail`**：`crate::code_detail` 内部就是 `log::error`
//! （细节只进日志、错误码串回前端），所以云端的 ERROR 级日志由
//! `code_detail` 承担 —— 它也脱敏。
//!
//! 关口的范围只有云端模块：`record.rs` 那些日志只拼字典码，没有 provider 文本
//! 也没有凭据（边界见 OpenSpec change `cloud-log-redaction` 的 design D3）。

use super::sanitize;

/// 写一条 WARN：写入前把凭据从文本里抹掉。
///
/// `secret` 是**本 provider 的凭据**；`None` 表示这条日志是本模块自己写的
/// 固定文案或纯数字，不含 provider 文本（会话层拿不到凭据 —— 凭据在 provider
/// 的配置里）。
pub(crate) fn warn(text: &str, secret: Option<&str>) {
    crate::log::warn(redact(text, secret));
}

/// 写一条 INFO：写入前把凭据从文本里抹掉（同 `warn`）。
pub(crate) fn info(text: &str, secret: Option<&str>) {
    crate::log::info(redact(text, secret));
}

/// 云端的错误详情：**先脱敏**，再写日志、返回错误码串。
///
/// 与 crate 的 `code_detail` 同形状（细节只进日志、错误码串回前端），
/// 但多一步脱敏 —— 云端模块**一律用这个**，不许直接调 `crate::code_detail`：
/// 那条路也写日志（`crate::code_detail` 内部就是 `log::error`），却不脱敏。
/// 源码级守卫盯着 `crate::code_detail` 这个路径（见
/// `asr::cloud::tests::cloud_logs_go_through_the_redacting_choke_point`）。
///
/// `secret` 同 `warn`；`None` 表示细节里没有 provider 文本（固定文案、
/// 传输层错误、帧解析错误）。
pub(crate) fn code_detail(
    key: &str,
    detail: impl std::fmt::Display,
    secret: Option<&str>,
) -> String {
    let detail = redact(&detail.to_string(), secret);
    crate::code_detail(key, detail)
}

/// 把凭据从文本里抹掉（纯函数，可单测）。
///
/// `None` → 原样返回（这条日志不含 provider 文本）。
pub(crate) fn redact(text: &str, secret: Option<&str>) -> String {
    match secret {
        Some(secret) => sanitize(text, secret),
        None => text.to_string(),
    }
}
