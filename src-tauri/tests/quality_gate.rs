//! 质量门禁：识别质量**不得退化**。
//!
//! 与 `quality_probe` 的分工：
//! - `quality_probe` 只打印、不断言 —— 用来「先量后改」（换模型 / 调参数 / 加热词）。
//! - `quality_gate` 断言阈值 —— 用来「不许变差」。CI 没跑它（macOS runner 要下模型、
//!   跑一次十几分钟，成本太高），所以要手动跑。
//!
//! 量的**生产路径**：`OfflineSession`（VAD 切句 + 短句合并 + ITN）与
//! `StreamingSession`（Zipformer + 尾部补静音）。探针量的是裸引擎，
//! 门禁量的是用户真正听到的那条路 —— 门禁要守住的是后者。
//!
//! 语料（都不入库，本机生成 / 抓取）：
//! - `tests/fixtures/quality/tts-*.wav`（macOS `say` 合成，文本已知）→ **设阈值**，稳定基线
//! - `tests/fixtures/quality/real-*.wav`（minds14 真人电话录音）→ **只打印**，人工看
//!
//! 阈值能用环境变量覆盖（换模型要重新定基线时不用改代码）：
//! `VOICE_NOTES_MAX_OFFLINE_CER` / `VOICE_NOTES_MAX_STREAMING_CER` / `VOICE_NOTES_MAX_RTF`。
//!
//! ```bash
//! bash scripts/gen-tts-corpus.sh                      # 生成 TTS 语料
//! node scripts/fetch-quality-corpus.mjs               # 可选：抓真人语料（只看，不设阈值）
//! cd src-tauri
//! VOICE_NOTES_MODEL_DIR=... VOICE_NOTES_STREAMING_MODEL_DIR=... \
//!   cargo test --test quality_gate -- --nocapture --test-threads=1
//! ```
//!
//! 没有模型 / 没有语料时**跳过**（打印「跳过：…」）—— 那是「没验」，不是「过了」。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use voice_notes_lib::asr::{OfflineSession, StreamingSession};
use voice_notes_lib::engine::{Engine, StreamingEngine};
use voice_notes_lib::models::{ModelPaths, StreamingPaths};

mod common;
use common::{cer, corpus, category, env_f64, reference_for};

/// 整段 CER 上限（%）。基线是 TTS 语料上的实测值，留了余量：
/// 语料是本机合成的，不同 macOS 版本的 `say` 声线会有细微差别。
const DEFAULT_MAX_OFFLINE_CER: f64 = 25.0;
/// 流式 CER 上限（%）。
const DEFAULT_MAX_STREAMING_CER: f64 = 55.0;
/// RTF 上限（识别耗时 / 音频时长）。1.0 = 不慢于实时。
const DEFAULT_MAX_RTF: f64 = 1.0;

fn offline_model_dir() -> Option<PathBuf> {
    std::env::var("VOICE_NOTES_MODEL_DIR").ok().map(PathBuf::from)
}

fn streaming_model_dir() -> Option<PathBuf> {
    std::env::var("VOICE_NOTES_STREAMING_MODEL_DIR")
        .ok()
        .map(PathBuf::from)
}

/// 整段模型路径（跟 `models.rs` 的资产名保持一致，别自己写文件名）。
fn offline_paths(dir: &Path) -> Option<ModelPaths> {
    let model = [dir.join("model.int8.onnx"), dir.join("model.onnx")]
        .into_iter()
        .find(|p| p.is_file())?;
    let tokens = dir.join("tokens.txt");
    if !tokens.is_file() {
        return None;
    }
    Some(ModelPaths {
        dir: dir.display().to_string(),
        model: model.display().to_string(),
        tokens: tokens.display().to_string(),
        vad: Some(dir.join("silero_vad.onnx").display().to_string()),
        source: "gate".into(),
    })
}

