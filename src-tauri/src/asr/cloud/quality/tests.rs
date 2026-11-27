//! 云端质量门禁：两条要真 key（质量 / 终点），两条是不联网的纯函数守卫。
//!
//! 跑法（需要真 key，**不进 CI**）：
//!
//! ```bash
//! export VOICE_NOTES_VOLCENGINE_KEY=$(…)
//! export VOICE_NOTES_ALIYUN_KEY=$(…)
//! cd src-tauri
//! cargo test --lib cloud_quality -- --nocapture --test-threads=1
//! cargo test --lib last_sentence_end -- --nocapture --test-threads=1
//! ```
//!
//! 没设 key / 没语料时**跳过**（打印「跳过：…」），不是失败 —— 那是「没验」，
//! 不是「过了」。key 只从环境变量读，不打印。

use super::*;
use crate::asr::cloud::CloudProvider;

const VOLCENGINE_KEY: &str = "VOICE_NOTES_VOLCENGINE_KEY";
const ALIYUN_KEY: &str = "VOICE_NOTES_ALIYUN_KEY";

type ProviderFactory = fn(String) -> Box<dyn CloudProvider>;

/// 一家 provider 的完整测量：TTS 聚合 + 真人 fixture。
struct ProviderRun {
    verdict: Verdict,
    human: Measurement,
}

/// 跑一家 provider 的全部语料（TTS + 真人）并判定。
fn run_provider(
    label: &str,
    make: ProviderFactory,
    api_key: String,
    entries: &[CorpusEntry],
    local_cer: Option<f64>,
    default_max_cer: f64,
) -> ProviderRun {
    let mut measurements = Vec::new();
    for entry in entries {
        match measure(make(api_key.clone()), entry) {
            Ok(measurement) => {
                print_one(label, &measurement);
                measurements.push(measurement);
            }
            Err(error) => panic!("{label} 的 {} 跑失败：{error}", entry.name),
        }
    }
    let verdict = summarize(label, &measurements, local_cer, default_max_cer);
    let human = measure_human(make(api_key)).unwrap_or_else(|error| {
        panic!("{label} 跑真人 fixture 失败：{error}");
    });
    print_one(&format!("{label} 真人"), &human);
    println!(
        "  → {}",
        if verdict.pass() && human.cer_percent() <= MAX_HUMAN_CER {
            "通过"
        } else {
            "未通过（见上方上限）"
        }
    );
    ProviderRun { verdict, human }
}

