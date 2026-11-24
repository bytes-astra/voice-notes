//! `error_code` 模块的单测：变体 ↔ key 的映射。

use super::*;

/// 每个变体的 key 必须是「前缀.驼峰」形状 —— 界面按这个形状识别错误码
/// （前端 `core.js` 的 `isErrorCode`）。
#[test]
fn every_error_code_key_looks_like_an_error_code() {
    for code in ALL_ERROR_CODES {
        let key = code.key();
        assert!(
            key.split('.').count() >= 2,
            "错误码 {key} 至少要有两段（模块.名字）"
        );
        assert!(
            key.chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '.'),
            "错误码 {key} 只能有字母、数字和点"
        );
    }
}

/// `from_key` 与 `Display` 必须是**同一张表** —— 两份表会漂：
/// 漂了就出现「显示的 key 与比较用的 key 对不上」。
#[test]
fn from_key_and_display_agree_on_every_code() {
    for code in ALL_ERROR_CODES {
        let key = code.key();
        assert_eq!(
            ErrorCode::from_key(key),
            Some(*code),
            "错误码 {key} 反查不回来 —— from_key 与 Display 不是同一张表"
        );
    }
}

/// 查不到的 key 返回 `None`（而不是 panic）。
#[test]
fn unknown_key_is_none() {
    assert_eq!(ErrorCode::from_key("record.notARealCode"), None);
    assert_eq!(ErrorCode::from_key(""), None);
}

/// 枚举里没有**重复的 key** —— 重复会让 `from_key` 只认第一个，
/// 另一个错误码的比较逻辑永远不成立。
#[test]
fn error_code_keys_are_unique() {
    let mut seen = std::collections::HashSet::new();
    for code in ALL_ERROR_CODES {
        assert!(seen.insert(code.key()), "错误码 key 重复了：{}", code.key());
    }
}

/// `ALL_ERROR_CODES` 与枚举的变体**一一对应** —— 少登记一个，
/// 那个错误码的 `from_key` 就永远反查不回来。
///
/// 变体与 key 的对应由 `key()` 的 match 决定（唯一真值源）；
/// 这条查的是「`ALL_ERROR_CODES` 覆盖了每个 key」——
/// 少登记一个，那个 key 就反查不到。
#[test]
fn all_error_codes_covers_every_defined_key() {
    let mut seen = std::collections::HashSet::new();
    for code in ALL_ERROR_CODES {
        assert!(seen.insert(code.key()), "重复登记：{}", code.key());
    }
    // `key()` 的 match 里出现的每个 key 都要在 ALL_ERROR_CODES 里。
    for key in defined_keys() {
        assert!(
            seen.contains(key.as_str()),
            "错误码 {key} 在 key() 里定义了，却没登记进 ALL_ERROR_CODES —— from_key 会漏"
        );
    }
}

/// 从源码里抽出 `ErrorCode::key()` 的 match 里定义的 key。
fn defined_keys() -> Vec<String> {
    let source = include_str!("../error_code.rs");
    let body = source
        .split("pub fn key(self) -> &'static str {")
        .nth(1)
        .and_then(|rest| rest.split("}").next())
        .unwrap_or("");
    body.lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            let rest = trimmed.strip_prefix("ErrorCode::")?;
            let (_, key) = rest.split_once("=>")?;
            Some(
                key.trim()
                    .trim_end_matches(',')
                    .trim_matches('"')
                    .to_string(),
            )
        })
        .collect()
}