/// 流式模型路径（文件名取自 `models.rs` 的常量）。
fn streaming_paths(dir: &Path) -> Option<StreamingPaths> {
    use voice_notes_lib::models::{STREAM_DECODER, STREAM_ENCODER, STREAM_JOINER, STREAM_TOKENS};
    let encoder = dir.join(STREAM_ENCODER.name);
    let decoder = dir.join(STREAM_DECODER.name);
    let joiner = dir.join(STREAM_JOINER.name);
    let tokens = dir.join(STREAM_TOKENS.name);
    if !encoder.is_file() || !decoder.is_file() || !joiner.is_file() || !tokens.is_file() {
        return None;
    }
    Some(StreamingPaths {
        dir: dir.display().to_string(),
        encoder: encoder.display().to_string(),
        decoder: decoder.display().to_string(),
        joiner: joiner.display().to_string(),
        tokens: tokens.display().to_string(),
        vad: Some(dir.join("silero_vad.onnx").display().to_string()),
    })
}

/// 一条语料的测量结果。
struct Measurement {
    name: String,
    category: String,
    /// (编辑距离, 参考文本字数)；没有参考文本时是 None。
    error: Option<(usize, usize)>,
    /// 识别耗时 / 音频时长。
    rtf: f64,
    text: String,
}

/// 整段生产路径：VAD 切句 → 短句合并 → 识别。
fn measure_offline(engine: &Arc<Engine>, file: &Path) -> Result<Measurement, String> {
    let wave = sherpa_onnx::Wave::read(file.to_str().unwrap())
        .ok_or_else(|| format!("读不了音频：{}", file.display()))?;
    let samples = wave.samples();
    let duration = samples.len() as f32 / 16_000.0;
    let vad = engine.vad_config().ok_or("没有 VAD 模型")?;
    let mut session = OfflineSession::new(engine.clone(), vad)?;
    session.push(samples);
    session.finish();
    let transcript = session.transcript(duration);
    let name = file.file_name().unwrap().to_string_lossy().to_string();
    let error = reference_for(file).map(|reference| cer(&transcript.text, &reference));
    let rtf = transcript.elapsed_ms as f64 / 1000.0 / duration.max(f32::EPSILON) as f64;
    Ok(Measurement {
        category: category(&name).to_string(),
        name,
        error,
        rtf,
        text: transcript.text.replace('\n', " "),
    })
}

/// 流式生产路径：Zipformer + 尾部补静音。
fn measure_streaming(engine: &Arc<StreamingEngine>, file: &Path) -> Result<Measurement, String> {
    let wave = sherpa_onnx::Wave::read(file.to_str().unwrap())
        .ok_or_else(|| format!("读不了音频：{}", file.display()))?;
    let samples = wave.samples();
    let mut session = StreamingSession::new(engine.clone(), None);
    session.push(samples);
    session.finish();
    let transcript = session.transcript();
    let name = file.file_name().unwrap().to_string_lossy().to_string();
    let error = reference_for(file).map(|reference| cer(&transcript.text, &reference));
    let duration = samples.len() as f32 / 16_000.0;
    let rtf = transcript.elapsed_ms as f64 / 1000.0 / duration.max(f32::EPSILON) as f64;
    Ok(Measurement {
        category: category(&name).to_string(),
        name,
        error,
        rtf,
        text: transcript.text.replace('\n', " "),
    })
}

fn print_one(label: &str, m: &Measurement) {
    let cer_text = match m.error {
        Some((dist, chars)) => format!("CER {dist}/{chars}={:.1}%", ratio(dist, chars) * 100.0),
        None => "（无参考文本）".to_string(),
    };
    println!(
        "[{label} {:>6}] {:<18} RTF {:.3}  {cer_text} → {}",
        m.category, m.name, m.rtf, m.text
    );
}

fn ratio(dist: usize, chars: usize) -> f64 {
    dist as f64 / chars.max(1) as f64
}

