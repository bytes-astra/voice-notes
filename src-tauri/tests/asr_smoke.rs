//! 端到端冒烟测试：用真实模型转写一段中文语音。
//!
//! 模型不存在时自动跳过（CI/新机器上不阻塞）：
//!   VOICE_NOTES_MODEL_DIR=/path/to/models cargo test --test asr_smoke -- --nocapture

use std::path::PathBuf;

use voice_notes_lib::asr::{OfflineEvent, OfflineSession};
use voice_notes_lib::engine::Engine;
use sherpa_onnx::{SileroVadModelConfig, VadModelConfig};
use voice_notes_lib::models::ModelPaths;

fn model_paths() -> Option<ModelPaths> {
    let dir = std::env::var("VOICE_NOTES_MODEL_DIR")
        .ok()
        .map(PathBuf::from)?;

    let model = [dir.join("model.int8.onnx"), dir.join("model.onnx")]
        .into_iter()
        .find(|path| path.is_file())?;
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
    .find(|path| path.is_file());

    Some(ModelPaths {
        dir: dir.display().to_string(),
        model: model.display().to_string(),
        tokens: tokens.display().to_string(),
        vad: vad.map(|path| path.display().to_string()),
        source: "test".into(),
    })
}

#[test]
fn transcribes_chinese_speech() {
    let Some(paths) = model_paths() else {
        eprintln!("跳过：找不到 SenseVoice 模型");
        return;
    };

    let engine = Engine::new(&paths, "zh", true).expect("加载模型");
    let wave = sherpa_onnx::Wave::read("tests/fixtures/zh.wav").expect("读取测试音频");
    assert_eq!(wave.sample_rate(), 16_000);

    let transcript = engine.transcribe(wave.samples());
    println!("识别结果：{}", transcript.text);
    println!(
        "音频 {:.2}s，耗时 {}ms，分句 {} 段",
        transcript.duration_seconds,
        transcript.elapsed_ms,
        transcript.segments.len()
    );

    assert!(!transcript.text.is_empty(), "转写结果为空");
    assert!(!transcript.segments.is_empty(), "VAD 没有切出语音段");
    assert!(transcript.duration_seconds > 0.0);
}

#[test]
fn joins_sensevoice_segments() {
    // 纯逻辑回归，不需要模型。
    assert_eq!(
        voice_notes_lib::asr::join_segments(["今天天气不错", "我们出去走走吧"].into_iter()),
        "今天天气不错我们出去走走吧"
    );
}

/// recognize 与 transcribe 对同一段音频给出一致的文本（证明去掉内部 VAD 没改变解码）
#[test]
fn recognize_matches_transcribe_on_one_sentence() {
    let Some(paths) = model_paths() else {
        eprintln!("跳过：找不到 SenseVoice 模型");
        return;
    };
    let wave = sherpa_onnx::Wave::read("tests/fixtures/zh.wav").expect("读取测试音频");
    let engine = Engine::new(&paths, "zh", true).expect("加载模型");

    let direct = engine.recognize(wave.samples());
    let batch = engine.transcribe(wave.samples());

    println!("recognize：{direct}");
    println!("transcribe：{}", batch.text);
    assert!(!direct.trim().is_empty());
    assert_eq!(direct.trim(), batch.text.trim());
}

/// 边录边识别与整条一次识别的文本一致（去掉空白和标点后比字符）
#[test]
fn offline_session_matches_batch_transcribe() {
    let Some(paths) = model_paths() else {
        eprintln!("跳过：找不到 SenseVoice 模型");
        return;
    };
    let wave = sherpa_onnx::Wave::read("tests/fixtures/zh.wav").expect("读取测试音频");
    let engine = std::sync::Arc::new(Engine::new(&paths, "zh", true).expect("加载模型"));

    // 路径 A：边录边（按 3200 样本推进）
    let mut session = OfflineSession::new(engine.clone(), vad_config(paths.vad.as_deref()));
    let mut events = Vec::new();
    for chunk in wave.samples().chunks(3200) {
        events.extend(session.push(chunk));
    }
    events.extend(session.finish());
    let live: String = events
        .iter()
        .map(|e| match e {
            OfflineEvent::Segment(s) => s.text.as_str(),
        })
        .collect();

    // 路径 B：整条一次
    let batch = engine.transcribe(wave.samples());

    println!("边录边：{live}");
    println!("整条一次：{}", batch.text);
    let norm = |s: &str| -> String {
        s.chars()
            .filter(|c| !c.is_whitespace() && !c.is_ascii_punctuation() && !"，。！？、；：“”‘’（）《》".contains(*c))
            .collect()
    };
    assert!(!norm(&live).is_empty());
    assert_eq!(norm(&live), norm(&batch.text));
}

/// 说到一半停止：finish() 要把没成句的剩余音频也识别出来
#[test]
fn offline_session_finishes_partial_sentence() {
    let Some(paths) = model_paths() else {
        eprintln!("跳过：找不到 SenseVoice 模型");
        return;
    };
    let wave = sherpa_onnx::Wave::read("tests/fixtures/zh.wav").expect("读取测试音频");
    let engine = std::sync::Arc::new(Engine::new(&paths, "zh", true).expect("加载模型"));
    let mut session = OfflineSession::new(engine, vad_config(paths.vad.as_deref()));
    // 只喂前 1.2 秒（不够 VAD 判静音成句），然后 finish
    let head = &wave.samples()[..(16_000 * 12 / 10).min(wave.samples().len())];
    let _ = session.push(head);
    let events = session.finish();
    let text: String = events
        .iter()
        .map(|e| match e {
            OfflineEvent::Segment(s) => s.text.as_str(),
        })
        .collect();
    println!("收尾识别：{text}");
    assert!(!text.trim().is_empty(), "收尾时应该把没说完的半句也识别出来");
}

