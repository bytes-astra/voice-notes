//! 结果质量检测：识别结果明显异常时提醒用户。
//!
//! 这里只做「检测」，不做「纠正」——早期版本会在流式结束后用整段模型复核
//! 整条音频，但实测流式准确率不低于整段（修好收尾后 99% vs 91%），
//! 复核反而会把「明天」听成「今天」。而且复核要求保留全部音频，
//! 和「录音时长不限、内存恒定」的目标直接冲突。

use std::collections::HashSet;

/// 流式结果是否明显异常：空、重复字、或字少得跟音频时长对不上。
///
/// 实测修好收尾后流式的准确率不低于 SenseVoice，所以这里只判定「明显坏掉」，
/// 命中也只是提醒用户，不改动已经识别出来的文本。
pub fn is_degenerate(text: &str, duration_seconds: f32) -> bool {
    let chars: Vec<char> = text.chars().filter(|c| !c.is_whitespace()).collect();
    if chars.is_empty() {
        return true;
    }
    // 重复字：唯一字符占比过低（“走走走走走”）
    let unique = chars.iter().collect::<HashSet<_>>().len();
    if unique * 2 < chars.len() {
        return true;
    }
    // 字数明显少于音频时长能说出的量
    let expected = (duration_seconds * 0.5).ceil().max(1.0) as usize;
    chars.len() < expected
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_empty_and_repeated() {
        assert!(is_degenerate("", 3.0));
        assert!(is_degenerate("   ", 3.0));
        assert!(is_degenerate("走走走走走走", 3.0));
        assert!(!is_degenerate("今天下午三点开一个产品评审会", 3.0));
    }

    #[test]
    fn detects_too_few_chars_for_duration() {
        // 10 秒音频只识别出 1 个字：明显偏少
        assert!(is_degenerate("好", 10.0));
        assert!(!is_degenerate(
            "今天下午三点开一个产品评审会材料已经发到群里了",
            10.0
        ));
    }
}
