//! 探针：内存**不随录音时长增长**。
//!
//! 背景：应用宣称「不限时长、内存恒定」—— 采集缓冲是消费式的
//! （`audio::SampleQueue` 的 `take_new()` 取走即释放），识别会话边跑边扔。
//! 但这条只有「队列语义」的单测守着（`audio/tests.rs`），**端到端的内存曲线没人量过**。
//!
//! 量的方式：**一个会话**跑完一整段录音（生产形态），在两个时间点看**峰值内存**
//! （`getrusage` 的 `ru_maxrss`，单调不减）：
//!
//!   - 300 秒：已经过了预热（模型、ONNX arena、VAD 缓冲都到顶了），此时是平台期
//!   - 1800 秒（30 分钟）：平台期之后又跑了 25 分钟
//!
//! 判据：这两点之间的峰值增长必须远小于这 25 分钟音频本身的体积
//! （1500 秒 × 16000 × 4 字节 ≈ 96 MB）。要是实现把音频攒在内存里，
//! 这 96 MB 会直接长在峰值上 —— 门禁就是抓这个。
//!
//! 实测（2026-11-03，本机 Apple Silicon）：300s 829.6 MB → 1800s 829.6 MB，
//! **一点没涨**。开头从 550 MB 跳到 830 MB 是一次性的预热（模型 + arena + 缓冲），
//! 不是随时长增长。
//!
//! 运行（要真模型；**必须单线程**，两个路径的峰值会互相干扰）：
//! ```bash
//! VOICE_NOTES_MODEL_DIR=... VOICE_NOTES_STREAMING_MODEL_DIR=... \
//!   cargo test --test memory_probe -- --nocapture --test-threads=1
//! ```

use std::sync::Arc;

use voice_notes_lib::asr::{OfflineSession, StreamingSession};
use voice_notes_lib::engine::{Engine, StreamingEngine};

mod common;
use common::trim_silence;

/// 峰值内存允许增长的上限（字节）。
///
/// 300s → 1800s 之间是 1500 秒音频 ≈ 96 MB。真把音频攒在内存里的话，
/// 增长会远超这个数；正常增长只有识别出来的文本（几 KB）。
const ALLOWANCE: i64 = 32 * 1024 * 1024;

/// 检查点 A：预热结束（平台期）。
const CHECKPOINT_SECONDS: f32 = 300.0;
/// 检查点 B：整段跑完。
const TOTAL_SECONDS: f32 = 1800.0;

/// 进程的峰值内存（字节）。
///
/// macOS 上 `ru_maxrss` 的单位是字节，Linux 上是 KB —— 换算成字节再比。
///
/// **只有 Unix 有 `getrusage`**（libc 在 Windows 上没有这个函数）——
/// 所以整段按 `cfg` 分开：Windows 上这个探针只编译桩函数，
/// CI 的 `rust-windows` job（每个 PR 都 `cargo check --all-targets`）才编得过。
#[cfg(unix)]
fn peak_rss() -> i64 {
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) };
    assert_eq!(rc, 0, "getrusage 失败");
    if cfg!(target_os = "macos") {
        usage.ru_maxrss as i64
    } else {
        usage.ru_maxrss as i64 * 1024
    }
}

/// 非 Unix 平台：没有峰值内存可读，探针在这里直接跳过。
#[cfg(not(unix))]
fn peak_rss() -> i64 {
    0
}

/// 造一段指定长度的语音（把 zh.wav 首尾静音去掉后重复）。
fn speech_of(seconds: f32) -> Vec<f32> {
    let wave = sherpa_onnx::Wave::read("tests/fixtures/zh.wav").expect("读取音频");
    let speech = trim_silence(wave.samples(), 0.02);
    let want = (seconds * 16000.0) as usize;
    let mut out = Vec::with_capacity(want + speech.len());
    while out.len() < want {
        out.extend_from_slice(&speech);
    }
    out.truncate(want);
    out
}

