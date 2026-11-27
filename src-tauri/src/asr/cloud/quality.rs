//! 云端质量门禁：用已知参考文本的 TTS 语料，量**生产路径**认得多准。
//!
//! 与协议层单测的分工：
//! - 协议单测守「帧解得对不对」「映射对不对」—— 输入是样例帧。
//! - 这里守「认得准不准」—— 输入是真实音频，走 `CloudStreamingSession`
//!   （建连 → 20 ms 分批喂 → 收定稿 → 收尾），算 CER。
//!
//! 与 `src-tauri/tests/quality_gate.rs` 的分工：那个量本地两条路径，走集成测试
//! （只能碰 `pub` 项）；云端 provider 在 `pub(crate) mod cloud` 里，集成测试够不着，
//! 所以门禁放在 `--lib` 内（见 OpenSpec change `2026-11-28-cloud-quality-baseline`）。
//!
//! **整个模块只在测试构建里编译**（`cloud.rs` 里是 `#[cfg(test)] mod quality;`），
//! 不进 release 二进制。
//!
//! 语料（不入库，本机生成）：
//!
//! ```bash
//! bash scripts/gen-tts-corpus.sh                       # → src-tauri/tests/fixtures/quality/
//! export VOICE_NOTES_VOLCENGINE_KEY=$(…)               # 见 docs/HOW_TO_VERIFY.md
//! export VOICE_NOTES_ALIYUN_KEY=$(…)
//! cd src-tauri
//! cargo test --lib cloud_quality -- --nocapture --test-threads=1
//! ```
//!
//! 凭据只从环境变量读，没设就跳过；key 不进日志、不进断言消息。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::asr::cloud::aliyun::{AliyunConfig, AliyunProvider};
use crate::asr::cloud::volcengine::{VolcengineConfig, VolcengineProvider};
use crate::asr::{AsrSession, StreamingSession, Transcript};
use crate::engine::StreamingEngine;
use crate::models::{StreamingPaths, STREAM_DECODER, STREAM_ENCODER, STREAM_JOINER, STREAM_TOKENS};

use super::CloudStreamingSession;

/// 语料目录（相对 crate 根，与 `tests/common/mod.rs` 的 `QUALITY_DIR` 同一口径）。
pub const QUALITY_DIR: &str = "tests/fixtures/quality";

/// 门禁默认每批喂 20 ms @ 16 kHz。
///
/// **生产路径不是这个数**：`record.rs` 的云端循环每 60 ms 从采集队列取一次，
/// 所以每批约 60 ms（960 采样）。门禁用 20 ms 是为了让 `poll` 更密；
/// **它不决定线上帧长** —— provider 自己会重新攒包（火山 200 ms、阿里 100 ms，
/// 见各自的 `AUDIO_PACKET_BYTES`）。实测把 320 改成 3200 结果几乎不变。
/// `VOICE_NOTES_CLOUD_CHUNK` 可覆盖（诊断用）。
pub const CHUNK: usize = 320;

/// 本批喂入的采样数（`VOICE_NOTES_CLOUD_CHUNK` 可覆盖）。
fn chunk_size() -> usize {
    env_f64("VOICE_NOTES_CLOUD_CHUNK", CHUNK as f64).max(1.0) as usize
}

/// 收尾后等迟到的 final：最长 5 s，静默 1 s 就停。
const LATE_FINAL_MAX: Duration = Duration::from_secs(5);
const LATE_FINAL_QUIET: Duration = Duration::from_secs(1);

/// 火山在 TTS 语料上的聚合 CER 上限（%）。
///
/// **不是质量目标，是退化告警线**。2026-11-28 实测 54.20%（TTS 20 条，
/// 见 `docs/HOW_TO_VERIFY.md`）—— `say` 合成音对豆包流式模型是域外：
/// 同一条 tts-05 本地整段模型能认出全句，火山只出后半句。阈值 = 实测 + 10pp
/// （向上取整到 5%）。`VOICE_NOTES_MAX_CLOUD_CER` 可覆盖。
pub const DEFAULT_VOLCENGINE_MAX_CER: f64 = 65.0;

/// 阿里在 TTS 语料上的聚合 CER 上限（%）。
///
/// 同上，实测 6.30% / 7.14%（跑两轮）→ 阈值 20%（+10pp 向上取整）。两家的差距是模型差异，
/// 不是门禁写错了 —— 所以**一家一个上限**，不共用。
pub const DEFAULT_ALIYUN_MAX_CER: f64 = 20.0;

/// 真人 fixture 上的 CER 上限（%）。
///
/// `tests/fixtures/zh.wav`（入库的单句真人语音，live_smoke 用的也是它）：
/// 2026-11-28 实测两家都是 0%。这一条是门禁的「牙齿」—— TTS 域外会把阈值
/// 撑得很松，真人语音上的退化必须能被抓住。
pub const MAX_HUMAN_CER: f64 = 25.0;

