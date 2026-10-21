//! 流式 + 非流式混合（校正）逻辑测试。
//!
//! 纯逻辑部分不需要模型；标点部分需要标点模型：
//!   VOICE_NOTES_PUNCT_MODEL=/path/to/model.onnx cargo test --test hybrid -- --nocapture

use std::path::PathBuf;
use std::sync::Arc;

use voice_notes_lib::asr::{
    Engine, PunctuationEngine, Segment, StreamingEngine, StreamingSession, Transcript,
};
use voice_notes_lib::hybrid::{finalize, is_degenerate, punctuate, should_correct};
use voice_notes_lib::models::{ModelPaths, StreamingPaths};

fn transcript(text: &str, elapsed_ms: u64) -> Transcript {
    transcript_with(text, elapsed_ms, 1.0)
}

fn transcript_with(text: &str, elapsed_ms: u64, duration_seconds: f32) -> Transcript {
    Transcript {
        text: text.into(),
        segments: vec![Segment {
            text: text.into(),
            start: 0.0,
            end: duration_seconds,
        }],
        duration_seconds,
        elapsed_ms,
        language: "zh".into(),
        corrected: false,
    }
}

#[test]
fn keeps_streaming_when_no_correction_available() {
    let streaming = transcript("流式结果", 120);
    let result = finalize(streaming.clone(), None);

    assert_eq!(result.text, "流式结果");
    assert!(!result.corrected, "没有校正结果时不应标记为已校正");
    assert_eq!(result.elapsed_ms, 120);
}

/// 流式结果正常时不该被覆盖：实测流式（修收尾后）比 SenseVoice 还准。
#[test]
fn keeps_healthy_streaming_result() {
    let streaming = transcript_with("今天下午三点开一个产品评审会。", 350, 3.5);
    let corrected = transcript("今天下午3点开一个产品评审会。", 200);

    let result = finalize(streaming, Some(corrected));

    assert_eq!(result.text, "今天下午三点开一个产品评审会。");
    assert!(!result.corrected, "正常结果不该被标记为已校正");
}

/// 流式结果明显异常（重复字/空/太少）时，才用校正结果接管。
#[test]
fn uses_correction_when_streaming_is_degenerate() {
    let streaming = transcript_with("走走走走走走走走走走走走走走", 350, 3.5);
    let corrected = transcript("今天下午三点开一个产品评审会。", 200);

    let result = finalize(streaming, Some(corrected));

    assert_eq!(result.text, "今天下午三点开一个产品评审会。");
    assert!(result.corrected, "异常结果被接管后应标记为已校正");
}

#[test]
fn detects_degenerate_streaming_text() {
    assert!(is_degenerate("", 3.5), "空文本算异常");
    assert!(is_degenerate("   ", 3.5), "空白文本算异常");
    assert!(
        is_degenerate("走走走走走走走走走走走走走走", 3.5),
        "重复字算异常"
    );
    assert!(is_degenerate("嗯", 5.0), "音频很长却只有一个字算异常");
    assert!(
        !is_degenerate("今天下午三点开一个产品评审会。", 3.5),
        "正常文本不算异常"
    );
    assert!(!is_degenerate("好", 0.5), "短句短音频不算异常");
}

#[test]
fn keeps_streaming_when_correction_is_blank() {
    let streaming = transcript("流式结果", 120);
    let corrected = transcript("   ", 200);

    let result = finalize(streaming, Some(corrected));

    assert_eq!(result.text, "流式结果");
    assert!(!result.corrected, "空白校正结果不应覆盖流式文本");
}

#[test]
fn sums_elapsed_time_of_both_passes() {
    let streaming = transcript_with("走走走走走走走走走走走走走走", 120, 3.5);
    let corrected = transcript("非流式结果", 200);

    let result = finalize(streaming, Some(corrected));

    assert_eq!(result.elapsed_ms, 320, "总耗时应是两段耗时之和");
}

#[test]
fn corrects_only_when_enabled_and_ready() {
    let long_enough = 16_000; // 1 秒
    assert!(
        should_correct(true, true, long_enough),
        "开关打开且模型就绪应校正"
    );
    assert!(
        !should_correct(false, true, long_enough),
        "开关关闭不应校正"
    );
    assert!(
        !should_correct(true, false, long_enough),
        "没有非流式模型不应校正"
    );
    assert!(!should_correct(true, true, 100), "音频太短不应校正");
}

/// SenseVoice 开着 ITN 时自己就带标点，再套一遍标点模型会变成「。。」。
#[test]
fn does_not_duplicate_existing_punctuation() {
    let Ok(model) = std::env::var("VOICE_NOTES_PUNCT_MODEL") else {
        eprintln!("跳过：没有标点模型（VOICE_NOTES_PUNCT_MODEL）");
        return;
    };
    let Ok(punctuator) = PunctuationEngine::new(&model) else {
        eprintln!("跳过：标点模型加载失败");
        return;
    };

    let corrected = transcript("今天上午10点，记得去三楼取快递。", 200);
    let result = punctuate(corrected, &punctuator);
    println!("已有标点：{}", result.text);

    assert!(
        !result.text.contains("。。"),
        "不该出现重复句号：{}",
        result.text
    );
    assert!(
        !result.text.contains("，，"),
        "不该出现重复逗号：{}",
        result.text
    );
    assert_eq!(result.text, "今天上午10点，记得去三楼取快递。");
}