/// 峰值内存不能跟着音频时长线性涨。
///
/// 整段与流式各量一次：两条生产路径的「常驻」结构不一样
/// （整段有 VAD 缓冲 + 句子合并，流式是常量内存的解码器）。
#[test]
fn probe_memory_does_not_grow_with_duration() {
    if !cfg!(unix) {
        eprintln!("跳过：峰值内存的读法（getrusage）只有 Unix 有");
        return;
    }
    let offline_dir = std::env::var("VOICE_NOTES_MODEL_DIR").ok();
    let streaming_dir = std::env::var("VOICE_NOTES_STREAMING_MODEL_DIR").ok();
    if offline_dir.is_none() && streaming_dir.is_none() {
        eprintln!("跳过：未设置 VOICE_NOTES_MODEL_DIR / VOICE_NOTES_STREAMING_MODEL_DIR");
        return;
    }

    if offline_dir.is_some() {
        let paths = common::model_paths().expect("整段模型");
        let engine = Arc::new(Engine::new(&paths, "zh", true).expect("加载整段模型"));
        let mut session = OfflineSession::new(engine.clone(), engine.vad_config().expect("VAD 模型"))
            .expect("建整段会话");
        let samples = speech_of(TOTAL_SECONDS);
        let mut checkpoint = 0i64;
        for (index, chunk) in samples.chunks(512).enumerate() {
            session.push(chunk);
            let seconds = (index * 512) as f32 / 16000.0;
            if checkpoint == 0 && seconds >= CHECKPOINT_SECONDS {
                checkpoint = peak_rss();
                println!(
                    "[整段] {seconds:.0}s → 峰值 {:.1} MB",
                    checkpoint as f64 / 1048576.0
                );
            }
        }
        session.finish();
        let grown = peak_rss() - checkpoint;
        println!(
            "[整段] {TOTAL_SECONDS:.0}s → 峰值 {:.1} MB（比 {CHECKPOINT_SECONDS:.0}s 时长了 {:.1} MB，上限 {} MB）",
            peak_rss() as f64 / 1048576.0,
            grown as f64 / 1048576.0,
            ALLOWANCE / 1048576,
        );
        assert!(
            grown < ALLOWANCE,
            "整段：从 {CHECKPOINT_SECONDS:.0}s 到 {TOTAL_SECONDS:.0}s，峰值内存长了 {:.1} MB（上限 {} MB）—— 内存随时长增长了",
            grown as f64 / 1048576.0,
            ALLOWANCE / 1048576,
        );
    }

    if let Some(dir) = streaming_dir {
        let Some(paths) = streaming_paths(std::path::Path::new(&dir)) else {
            eprintln!("跳过：{dir:?} 里没有流式模型");
            return;
        };
        let engine = Arc::new(StreamingEngine::new(&paths, &[]).expect("加载流式模型"));
        let mut session = StreamingSession::new(engine.clone(), None);
        let samples = speech_of(TOTAL_SECONDS);
        let mut checkpoint = 0i64;
        for (index, chunk) in samples.chunks(3200).enumerate() {
            session.push(chunk);
            let seconds = (index * 3200) as f32 / 16000.0;
            if checkpoint == 0 && seconds >= CHECKPOINT_SECONDS {
                checkpoint = peak_rss();
                println!(
                    "[流式] {seconds:.0}s → 峰值 {:.1} MB",
                    checkpoint as f64 / 1048576.0
                );
            }
        }
        session.finish();
        let grown = peak_rss() - checkpoint;
        println!(
            "[流式] {TOTAL_SECONDS:.0}s → 峰值 {:.1} MB（比 {CHECKPOINT_SECONDS:.0}s 时长了 {:.1} MB，上限 {} MB）",
            peak_rss() as f64 / 1048576.0,
            grown as f64 / 1048576.0,
            ALLOWANCE / 1048576,
        );
        assert!(
            grown < ALLOWANCE,
            "流式：从 {CHECKPOINT_SECONDS:.0}s 到 {TOTAL_SECONDS:.0}s，峰值内存长了 {:.1} MB（上限 {} MB）—— 内存随时长增长了",
            grown as f64 / 1048576.0,
            ALLOWANCE / 1048576,
        );
    }
}

/// 流式模型路径（文件名取自 `models.rs` 的常量）。
fn streaming_paths(dir: &std::path::Path) -> Option<voice_notes_lib::models::StreamingPaths> {
    use voice_notes_lib::models::{STREAM_DECODER, STREAM_ENCODER, STREAM_JOINER, STREAM_TOKENS};
    let encoder = dir.join(STREAM_ENCODER.name);
    let decoder = dir.join(STREAM_DECODER.name);
    let joiner = dir.join(STREAM_JOINER.name);
    let tokens = dir.join(STREAM_TOKENS.name);
    if !encoder.is_file() || !decoder.is_file() || !joiner.is_file() || !tokens.is_file() {
        return None;
    }
    Some(voice_notes_lib::models::StreamingPaths {
        dir: dir.display().to_string(),
        encoder: encoder.display().to_string(),
        decoder: decoder.display().to_string(),
        joiner: joiner.display().to_string(),
        tokens: tokens.display().to_string(),
        vad: Some(dir.join("silero_vad.onnx").display().to_string()),
    })
}