/// 真人 fixture 参考文本（`zh.wav` 的内容，与 `live_smoke` / 文档同一句）。
pub const HUMAN_REFERENCE: &str = "开放时间，早上9点至下午5点";

/// 入库的真人单句 fixture（16 kHz / 5.592 s）。
pub const HUMAN_FIXTURE: &str = "tests/fixtures/zh.wav";

/// 「云端不得比本地流式基线差太多」的余量（百分点）。
///
/// 本地流式在 TTS 上是域外（基线 43.28%），这条只是兜底 —— 云端正常应显著更好，
/// 比本地还差 20 个点只可能是云端整体崩了。
pub const LOCAL_BASELINE_MARGIN: f64 = 20.0;

/// 最后一句的本地轴终点与音频末点的允许偏差（秒）。
///
/// 服务端断句族的已知取舍是「尾巴静音整段算进最后一句」—— 补 1.5 s 静音后
/// 本地轴终点会顶到音频末点（与音频末点差 0）。这里钉的是**上界**：
/// 偏移不许从 1.3~1.7 s 量级继续涨过 3 s。`docs/CLOUD.md` 里 1.28 s / 1.71 s
/// 那两个数字是「服务端给的 end vs 本地轴 end」，生产路径不读服务端时间戳，
/// 所以门禁复现不了它们 —— 只能钉住本地轴这一侧（见 change 的 design D4）。
pub const ENDPOINT_SLACK_SECONDS: f32 = 3.0;

/// 末尾补的静音（秒）：复现 `docs/CLOUD.md` 那张表的测试条件。
pub const ENDPOINT_TAIL_SILENCE_SECONDS: f32 = 1.5;

/// CER 测量时句尾补的静音（毫秒）。
///
/// `say` 产出的 wav 说完就结束、尾部几乎没有静音；服务端断句族（火山）靠静音
/// 收句，音频在语音中间截断时经常一个定稿都不回（实测 2026-11-28）。生产里
/// 用户说完会继续上传静音，所以这里补 1 s，让两家的收句条件一致。
/// `VOICE_NOTES_CLOUD_TAIL_PAD_MS` 可覆盖。
pub const DEFAULT_TAIL_PAD_MS: f64 = 1_000.0;

/// 一条带参考文本的 TTS 语料。
pub struct CorpusEntry {
    pub name: String,
    pub wav: PathBuf,
    pub reference: String,
}

/// TTS 语料（`tts-*.wav` + 同名 `.txt`）。
///
/// 没有 `.txt` 的条目跳过（算不了 CER）；目录不存在时返回空。
pub fn corpus() -> Vec<CorpusEntry> {
    let dir = Path::new(QUALITY_DIR);
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("wav"))
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .map(|name| name.starts_with("tts-"))
                .unwrap_or(false)
        })
        .collect();
    files.sort();
    files
        .into_iter()
        .filter_map(|wav| {
            let reference = std::fs::read_to_string(wav.with_extension("txt"))
                .ok()
                .map(|text| text.trim().to_string())
                .filter(|text| !text.is_empty())?;
            let name = wav.file_name()?.to_string_lossy().to_string();
            Some(CorpusEntry {
                name,
                wav,
                reference,
            })
        })
        .collect()
}

/// 读 16 kHz 单声道 wav 的采样。
pub fn read_wave(path: &Path) -> Result<Vec<f32>, String> {
    let shown = path.display();
    let Some(text) = path.to_str() else {
        return Err(format!("路径不是 UTF-8：{shown}"));
    };
    let wave = sherpa_onnx::Wave::read(text).ok_or_else(|| format!("读不了音频：{shown}"))?;
    if wave.sample_rate() != 16_000 {
        return Err(format!(
            "{} 的采样率是 {}，门禁只收 16 kHz",
            path.display(),
            wave.sample_rate()
        ));
    }
    Ok(wave.samples().to_vec())
}

/// 一次测量的结果。
pub struct Measurement {
    pub name: String,
    /// (编辑距离, 参考文本字数)
    pub error: (usize, usize),
    pub text: String,
    /// 最后一句的本地轴终点（秒）；没有句子时是 None。
    pub last_end: Option<f32>,
    /// 音频末点（秒，16 kHz 采样数）。
    pub audio_end: f32,
    /// 会话层收回的错误（字典码）。
    pub warning: Option<String>,
}

impl Measurement {
    pub fn cer_percent(&self) -> f64 {
        ratio(self.error.0, self.error.1) * 100.0
    }
}

fn ratio(dist: usize, chars: usize) -> f64 {
    dist as f64 / chars.max(1) as f64
}