#[test]
fn punctuates_corrected_text() {
    let Ok(model) = std::env::var("VOICE_NOTES_PUNCT_MODEL") else {
        eprintln!("跳过：没有标点模型（VOICE_NOTES_PUNCT_MODEL）");
        return;
    };
    let Ok(punctuator) = PunctuationEngine::new(&model) else {
        eprintln!("跳过：标点模型加载失败");
        return;
    };

    let corrected = transcript("开放时间早上九点至下午五点", 200);
    let result = punctuate(corrected, &punctuator);
    println!("标点后：{}", result.text);

    assert!(
        result.text.contains('，') && result.text.ends_with('。'),
        "标点后应出现逗号和句号，实际：{}",
        result.text
    );
    assert_eq!(result.segments[0].text, result.text, "分段文本应与整体一致");
}

// ── 真模型端到端：流式结果 vs 校正后的最终稿 ────────────────

fn offline_paths() -> Option<ModelPaths> {
    let dir = std::env::var("VOICE_NOTES_MODEL_DIR")
        .ok()
        .map(PathBuf::from)?;
    let model = [dir.join("model.int8.onnx"), dir.join("model.onnx")]
        .into_iter()
        .find(|p| p.is_file())?;
    let tokens = dir.join("tokens.txt");
    if !tokens.is_file() {
        return None;
    }
    let vad = [
        dir.parent().map(|p| p.join("silero/silero_vad.onnx")),
        Some(dir.join("silero_vad.onnx")),
    ]
    .into_iter()
    .flatten()
    .find(|p| p.is_file());
    Some(ModelPaths {
        dir: dir.display().to_string(),
        model: model.display().to_string(),
        tokens: tokens.display().to_string(),
        vad: vad.map(|p| p.display().to_string()),
        source: "test".into(),
    })
}

fn streaming_paths() -> Option<StreamingPaths> {
    let dir = std::env::var("VOICE_NOTES_STREAMING_MODEL_DIR")
        .ok()
        .map(PathBuf::from)?;
    let encoder = dir.join("encoder.int8.onnx");
    let decoder = dir.join("decoder.onnx");
    let joiner = dir.join("joiner.int8.onnx");
    let tokens = dir.join("tokens.txt");
    if !(encoder.is_file() && decoder.is_file() && joiner.is_file() && tokens.is_file()) {
        return None;
    }
    Some(StreamingPaths {
        dir: dir.display().to_string(),
        encoder: encoder.display().to_string(),
        decoder: decoder.display().to_string(),
        joiner: joiner.display().to_string(),
        tokens: tokens.display().to_string(),
        vad: [dir.join("silero_vad.onnx")]
            .into_iter()
            .find(|p| p.is_file())
            .map(|p| p.display().to_string()),
    })
}

fn punctuator() -> Option<PunctuationEngine> {
    let model = std::env::var("VOICE_NOTES_PUNCT_MODEL").ok()?;
    PunctuationEngine::new(&model).ok()
}

#[test]
fn keeps_healthy_streaming_result_end_to_end() {
    let (Some(offline), Some(streaming)) = (offline_paths(), streaming_paths()) else {
        eprintln!("跳过：缺少非流式或流式模型");
        return;
    };

    let wave = sherpa_onnx::Wave::read("tests/fixtures/zh.wav").expect("读取测试音频");

    // 流式：按 200ms 分片喂进去，最后 finish() 冲刷
    let engine = Arc::new(StreamingEngine::new(&streaming).expect("加载流式模型"));
    let punct = punctuator().map(Arc::new);
    let mut session = StreamingSession::new(engine, punct.clone());
    for chunk in wave.samples().chunks(3_200) {
        session.push(chunk);
    }
    session.finish();
    let streaming_result = session.transcript();

    // 非流式：同一段音频再跑一遍 SenseVoice
    let offline_engine = Engine::new(&offline, "auto", true).expect("加载非流式模型");
    let corrected = offline_engine.transcribe(wave.samples());
    let corrected = match punct.as_deref() {
        Some(p) => punctuate(corrected, p),
        None => corrected,
    };

    let final_result = finalize(streaming_result.clone(), Some(corrected.clone()));

    println!("流式结果：{}", streaming_result.text);
    println!("校正结果：{}", corrected.text);
    println!("最终文本：{}", final_result.text);

    assert!(
        !is_degenerate(&streaming_result.text, streaming_result.duration_seconds),
        "流式结果不应被判为异常：{}",
        streaming_result.text
    );
    assert!(
        !final_result.corrected,
        "正常的流式结果不该被非流式覆盖（实测流式更准）"
    );
    assert_eq!(final_result.text, streaming_result.text);
}

#[test]
fn rescues_degenerate_streaming_result() {
    let (Some(offline), Some(streaming)) = (offline_paths(), streaming_paths()) else {
        eprintln!("跳过：缺少非流式或流式模型");
        return;
    };

    let wave = sherpa_onnx::Wave::read("tests/fixtures/zh.wav").expect("读取测试音频");
    let offline_engine = Engine::new(&offline, "auto", true).expect("加载非流式模型");
    let mut corrected = offline_engine.transcribe(wave.samples());
    if let Some(p) = punctuator() {
        corrected = punctuate(corrected, &p);
    }
    assert!(!corrected.text.trim().is_empty(), "校正结果为空");

    // 流式结果坏掉（重复字）时，校正结果接管
    let broken = transcript_with("走走走走走走走走走走走走走走走走", 350, 3.5);
    let final_result = finalize(broken, Some(corrected.clone()));

    assert!(final_result.corrected, "异常结果应被校正接管");
    assert_eq!(final_result.text, corrected.text);
    assert_eq!(
        final_result.elapsed_ms,
        350 + corrected.elapsed_ms,
        "总耗时应是两段之和"
    );
    let _ = streaming;
}