/// 与 asr.rs 里整段模式同一套 VAD 参数。
fn vad_config(vad_model: Option<&str>) -> VadModelConfig {
    VadModelConfig {
        silero_vad: SileroVadModelConfig {
            model: vad_model.map(|p| p.to_string()),
            threshold: 0.5,
            min_silence_duration: 0.5,
            min_speech_duration: 0.25,
            window_size: 512,
            max_speech_duration: 30.0,
        },
        sample_rate: 16_000,
        num_threads: 1,
        provider: Some("cpu".into()),
        debug: false,
        ..Default::default()
    }
}

/// 长录音：15 分钟音频边录边识别，句子一句不少，未消费缓冲始终有界。
///
/// 这是整个 Phase B 的核心承诺（「支持长时间的内容」），必须真的跑一遍：
///   VOICE_NOTES_MODEL_DIR=/path/to/models cargo test --test asr_smoke -- --nocapture
#[test]
fn handles_fifteen_minute_recording() {
    let Some(dir) = std::env::var("VOICE_NOTES_MODEL_DIR").ok().map(std::path::PathBuf::from) else {
        eprintln!("跳过：未设置 VOICE_NOTES_MODEL_DIR");
        return;
    };
    let model = [dir.join("model.int8.onnx"), dir.join("model.onnx")]
        .into_iter()
        .find(|p| p.is_file());
    let (Some(model), tokens) = (model, dir.join("tokens.txt")) else {
        eprintln!("跳过：找不到 SenseVoice 模型");
        return;
    };
    if !tokens.is_file() {
        eprintln!("跳过：找不到 tokens.txt");
        return;
    }
    let mut vad_candidates = vec![dir.join("silero_vad.onnx")];
    if let Some(parent) = dir.parent() {
        vad_candidates.push(parent.join("silero_vad.onnx"));
    }
    let Some(vad_model) = vad_candidates.into_iter().find(|p| p.is_file()) else {
        eprintln!("跳过：找不到 VAD 模型");
        return;
    };
    let paths = voice_notes_lib::models::ModelPaths {
        dir: dir.display().to_string(),
        model: model.display().to_string(),
        tokens: tokens.display().to_string(),
        vad: Some(vad_model.display().to_string()),
        source: "test".into(),
    };
    let engine = std::sync::Arc::new(
        voice_notes_lib::engine::Engine::new(&paths, "zh", true).expect("加载模型"),
    );
    let vad = engine.vad_config().expect("VAD 配置");

    let wave = sherpa_onnx::Wave::read("tests/fixtures/zh.wav").expect("读取测试音频");
    let chunk = wave.samples();
    // 5.6 秒 × 160 ≈ 15 分钟
    const REPEATS: usize = 160;
    let total_samples = chunk.len() * REPEATS;
    let minutes = total_samples as f32 / 16_000.0 / 60.0;
    println!("模拟录音 {minutes:.1} 分钟（{total_samples} 个采样）");

    let queue = voice_notes_lib::audio::SampleQueue::new(16_000);
    let mut session = voice_notes_lib::asr::OfflineSession::new(engine.clone(), vad);
    let mut segments = 0usize;
    let mut max_pending = 0usize;

    // 模拟「边录边消费」：每喂 1 秒，取一次（和 ASR 线程的实际节奏一致）
    let step = 16_000usize;
    for _ in 0..REPEATS {
        for piece in chunk.chunks(step) {
            queue.push(piece);
            let new_audio = queue.take_new();
            max_pending = max_pending.max(queue.pending());
            for event in session.push(&new_audio.samples) {
                let voice_notes_lib::asr::OfflineEvent::Segment(segment) = event;
                segments += 1;
                if segments % 40 == 1 {
                    println!("  第 {segments} 句：{}", segment.text);
                }
            }
        }
    }
    for event in session.finish() {
        let voice_notes_lib::asr::OfflineEvent::Segment(_) = event;
        segments += 1;
    }

    println!(
        "识别出 {segments} 句，未消费缓冲峰值 {max_pending} 个采样（{:.1} 秒）",
        max_pending as f32 / 16_000.0
    );
    let text = voice_notes_lib::asr::join_segments(session.segments().iter().map(|s| s.text.as_str()));
    println!("总字数：{}", text.chars().count());

    // 15 分钟 ≈ 160 段，每段都应该识别出来（允许 VAD 边界处的少量出入）
    assert!(
        segments * 100 >= REPEATS * 95,
        "15 分钟只识别出 {segments} 句，漏了内容"
    );
    // 未消费缓冲必须恒定在上限以内（内存恒定的核心）
    let limit = 60 * 16_000;
    assert!(max_pending <= limit, "未消费缓冲涨到了 {max_pending}（上限 {limit}）");
    assert!(text.chars().count() > 1000, "识别出的字数太少");
}
