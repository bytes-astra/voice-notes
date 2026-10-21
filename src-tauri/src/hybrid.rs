//! 流式 + 非流式混合：流式结束后用 SenseVoice 校正最终文本。
//!
//! 录音的 PCM 在流式模式里本来就完整保留着，停止后再跑一遍非流式引擎
//! （约 200ms），就能把「边说边出」的实时体验和最终稿的准确度结合起来。

use std::collections::HashSet;

use crate::asr::{join_segments, PunctuationEngine, Transcript};

/// 最短可校正的采样数（16kHz 下 0.2 秒），太短的音频不值得再跑一次。
const MIN_SAMPLES: usize = 16_000 / 5;

/// 是否该做校正：开关打开、非流式模型就绪、音频不太短。
pub fn should_correct(enabled: bool, model_ready: bool, sample_count: usize) -> bool {
    enabled && model_ready && sample_count >= MIN_SAMPLES
}

/// 中文/英文标点：用来判断一段文本是否已经带标点。
const PUNCTUATION_MARKS: [char; 12] = [
    '，', '。', '！', '？', '、', '；', '：', '“', '”', '（', '）', ',',
];

/// 给识别结果补标点：逐句加标点后重新拼接。
///
/// 已经带标点的句子原样保留：SenseVoice 开 ITN 时自己就输出标点，
/// 再套一遍标点模型会变成「。。」。
pub fn punctuate(transcript: Transcript, punctuator: &PunctuationEngine) -> Transcript {
    let mut out = transcript;
    for segment in out.segments.iter_mut() {
        if segment.text.chars().any(|c| PUNCTUATION_MARKS.contains(&c)) {
            continue;
        }
        segment.text = punctuator.apply(&segment.text);
    }
    out.text = join_segments(out.segments.iter().map(|s| s.text.as_str()));
    out
}

/// 流式结果是否明显异常：空、重复字、或字少得跟音频时长对不上。
///
/// 实测修好收尾后流式的准确率不低于 SenseVoice，所以只在结果明显坏掉时
/// 才让校正结果接管，正常结果一律保留。
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

/// 合并流式结果与校正结果。
///
/// 只有当流式结果明显异常、且校正结果非空时才用校正结果接管；
/// 否则保留流式文本（实测它更准）。总耗时按两段相加。
pub fn finalize(streaming: Transcript, corrected: Option<Transcript>) -> Transcript {
    let use_corrected = match &corrected {
        Some(corrected) => {
            !corrected.text.trim().is_empty()
                && is_degenerate(&streaming.text, streaming.duration_seconds)
        }
        None => false,
    };
    if !use_corrected {
        return streaming;
    }
    let Some(corrected) = corrected else {
        return streaming;
    };
    Transcript {
        text: corrected.text,
        segments: corrected.segments,
        duration_seconds: corrected.duration_seconds,
        elapsed_ms: streaming.elapsed_ms + corrected.elapsed_ms,
        language: corrected.language,
        corrected: true,
    }
}
