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
        .and_then(|rest| rest.split('}').next())
        .unwrap_or("");
    keys_in(body)
}

/// 从 `key()` 的函数体里抽出每个 arm 的 key。
///
/// **不按行解析**：rustfmt 在 arm 太长时会把 `=> "key"` 折到下一行，
/// 逐行匹配（`strip_prefix("ErrorCode::")` 后再 `split_once("=>")`）会让
/// 那条 arm **静默跳过** —— 少检查一个 key 就是假绿：新加的变体漏登记进
/// `ALL_ERROR_CODES` 也照样通过。这里按「`ErrorCode::` 之后第一个 `=>`、
/// 再后面第一对引号」抽，跨行照样认得出。
fn keys_in(body: &str) -> Vec<String> {
    let mut keys = Vec::new();
    for chunk in body.split("ErrorCode::").skip(1) {
        let Some(arrow) = chunk.find("=>") else {
            continue;
        };
        let after = &chunk[arrow + 2..];
        let Some(open) = after.find('"') else {
            continue;
        };
        let rest = &after[open + 1..];
        let Some(close) = rest.find('"') else {
            continue;
        };
        keys.push(rest[..close].to_string());
    }
    keys
}

/// 守卫自证：`=>` 被折到下一行时，key 仍要被抽出来。
///
/// 这条防的是「守卫自己退化成空测」—— 实测过：按行解析的版本对下面这段
/// 源码返回空数组（`BetaLongName` 那条整个消失）。
#[test]
fn key_extraction_survives_rustfmt_line_breaks() {
    let body = r#"
            ErrorCode::Alpha => "a.b",
            ErrorCode::BetaLongName
                => "c.d",
            ErrorCode::Gamma => {
                "e.f"
            }
"#;
    assert_eq!(
        keys_in(body),
        vec!["a.b", "c.d", "e.f"],
        "折行 / 花括号形态的 arm 必须照样抽得出 key，否则守卫会静默放过漏登记的变体"
    );
}