/// 云端质量不得整体退化：聚合 CER 与终点偏移都要在阈值内。
///
/// 阈值口径见 `quality.rs` 的常量与 change 的 design D3。
#[test]
fn cloud_quality_does_not_regress() {
    let entries = corpus();
    if entries.is_empty() {
        println!("跳过：没有带参考文本的 TTS 语料（先跑 scripts/gen-tts-corpus.sh）");
        return;
    }
    let local_cer = local_streaming_baseline(&entries);
    match local_cer {
        Some(local) => println!("本地流式基线（同一批 {} 条）：{local:.2}%", entries.len()),
        None => println!(
            "本地流式基线不可用（未设 VOICE_NOTES_STREAMING_MODEL_DIR 或目录里没有流式模型），相对断言跳过"
        ),
    }

    let mut runs: Vec<(&str, ProviderRun)> = Vec::new();
    if let Ok(key) = std::env::var(VOLCENGINE_KEY) {
        runs.push((
            "volcengine",
            run_provider(
                "volcengine",
                volcengine_provider,
                key,
                &entries,
                local_cer,
                DEFAULT_VOLCENGINE_MAX_CER,
            ),
        ));
    } else {
        println!("跳过：volcengine —— 未设置 {VOLCENGINE_KEY}");
    }
    if let Ok(key) = std::env::var(ALIYUN_KEY) {
        runs.push((
            "aliyun",
            run_provider(
                "aliyun",
                aliyun_provider,
                key,
                &entries,
                local_cer,
                DEFAULT_ALIYUN_MAX_CER,
            ),
        ));
    } else {
        println!("跳过：aliyun —— 未设置 {ALIYUN_KEY}");
    }

    if runs.is_empty() {
        println!("跳过：两家都没有凭据环境变量，未对真实服务验证");
        return;
    }

    let mut failures: Vec<String> = Vec::new();
    for (label, run) in &runs {
        if !run.verdict.pass() {
            let relative = run
                .verdict
                .local_cer
                .map(|local| format!("，本地基线 {local:.2}%"))
                .unwrap_or_default();
            failures.push(format!(
                "{label} TTS 聚合 CER {:.2}%（上限 {:.2}%{relative}）",
                run.verdict.cer, run.verdict.max_cer
            ));
        }
        if run.human.cer_percent() > MAX_HUMAN_CER {
            failures.push(format!(
                "{label} 真人 fixture CER {:.2}%（上限 {MAX_HUMAN_CER:.2}%）",
                run.human.cer_percent()
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "云端识别质量退化了 —— {}",
        failures.join("；")
    );
}

/// 最后一句的**本地轴终点**不能被截断 / 漂走（上界 3 s）。
///
/// 测试条件对齐 `docs/CLOUD.md` 那张表：第一条 TTS 语料 + 1.5 s 尾静音。
/// 服务端断句族把尾巴静音整段算进最后一句 → 本地轴终点 = 音频末点
/// （实测两家都是 0.00 s）。
///
/// **这条守的是「本地 16 kHz 时钟」这一侧，不是 provider 侧的终点偏长。**
/// `CLOUD.md` 表里的 1.28 s / 1.71 s 是「服务端给的 `end` vs 本地轴终点」；
/// 生产路径刻意不读 provider 时间戳（`CloudEvent::Final` 的 `start_ms` /
/// `end_ms` 两家都填 `None`），所以本断言**发现不了那件事** —— 要断言它，
/// 得先把 provider 时间戳接出来（见 change `2026-11-28-cloud-quality-baseline`
/// 的 design D4）。
///
/// **mutation 自证**（2026-11-28 实测，已恢复）：把 `force_end()` 里的
/// `commit_at(self.fed_end, …)` 改成 `commit_at(self.fed_end - 4 * 16_000, …)`
/// → 本用例红（最后一句 0.00~0.57 s、音频末点 4.57 s、差 -4.00 s）。
/// 也就是说它抓的是「终点跟踪丢了 ≥ 3 s」这类整体性错误；
/// **小于 3 s 的截断它抓不住**（3 s 是约定的上界，不是精度）。
#[test]
fn last_sentence_end_tracks_the_local_audio_clock() {
    let entries = corpus();
    if entries.is_empty() {
        println!("跳过：没有带参考文本的 TTS 语料（先跑 scripts/gen-tts-corpus.sh）");
        return;
    }
    let entry = &entries[0];
    let mut samples = match read_wave(&entry.wav) {
        Ok(samples) => samples,
        Err(error) => {
            println!("跳过：{error}");
            return;
        }
    };
    let tail = (ENDPOINT_TAIL_SILENCE_SECONDS * 16_000.0) as usize;
    samples.extend_from_slice(&vec![0.0f32; tail]);
    let audio_end = samples.len() as f32 / 16_000.0;
    println!(
        "终点断言语料：{} + {ENDPOINT_TAIL_SILENCE_SECONDS:.1}s 静音，音频末点 {audio_end:.2}s",
        entry.name
    );

    let mut checked = 0usize;
    let mut failures: Vec<String> = Vec::new();
    if let Ok(key) = std::env::var(VOLCENGINE_KEY) {
        check_endpoint(
            "volcengine",
            volcengine_provider(key),
            &samples,
            audio_end,
            &mut checked,
            &mut failures,
        );
    } else {
        println!("跳过：volcengine —— 未设置 {VOLCENGINE_KEY}");
    }
    if let Ok(key) = std::env::var(ALIYUN_KEY) {
        check_endpoint(
            "aliyun",
            aliyun_provider(key),
            &samples,
            audio_end,
            &mut checked,
            &mut failures,
        );
    } else {
        println!("跳过：aliyun —— 未设置 {ALIYUN_KEY}");
    }

    if checked == 0 {
        println!("跳过：两家都没有凭据环境变量，未对真实服务验证");
        return;
    }
    assert!(
        failures.is_empty(),
        "句子终点偏离音频末点超过 {ENDPOINT_SLACK_SECONDS:.1}s —— {}",
        failures.join("；")
    );
}

fn check_endpoint(
    label: &str,
    provider: Box<dyn CloudProvider>,
    samples: &[f32],
    audio_end: f32,
    checked: &mut usize,
    failures: &mut Vec<String>,
) {
    let transcript = match run_session(provider, samples) {
        Ok(transcript) => transcript,
        Err(error) => {
            failures.push(format!("{label} 连不上：{error}"));
            *checked += 1;
            return;
        }
    };
    let Some(last) = transcript.segments.last() else {
        failures.push(format!("{label} 没有返回任何句子"));
        *checked += 1;
        return;
    };
    let offset = last.end - audio_end;
    println!(
        "[{label} 终点] 最后一句 {:.2}s ~ {:.2}s，音频末点 {audio_end:.2}s，差 {offset:+.2}s（上界 {ENDPOINT_SLACK_SECONDS:.1}s）",
        last.start, last.end
    );
    *checked += 1;
    if offset.abs() > ENDPOINT_SLACK_SECONDS {
        failures.push(format!("{label} 差 {offset:+.2}s"));
    }
}

/// 文本比较与语料枚举的纯函数（不联网，永远跑）。
#[test]
fn quality_helpers_match_the_shared_implementation() {
    // 与 `tests/common/mod.rs` 的 `cer` 同一口径：标点 / 空白不进比较。
    let (dist, chars) = cer("开放时间，早上9点。", "开放时间早上九点");
    assert_eq!(chars, 8, "归一化后参考文本应为 8 个字");
    assert_eq!(dist, 1, "同义写法（9 / 九）是 1 个替换");

    let (dist, chars) = cer("你好，世界！", "你好世界");
    assert_eq!((dist, chars), (0, 4), "标点被归一化掉后应完全一致");
}

/// 语料没有 `.txt` 时会被跳过 —— 门禁不能拿空参考算出一个假的 0%。
#[test]
fn corpus_entries_all_carry_reference_text() {
    let entries = corpus();
    if entries.is_empty() {
        println!("跳过：没有 TTS 语料（先跑 scripts/gen-tts-corpus.sh）");
        return;
    }
    assert!(
        entries
            .iter()
            .all(|entry| !entry.reference.trim().is_empty()),
        "语料条目必须带非空参考文本"
    );
}