/// 汇总一条路径的结果，打印全部、只对 `tts` 设阈值。
fn summarize(label: &str, measurements: &[Measurement], max_cer: f64, max_rtf: f64) {
    let mut tts_dist = 0usize;
    let mut tts_chars = 0usize;
    let mut tts_files = 0usize;
    let mut worst_rtf = 0.0f64;
    for m in measurements {
        print_one(label, m);
        if let Some((dist, chars)) = m.error {
            if m.category == "tts" {
                tts_dist += dist;
                tts_chars += chars;
                tts_files += 1;
            }
        }
        worst_rtf = worst_rtf.max(m.rtf);
    }
    if tts_files == 0 {
        println!("{label}: 没有带参考文本的 TTS 语料，跳过阈值判定");
        return;
    }
    let cer = ratio(tts_dist, tts_chars) * 100.0;
    println!(
        "\n=== {label} 汇总 ===\n  tts: {tts_files} 条, CER {tts_dist}/{tts_chars} = {cer:.2}%（上限 {max_cer:.2}%）",
    );
    println!("  最差 RTF {worst_rtf:.3}（上限 {max_rtf:.3}）");
    assert!(
        cer <= max_cer,
        "{label} 的 CER 退化了：{cer:.2}% > 上限 {max_cer:.2}%（TTS 语料 {tts_files} 条）",
    );
    assert!(
        worst_rtf <= max_rtf,
        "{label} 的实时性退化了：最差 RTF {worst_rtf:.3} > 上限 {max_rtf:.3}",
    );
}

/// 整段：CER 与 RTF 都不得退化。
#[test]
fn gate_offline_quality_does_not_regress() {
    let Some(dir) = offline_model_dir() else {
        eprintln!("跳过：未设置 VOICE_NOTES_MODEL_DIR");
        return;
    };
    let Some(paths) = offline_paths(&dir) else {
        eprintln!("跳过：{dir:?} 里没有整段模型");
        return;
    };
    let tts = corpus("tts-");
    if tts.is_empty() {
        eprintln!("跳过：没有 TTS 语料（先跑 scripts/gen-tts-corpus.sh）");
        return;
    }
    let engine = Arc::new(Engine::new(&paths, "zh", true).expect("加载整段模型"));
    let mut measurements = Vec::new();
    for (_, file) in tts {
        measurements.push(measure_offline(&engine, &file).expect("整段识别"));
    }
    // 真人语料只打印，不设阈值（抓下来的样本每次可能不一样）。
    for (_, file) in corpus("real-") {
        measurements.push(measure_offline(&engine, &file).expect("整段识别"));
    }
    summarize(
        "整段",
        &measurements,
        env_f64("VOICE_NOTES_MAX_OFFLINE_CER", DEFAULT_MAX_OFFLINE_CER),
        env_f64("VOICE_NOTES_MAX_RTF", DEFAULT_MAX_RTF),
    );
}

/// 流式：CER 与 RTF 都不得退化。
#[test]
fn gate_streaming_quality_does_not_regress() {
    let Some(dir) = streaming_model_dir() else {
        eprintln!("跳过：未设置 VOICE_NOTES_STREAMING_MODEL_DIR");
        return;
    };
    let Some(paths) = streaming_paths(&dir) else {
        eprintln!("跳过：{dir:?} 里没有流式模型");
        return;
    };
    let tts = corpus("tts-");
    if tts.is_empty() {
        eprintln!("跳过：没有 TTS 语料（先跑 scripts/gen-tts-corpus.sh）");
        return;
    }
    let engine = Arc::new(StreamingEngine::new(&paths, &[]).expect("加载流式模型"));
    let mut measurements = Vec::new();
    for (_, file) in tts {
        measurements.push(measure_streaming(&engine, &file).expect("流式识别"));
    }
    for (_, file) in corpus("real-") {
        measurements.push(measure_streaming(&engine, &file).expect("流式识别"));
    }
    summarize(
        "流式",
        &measurements,
        env_f64("VOICE_NOTES_MAX_STREAMING_CER", DEFAULT_MAX_STREAMING_CER),
        env_f64("VOICE_NOTES_MAX_RTF", DEFAULT_MAX_RTF),
    );
}

/// 语料与参考文本要配套：没有 `.txt` 就没法算 CER（只能人工看输出）。
#[test]
fn gate_corpus_has_reference_text() {
    let tts = corpus("tts-");
    if tts.is_empty() {
        eprintln!("跳过：没有 TTS 语料（先跑 scripts/gen-tts-corpus.sh）");
        return;
    }
    let missing: Vec<String> = tts
        .iter()
        .filter(|(_, file)| reference_for(file).is_none())
        .map(|(name, _)| name.clone())
        .collect();
    assert!(
        missing.is_empty(),
        "这些 TTS 语料没有参考文本，没法算 CER：{}",
        missing.join("、"),
    );
}
