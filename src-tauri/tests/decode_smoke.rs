//! 后端解码冒烟测试：symphonia 从磁盘解码 → 16 kHz 单声道。
//!
//! fixture 都在 `tests/fixtures/`：
//!   sample.m4a  — afconvert 造的 AAC（mp4 容器）
//!   broken.mp4  — sample.m4a 的前 1KB（损坏文件）

use std::path::Path;

use voice_notes_lib::decode::decode_to_16k;

fn fixture(name: &str) -> Option<std::path::PathBuf> {
    let path = Path::new("tests/fixtures").join(name);
    if path.exists() {
        Some(path)
    } else {
        eprintln!("跳过：缺少 {name}");
        None
    }
}

#[test]
fn decodes_wav_to_16k() {
    let Some(path) = fixture("zh.wav") else { return };
    let mut total = 0usize;
    let info = decode_to_16k(&path, |chunk| {
        total += chunk.len();
        Ok(())
    })
    .expect("解码 wav");
    println!("wav → {total} 个采样 @ {} Hz", info.sample_rate);
    assert_eq!(info.sample_rate, 16_000);
    assert!(total > 16_000, "至少应该解出 1 秒");
    assert_eq!(info.samples as usize, total);
}

#[test]
fn decodes_m4a_aac() {
    let Some(path) = fixture("sample.m4a") else { return };
    let mut total = 0usize;
    let info = decode_to_16k(&path, |chunk| {
        total += chunk.len();
        Ok(())
    })
    .expect("解码 m4a");
    println!("m4a → {total} 个采样 @ {} Hz", info.sample_rate);
    assert_eq!(info.sample_rate, 16_000);
    assert!(total > 16_000, "至少应该解出 1 秒");
}

#[test]
fn corrupted_file_returns_error() {
    let Some(path) = fixture("broken.mp4") else { return };
    let err = decode_to_16k(&path, |_| Ok(())).expect_err("损坏文件应该报错");
    println!("损坏文件报错：{err}");
    assert!(!err.is_empty());
}

#[test]
fn missing_file_returns_error() {
    let err = decode_to_16k(Path::new("tests/fixtures/does-not-exist.wav"), |_| Ok(()))
        .expect_err("不存在的文件应该报错");
    assert!(!err.is_empty());
}

/// 全链路：文件 → 解码 → VAD 切句 → SenseVoice 识别。
///
/// 需要 SenseVoice 模型，没设置就跳过：
///   VOICE_NOTES_MODEL_DIR=/path/to/models cargo test --test decode_smoke -- --nocapture
#[test]
fn transcribes_file_end_to_end() {
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
    let vad_model = [
        dir.parent().map(|p| p.join("silero/silero_vad.onnx")),
        Some(dir.join("silero_vad.onnx")),
    ]
    .into_iter()
    .flatten()
    .find(|p| p.is_file());

    let paths = voice_notes_lib::models::ModelPaths {
        dir: dir.display().to_string(),
        model: model.display().to_string(),
        tokens: tokens.display().to_string(),
        vad: vad_model.map(|p| p.display().to_string()),
        source: "test".into(),
    };
    let engine = std::sync::Arc::new(
        voice_notes_lib::engine::Engine::new(&paths, "zh", true).expect("加载模型"),
    );
    let vad = engine.vad_config().expect("VAD 配置");
    let mut session = voice_notes_lib::asr::OfflineSession::new(engine.clone(), vad);

    let path = Path::new("tests/fixtures/sample.m4a");
    if !path.exists() {
        eprintln!("跳过：缺少 sample.m4a");
        return;
    }
    let info = decode_to_16k(path, |chunk| {
        for event in session.push(chunk) {
            let voice_notes_lib::asr::OfflineEvent::Segment(segment) = event;
            println!("  句子：{}（{:.2}s~{:.2}s）", segment.text, segment.start, segment.end);
        }
        Ok(())
    })
    .expect("解码");
    for event in session.finish() {
        let voice_notes_lib::asr::OfflineEvent::Segment(segment) = event;
        println!("  收尾句子：{}", segment.text);
    }

    let text = voice_notes_lib::asr::join_segments(session.segments().iter().map(|s| s.text.as_str()));
    println!("识别结果：{text}（{:.2}s 音频，{} 句）", info.samples as f32 / 16000.0, session.segments().len());
    assert!(!text.trim().is_empty(), "识别结果为空");
    assert!(!session.segments().is_empty());
}
