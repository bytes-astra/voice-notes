//! 「半个字被切断」的回归测试：字节 token 重建文本（`text_from_tokens`）。
//!
//! 背景：这个流式模型的词表只有 1426 个汉字，表外的字（番、茄、腩……）
//! 只能拆成字节 token `<0xE7><0x95><0xAA>` 拼。sherpa 把这些 token 还原成
//! **裸字节**放进 `text`，切分点落在半个字上时就是非法 UTF-8，
//! sherpa 自己的 `Utf8ToUtf32()` 会把它固化成 U+FFFD —— 界面上的「�」。
//! token 列表是无损的，从它重建就能还原出正确的字。

use super::*;

/// 字节 token 列表 → 文本。
fn tokens(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

#[test]
fn rebuilds_char_from_byte_tokens() {
    // 番 = E7 95 AA
    let text = text_from_tokens(&tokens(&["<0xE7>", "<0x95>", "<0xAA>"]));
    assert_eq!(text, "番");
}

#[test]
fn rebuilds_mixed_cjk_and_byte_tokens() {
    // 「番茄牛腩」：牛 在词表里，番/茄/腩 走字节
    let text = text_from_tokens(&tokens(&[
        "<0xE7>", "<0x95>", "<0xAA>", "<0xE8>", "<0x8C>", "<0x84>", "牛", "<0xE8>", "<0x85>",
        "<0xA9>",
    ]));
    assert_eq!(text, "番茄牛腩");
    assert!(!text.contains('\u{FFFD}'));
}

#[test]
fn withholds_incomplete_tail() {
    // 「酪」= E9 85 AA，只解出第一个字节：半个字不能进文本，
    // 也不能变成「�」—— 等下一帧补齐。
    let text = text_from_tokens(&tokens(&["奶", "<0xE9>"]));
    assert_eq!(text, "奶");
    assert!(!text.contains('\u{FFFD}'));

    // 补齐之后完整的字才出来
    let text = text_from_tokens(&tokens(&["奶", "<0xE9>", "<0x85>", "<0xAA>"]));
    assert_eq!(text, "奶酪");
}

#[test]
fn withholds_incomplete_head() {
    // 段首：sherpa Reset() 会把上一段末尾的 token 当 context 留给新段，
    // 上一段停在半个字上时，新段开头就是半个字。
    let text = text_from_tokens(&tokens(&["<0xE8>", "<0x8B>"]));
    assert_eq!(text, "");
    assert!(!text.contains('\u{FFFD}'));

    let text = text_from_tokens(&tokens(&["<0xE8>", "<0x8B>", "<0xB9>"]));
    assert_eq!(text, "苹");
}

#[test]
fn clean_drops_replacement_char() {
    // 兜底：万一还有别的路径带进来 U+FFFD，也不能出现在结果里
    assert_eq!(clean("番茄\u{FFFD}牛腩"), "番茄牛腩");
}