/// 跑完一条语料的生产路径。
///
/// `samples` 必须是 16 kHz。收尾后继续 `poll` 一会儿，把迟到的 final 收干净
/// —— 与 `volcengine::tests::live_smoke` 同一套等待口径。
pub fn run_session(
    provider: Box<dyn super::CloudProvider>,
    samples: &[f32],
) -> Result<Transcript, String> {
    let mut session = CloudStreamingSession::new(provider);
    session.connect()?;
    for part in samples.chunks(chunk_size()) {
        session.advance_samples(part.len());
        // 事件本身不重要：会话内部已经把定稿句子记进 `segments`，
        // `transcript()` 会按本地轴排好序返回。这里只要把通道抽干。
        drop(session.push(part));
        drop(session.poll());
    }
    // 客户端提交族靠它提交最后一句；服务端断句族只登记区间，无副作用。
    drop(session.force_end());
    drop(session.finish());
    wait_for_late_finals(&mut session);
    Ok(session.transcript())
}

/// 收尾后等迟到的 final：最多 5 s，静默 1 s 就停。
fn wait_for_late_finals(session: &mut CloudStreamingSession) {
    let started = Instant::now();
    let mut last_event = Instant::now();
    while started.elapsed() < LATE_FINAL_MAX && last_event.elapsed() < LATE_FINAL_QUIET {
        let events = session.poll();
        if !events.is_empty() {
            last_event = Instant::now();
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// 量一条语料。
pub fn measure(
    provider: Box<dyn super::CloudProvider>,
    entry: &CorpusEntry,
) -> Result<Measurement, String> {
    let mut samples = read_wave(&entry.wav)?;
    // 句尾补一段静音：生产里用户说完也会继续有静音上传，服务端断句族靠它收句。
    // 不补的话音频在语音中间截断，火山经常一个定稿都不回（实测）。
    let pad = (env_f64("VOICE_NOTES_CLOUD_TAIL_PAD_MS", DEFAULT_TAIL_PAD_MS) / 1000.0 * 16_000.0)
        as usize;
    samples.extend_from_slice(&vec![0.0f32; pad]);
    let audio_end = samples.len() as f32 / 16_000.0;
    let transcript = run_session(provider, &samples)?;
    Ok(Measurement {
        name: entry.name.clone(),
        error: cer(&transcript.text, &entry.reference),
        last_end: transcript.segments.last().map(|segment| segment.end),
        audio_end,
        text: transcript.text.replace('\n', " "),
        warning: transcript.warning,
    })
}

/// 量入库的真人 fixture（`tests/fixtures/zh.wav`）。
///
/// 不补尾静音 —— 这个 fixture 本身尾部有 0.49 s 静音，与 `live_smoke` 口径一致。
pub fn measure_human(provider: Box<dyn super::CloudProvider>) -> Result<Measurement, String> {
    let samples = read_wave(Path::new(HUMAN_FIXTURE))?;
    let audio_end = samples.len() as f32 / 16_000.0;
    let transcript = run_session(provider, &samples)?;
    Ok(Measurement {
        name: "zh.wav".into(),
        error: cer(&transcript.text, HUMAN_REFERENCE),
        last_end: transcript.segments.last().map(|segment| segment.end),
        audio_end,
        text: transcript.text.replace('\n', " "),
        warning: transcript.warning,
    })
}

/// 聚合 CER（总编辑距离 / 总参考字数）。
pub fn aggregate(measurements: &[Measurement]) -> (usize, usize) {
    measurements.iter().fold((0, 0), |(dist, chars), m| {
        (dist + m.error.0, chars + m.error.1)
    })
}

/// 打印一条测量。
pub fn print_one(provider: &str, m: &Measurement) {
    let offset = m
        .last_end
        .map(|end| {
            format!(
                "终点 {:.2}s / 音频 {:.2}s（差 {:+.2}s）",
                end,
                m.audio_end,
                end - m.audio_end
            )
        })
        .unwrap_or_else(|| format!("终点 — / 音频 {:.2}s（没有句子）", m.audio_end));
    let warning = m
        .warning
        .as_ref()
        .map(|w| format!("  警告 {w}"))
        .unwrap_or_default();
    println!(
        "[{provider} {}] CER {}/{}={:.1}%  {offset}{warning} → {}",
        m.name,
        m.error.0,
        m.error.1,
        m.cer_percent(),
        m.text
    );
}

/// 聚合结果的判定。
pub struct Verdict {
    pub cer: f64,
    pub max_cer: f64,
    pub local_cer: Option<f64>,
}

impl Verdict {
    pub fn pass(&self) -> bool {
        self.cer <= self.max_cer
            && self
                .local_cer
                .map(|local| self.cer <= local + LOCAL_BASELINE_MARGIN)
                .unwrap_or(true)
    }
}

/// 汇总并判定一家 provider。`default_max_cer` 是该家的实测基线 + 余量。
pub fn summarize(
    provider: &str,
    measurements: &[Measurement],
    local_cer: Option<f64>,
    default_max_cer: f64,
) -> Verdict {
    let (dist, chars) = aggregate(measurements);
    let cer = ratio(dist, chars) * 100.0;
    let max_cer = env_f64("VOICE_NOTES_MAX_CLOUD_CER", default_max_cer);
    println!(
        "\n=== {provider} 汇总 ===\n  {} 条，CER {dist}/{chars} = {cer:.2}%（上限 {max_cer:.2}%）",
        measurements.len(),
    );
    match local_cer {
        Some(local) => println!(
            "  本地流式基线 {local:.2}%，云端相对上限 {:.2}%（基线 + {LOCAL_BASELINE_MARGIN:.0}pp）",
            local + LOCAL_BASELINE_MARGIN,
        ),
        None => println!("  本地流式基线不可用（未设 VOICE_NOTES_STREAMING_MODEL_DIR 或没有模型），相对断言跳过"),
    }
    Verdict {
        cer,
        max_cer,
        local_cer,
    }
}

/// 这一家的关键配置：端点、模型/资源标识都留空 → 用 provider 默认值。
pub fn volcengine_provider(api_key: String) -> Box<dyn super::CloudProvider> {
    Box::new(VolcengineProvider::new(VolcengineConfig {
        base_url: String::new(),
        api_key,
        resource_id: String::new(),
        keywords: Vec::new(),
        itn: true,
        punct: true,
    }))
}

/// 同上，阿里。语言与术语表都不传 —— 与 `live_smoke` 的默认口径一致。
pub fn aliyun_provider(api_key: String) -> Box<dyn super::CloudProvider> {
    Box::new(AliyunProvider::new(AliyunConfig {
        base_url: String::new(),
        api_key,
        model: String::new(),
        language_hint: None,
        vocabulary: Vec::new(),
    }))
}

/// 本地流式基线（同一批语料），用来判「云端不得比本地差太多」。
///
/// 没设 `VOICE_NOTES_STREAMING_MODEL_DIR`、或目录里没有模型时返回 None
/// （那条相对断言就跳过，绝对上限照判）。
pub fn local_streaming_baseline(entries: &[CorpusEntry]) -> Option<f64> {
    let dir = std::env::var("VOICE_NOTES_STREAMING_MODEL_DIR")
        .ok()
        .map(PathBuf::from)?;
    let paths = streaming_paths(&dir)?;
    let engine = Arc::new(StreamingEngine::new(&paths, &[]).ok()?);
    let mut dist = 0usize;
    let mut chars = 0usize;
    for entry in entries {
        let samples = read_wave(&entry.wav).ok()?;
        let mut session = StreamingSession::new(engine.clone(), None);
        session.advance_samples(samples.len());
        drop(session.push(&samples));
        drop(session.finish());
        let transcript = session.transcript();
        let (d, c) = cer(&transcript.text, &entry.reference);
        dist += d;
        chars += c;
    }
    Some(ratio(dist, chars) * 100.0)
}

/// 流式模型路径（文件名取自 `models.rs` 的常量，别自己写）。
fn streaming_paths(dir: &Path) -> Option<StreamingPaths> {
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
        vad: None,
    })
}

/// 读环境变量里的浮点数（拿不到就用默认值）。
pub fn env_f64(name: &str, default: f64) -> f64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<f64>().ok())
        .unwrap_or(default)
}

// ── 文本比较 ────────────────────────────────────────────────────────
//
// 最小实现，复制自 `src-tauri/tests/common/mod.rs`（集成测试 crate 里的
// `norm` / `edit_distance` / `cer`）。`--lib` 够不着那边的模块，所以这里
// 保留一份；两边算法必须一致，改动时同步。

/// 归一化文本：去掉空白与标点，只留可比对的字符。
pub fn norm(text: &str) -> String {
    text.chars()
        .filter(|c| {
            !c.is_whitespace()
                && !c.is_ascii_punctuation()
                && !"，。！？、；：“”‘’（）《》…—～·「」".contains(*c)
        })
        .collect()
}

/// 字符级编辑距离（纯 Levenshtein）。
pub fn edit_distance(a: &[char], b: &[char]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for i in 1..=a.len() {
        cur[0] = i;
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// CER（字符错误率）：编辑距离 / 参考文本长度。
pub fn cer(hypothesis: &str, reference: &str) -> (usize, usize) {
    let hyp: Vec<char> = norm(hypothesis).chars().collect();
    let reference: Vec<char> = norm(reference).chars().collect();
    (edit_distance(&hyp, &reference), reference.len())
}

#[cfg(test)]
mod tests;
