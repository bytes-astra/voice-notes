//! 转写质量探针：把「质量旋钮」在同一批音频上的实际表现打印出来，供人工对比。
//!
//! 只打印、不断言；目的是让任何「换模型 / 调参数 / 加热词」的改动**先量后改**。
//!
//! 语料（仅本机、不入库）：
//! - `tests/fixtures/quality/real-*.wav`：PolyAI/minds14 zh-CN 真人电话录音（附转写）
//! - `tests/fixtures/quality/tts-*.wav`：本机 say 合成（附文本，可算 CER）
//! - `tests/fixtures/quality/noisy-*.wav`：上面部分样本混粉红噪声（≈10 dB）
//! - `tests/fixtures/quality/num-*.wav`：数字/英文句，只观察输出（ITN 会改写法，不算 CER）
//! - `tests/fixtures/quality/long-01.wav`：长连续语音，用来测 rule3 / 短句合并
//!
//! ```bash
//! cd src-tauri
//! VOICE_NOTES_MODEL_DIR=... VOICE_NOTES_STREAMING_MODEL_DIR=... \
//!   cargo test --test quality_probe -- --nocapture --test-threads=1
//! ```
//!
//! 可选环境变量：`VOICE_NOTES_VAD_ALT`（对比 VAD 模型）、`VOICE_NOTES_HOTWORDS`（热词表）、
//! `VOICE_NOTES_RULE3`（端点 rule3 秒数）。

use std::path::{Path, PathBuf};

use sherpa_onnx::{
    OfflineRecognizer, OfflineRecognizerConfig, OfflineSenseVoiceModelConfig, OnlineRecognizer,
    OnlineRecognizerConfig, OnlineTransducerModelConfig,
};
use voice_notes_lib::asr::Engine;
use voice_notes_lib::models::ModelPaths;

fn model_dir() -> Option<PathBuf> {
    std::env::var("VOICE_NOTES_MODEL_DIR").ok().map(PathBuf::from)
}

fn stream_dir() -> Option<PathBuf> {
    std::env::var("VOICE_NOTES_STREAMING_MODEL_DIR")
        .ok()
        .map(PathBuf::from)
}

fn paths(dir: &Path) -> ModelPaths {
    let model = [dir.join("model.int8.onnx"), dir.join("model.onnx")]
        .into_iter()
        .find(|p| p.is_file())
        .expect("找不到 SenseVoice 模型");
    ModelPaths {
        dir: dir.display().to_string(),
        model: model.display().to_string(),
        tokens: dir.join("tokens.txt").display().to_string(),
        vad: Some(dir.join("silero_vad.onnx").display().to_string()),
        source: "probe".into(),
    }
}

fn legacy() -> Vec<(String, PathBuf)> {
    ["zh.wav", "zh-tail.wav", "zh-tail2.wav"]
        .iter()
        .map(|name| (name.to_string(), Path::new("tests/fixtures").join(name)))
        .filter(|(_, p)| p.is_file())
        .collect()
}

/// 质量语料（本机生成/抓取，见 scripts/）。
fn corpus(prefix: &str) -> Vec<(String, PathBuf)> {
    let dir = Path::new("tests/fixtures/quality");
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("wav"))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with(prefix))
                .unwrap_or(false)
        })
        .collect();
    files.sort();
    files
        .into_iter()
        .map(|p| {
            let name = p.file_name().unwrap().to_string_lossy().to_string();
            (name, p)
        })
        .collect()
}

fn all_corpus() -> Vec<(String, PathBuf)> {
    corpus("real-")
        .into_iter()
        .chain(corpus("tts-"))
        .chain(corpus("noisy-"))
        .chain(corpus("long-"))
        .chain(corpus("num-"))
        .collect()
}

fn norm(text: &str) -> String {
    text.chars()
        .filter(|c| {
            !c.is_whitespace()
                && !c.is_ascii_punctuation()
                && !"，。！？、；：“”‘’（）《》…—～·「」".contains(*c)
        })
        .collect()
}

fn edit_distance(a: &[char], b: &[char]) -> usize {
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

fn reference_for(file: &Path) -> Option<String> {
    let txt = file.with_extension("txt");
    std::fs::read_to_string(txt)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn category(name: &str) -> &str {
    if name.starts_with("noisy-") {
        "noisy"
    } else if name.starts_with("real-") {
        "real"
    } else if name.starts_with("tts-") {
        "tts"
    } else if name.starts_with("long-") {
        "long"
    } else if name.starts_with("num-") {
        "num"
    } else {
        "legacy"
    }
}

/// 真模型 + 整段模式跑全语料：逐条 CER + 分类汇总。
#[test]
fn probe_corpus_offline() {
    let Some(dir) = model_dir() else {
        eprintln!("跳过：未设置 VOICE_NOTES_MODEL_DIR");
        return;
    };
    let engine = Engine::new(&paths(&dir), "zh", true).expect("加载模型");
    let files = all_corpus();
    if files.is_empty() {
        eprintln!("跳过：没有语料（先跑 scripts/fetch-quality-corpus.mjs 与 scripts/gen-tts-corpus.sh）");
        return;
    }
    let mut stats: std::collections::BTreeMap<&str, (usize, usize, usize)> = Default::default();
    for (name, file) in &files {
        let wave = sherpa_onnx::Wave::read(file.to_str().unwrap()).expect("读取音频");
        let started = std::time::Instant::now();
        let out = engine.transcribe(wave.samples());
        let ms = started.elapsed().as_millis();
        let cat = category(name);
        let entry = stats.entry(cat).or_insert((0, 0, 0));
        entry.0 += 1;
        let mut cer = String::new();
        if let Some(reference) = reference_for(file) {
            let hyp: Vec<char> = norm(&out.text).chars().collect();
            let refc: Vec<char> = norm(&reference).chars().collect();
            let dist = edit_distance(&hyp, &refc);
            entry.1 += dist;
            entry.2 += refc.len();
            cer = format!(
                "  CER {}/{}={:.1}%",
                dist,
                refc.len(),
                dist as f64 / refc.len().max(1) as f64 * 100.0
            );
        }
        println!(
            "[离线 {cat:>6}] {name:<18} {}段 {ms:>4}ms{cer} → {}",
            out.segments.len(),
            out.text.replace('\n', " ")
        );
    }
    println!("\n=== 整段（zh + ITN）汇总 ===");
    for (cat, (files, dist, chars)) in &stats {
        if *chars > 0 {
            println!(
                "{cat:>6}: {files:>2} 条, CER {dist}/{chars} = {:.2}%",
                *dist as f64 / *chars as f64 * 100.0
            );
        } else {
            println!("{cat:>6}: {files:>2} 条（无参考文本，只看输出）");
        }
    }
}

/// 真模型 + 流式模式跑全语料：与整段对比。
#[test]
fn probe_corpus_streaming() {
    let Some(dir) = stream_dir() else {
        eprintln!("跳过：未设置 VOICE_NOTES_STREAMING_MODEL_DIR");
        return;
    };
    let mut config = OnlineRecognizerConfig::default();
    config.model_config.transducer = OnlineTransducerModelConfig {
        encoder: Some(dir.join("encoder.int8.onnx").display().to_string()),
        decoder: Some(dir.join("decoder.onnx").display().to_string()),
        joiner: Some(dir.join("joiner.int8.onnx").display().to_string()),
    };
    config.model_config.tokens = Some(dir.join("tokens.txt").display().to_string());
    config.model_config.num_threads = 2;
    config.model_config.provider = Some("cpu".into());
    config.decoding_method = Some("greedy_search".into());
    config.enable_endpoint = false;
    let Some(recognizer) = OnlineRecognizer::create(&config) else {
        eprintln!("跳过：流式模型加载失败");
        return;
    };
    let files = all_corpus();
    if files.is_empty() {
        eprintln!("跳过：没有语料");
        return;
    }
    let mut stats: std::collections::BTreeMap<&str, (usize, usize, usize)> = Default::default();
    for (name, file) in &files {
        let wave = sherpa_onnx::Wave::read(file.to_str().unwrap()).expect("读取音频");
        let stream = recognizer.create_stream();
        let started = std::time::Instant::now();
        for chunk in wave.samples().chunks(3200) {
            stream.accept_waveform(16_000, chunk);
            while recognizer.is_ready(&stream) {
                recognizer.decode(&stream);
            }
        }
        stream.input_finished();
        while recognizer.is_ready(&stream) {
            recognizer.decode(&stream);
        }
        let ms = started.elapsed().as_millis();
        let text = recognizer
            .get_result(&stream)
            .map(|r| r.text)
            .unwrap_or_default();
        let cat = category(name);
        let entry = stats.entry(cat).or_insert((0, 0, 0));
        entry.0 += 1;
        let mut cer = String::new();
        if let Some(reference) = reference_for(file) {
            let hyp: Vec<char> = norm(&text).chars().collect();
            let refc: Vec<char> = norm(&reference).chars().collect();
            let dist = edit_distance(&hyp, &refc);
            entry.1 += dist;
            entry.2 += refc.len();
            cer = format!(
                "  CER {}/{}={:.1}%",
                dist,
                refc.len(),
                dist as f64 / refc.len().max(1) as f64 * 100.0
            );
        }
        println!("[流式 {cat:>6}] {name:<18} {ms:>4}ms{cer} → {}", text.replace('\n', " "));
    }
    println!("\n=== 流式（greedy）汇总 ===");
    for (cat, (files, dist, chars)) in &stats {
        if *chars > 0 {
            println!(
                "{cat:>6}: {files:>2} 条, CER {dist}/{chars} = {:.2}%",
                *dist as f64 / *chars as f64 * 100.0
            );
        } else {
            println!("{cat:>6}: {files:>2} 条（无参考文本，只看输出）");
        }
    }
}

/// 离线：ITN 开关 + 语言指定方式（旧的三条样本）
#[test]
fn probe_offline_knobs() {
    let Some(dir) = model_dir() else {
        eprintln!("跳过：未设置 VOICE_NOTES_MODEL_DIR");
        return;
    };
    let paths = paths(&dir);
    for (label, lang, itn) in [
        ("zh + ITN", "zh", true),
        ("zh 无 ITN", "zh", false),
        ("auto + ITN", "auto", true),
    ] {
        let engine = Engine::new(&paths, lang, itn).expect("加载模型");
        for (name, file) in legacy() {
            let wave = sherpa_onnx::Wave::read(file.to_str().unwrap()).expect("读取音频");
            let out = engine.transcribe(wave.samples());
            println!(
                "[离线 {label:>8}] {name}: {}",
                out.text.replace('\n', " ")
            );
        }
    }
}

/// 流式：热词对比（在指定样本上）
#[test]
fn probe_streaming_hotwords() {
    let Some(dir) = stream_dir() else {
        eprintln!("跳过：未设置 VOICE_NOTES_STREAMING_MODEL_DIR");
        return;
    };
    let target = std::env::var("VOICE_NOTES_HOTWORD_TARGET")
        .unwrap_or_else(|_| "tests/fixtures/quality/tts-17.wav".into());
    let target = PathBuf::from(target);
    if !target.is_file() {
        eprintln!("跳过：找不到 {target:?}");
        return;
    }
    let hotwords = std::env::var("VOICE_NOTES_HOTWORDS").ok();
    for (label, words) in [("greedy", None), ("beam", None), ("beam+热词", hotwords.as_deref())] {
        let mut config = OnlineRecognizerConfig::default();
        config.model_config.transducer = OnlineTransducerModelConfig {
            encoder: Some(dir.join("encoder.int8.onnx").display().to_string()),
            decoder: Some(dir.join("decoder.onnx").display().to_string()),
            joiner: Some(dir.join("joiner.int8.onnx").display().to_string()),
        };
        config.model_config.tokens = Some(dir.join("tokens.txt").display().to_string());
        config.model_config.num_threads = 2;
        config.model_config.provider = Some("cpu".into());
        config.decoding_method = Some(
            if label == "greedy" { "greedy_search".into() } else { "modified_beam_search".into() },
        );
        config.enable_endpoint = false;
        if let Some(words) = words {
            config.hotwords_buf = Some(words.as_bytes().to_vec());
            config.hotwords_score = 1.5;
        }
        let Some(recognizer) = OnlineRecognizer::create(&config) else {
            eprintln!("跳过：{label} 加载失败");
            continue;
        };
        let wave = sherpa_onnx::Wave::read(target.to_str().unwrap()).expect("读取音频");
        let stream = match words {
            Some(words) => recognizer.create_stream_with_hotwords(words),
            None => recognizer.create_stream(),
        };
        for chunk in wave.samples().chunks(3200) {
            stream.accept_waveform(16_000, chunk);
            while recognizer.is_ready(&stream) {
                recognizer.decode(&stream);
            }
        }
        stream.input_finished();
        while recognizer.is_ready(&stream) {
            recognizer.decode(&stream);
        }
        println!(
            "[流式 {label:>10}] {}: {}",
            target.file_name().unwrap().to_string_lossy(),
            recognizer.get_result(&stream).map(|r| r.text).unwrap_or_default()
        );
    }
}

/// VAD：不同模型 / 不同静音阈值下的切句（默认只看噪声样本）
#[test]
fn probe_vad_variants() {
    let Some(dir) = model_dir() else {
        eprintln!("跳过：未设置 VOICE_NOTES_MODEL_DIR");
        return;
    };
    let v4 = dir.join("silero_vad.onnx");
    let alt = std::env::var("VOICE_NOTES_VAD_ALT")
        .ok()
        .map(PathBuf::from)
        .unwrap_or_else(|| v4.clone());
    let files: Vec<(String, PathBuf)> = if corpus("noisy-").is_empty() {
        legacy()
    } else {
        corpus("noisy-")
            .into_iter()
            .chain(corpus("real-"))
            .take(6)
            .collect()
    };
    for (label, vad_path, min_silence) in [
        ("v4 min_sil=0.5 (现状)", v4.clone(), 0.5),
        ("v4 min_sil=0.3", v4.clone(), 0.3),
        ("alt min_sil=0.5", alt.clone(), 0.5),
        ("alt min_sil=0.3", alt.clone(), 0.3),
    ] {
        if !vad_path.is_file() {
            println!("[{label}] 缺少 VAD 文件，跳过");
            continue;
        }
        let config = sherpa_onnx::VadModelConfig {
            silero_vad: sherpa_onnx::SileroVadModelConfig {
                model: Some(vad_path.display().to_string()),
                threshold: 0.5,
                min_silence_duration: min_silence,
                min_speech_duration: 0.25,
                window_size: 512,
                max_speech_duration: 30.0,
            },
            sample_rate: 16_000,
            num_threads: 1,
            provider: Some("cpu".into()),
            debug: false,
            ..Default::default()
        };
        let mut line = format!("[{label}]");
        for (name, file) in &files {
            let wave = sherpa_onnx::Wave::read(file.to_str().unwrap()).expect("读取音频");
            let Some(detector) = sherpa_onnx::VoiceActivityDetector::create(&config, 62.0) else {
                line.push_str(" VAD加载失败");
                break;
            };
            detector.reset();
            for chunk in wave.samples().chunks(512) {
                detector.accept_waveform(chunk);
            }
            detector.flush();
            let mut spans = Vec::new();
            while let Some(seg) = detector.front() {
                spans.push((seg.start() as i64, seg.samples().len() as i64));
                detector.pop();
            }
            line.push_str(&format!(
                " {}={}段[{}]",
                name.trim_end_matches(".wav"),
                spans.len(),
                spans
                    .iter()
                    .map(|(s, l)| format!("{:.1}-{:.1}", *s as f32 / 16_000.0, (*s + *l) as f32 / 16_000.0))
                    .collect::<Vec<_>>()
                    .join(",")
            ));
        }
        println!("{line}");
    }
}

/// 流式端点行为：连续说话会不会被 rule3 硬切
#[test]
fn probe_streaming_endpoint_cut() {
    let Some(dir) = stream_dir() else {
        eprintln!("跳过：未设置 VOICE_NOTES_STREAMING_MODEL_DIR");
        return;
    };
    let rule3: f32 = std::env::var("VOICE_NOTES_RULE3")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20.0);
    let mut config = OnlineRecognizerConfig::default();
    config.model_config.transducer = OnlineTransducerModelConfig {
        encoder: Some(dir.join("encoder.int8.onnx").display().to_string()),
        decoder: Some(dir.join("decoder.onnx").display().to_string()),
        joiner: Some(dir.join("joiner.int8.onnx").display().to_string()),
    };
    config.model_config.tokens = Some(dir.join("tokens.txt").display().to_string());
    config.model_config.num_threads = 2;
    config.model_config.provider = Some("cpu".into());
    config.decoding_method = Some("greedy_search".into());
    config.enable_endpoint = true;
    config.rule1_min_trailing_silence = 2.4;
    config.rule2_min_trailing_silence = 1.2;
    config.rule3_min_utterance_length = rule3;
    let Some(recognizer) = OnlineRecognizer::create(&config) else {
        eprintln!("跳过：模型加载失败");
        return;
    };
    let wave = sherpa_onnx::Wave::read("tests/fixtures/zh.wav").expect("读取音频");
    let start = (0.7 * 16_000.0) as usize;
    let speech = &wave.samples()[start..];
    let stream = recognizer.create_stream();
    let mut fed = 0usize;
    let mut cuts: Vec<f32> = Vec::new();
    for _ in 0..5 {
        for chunk in speech.chunks(3200) {
            stream.accept_waveform(16_000, chunk);
            fed += chunk.len();
            while recognizer.is_ready(&stream) {
                recognizer.decode(&stream);
                if recognizer.is_endpoint(&stream) {
                    cuts.push(fed as f32 / 16_000.0);
                    recognizer.reset(&stream);
                }
            }
        }
    }
    println!(
        "[流式端点 rule3={rule3}] 连续喂入 {:.1}s，endpoint 触发 {cuts:?}（秒）",
        fed as f32 / 16_000.0
    );
}

/// 离线流的按句选项（目前只有 SenseVoice 的 use_itn）
#[test]
fn probe_offline_stream_options() {
    let Some(dir) = model_dir() else {
        eprintln!("跳过：未设置 VOICE_NOTES_MODEL_DIR");
        return;
    };
    let paths = paths(&dir);
    let mut config = OfflineRecognizerConfig::default();
    config.model_config.sense_voice = OfflineSenseVoiceModelConfig {
        model: Some(paths.model.clone()),
        language: Some("zh".into()),
        use_itn: true,
    };
    config.model_config.tokens = Some(paths.tokens.clone());
    let Some(recognizer) = OfflineRecognizer::create(&config) else {
        eprintln!("跳过：模型加载失败");
        return;
    };
    for (name, file) in legacy() {
        let wave = sherpa_onnx::Wave::read(file.to_str().unwrap()).expect("读取音频");
        for itn in ["1", "0"] {
            let stream = recognizer.create_stream();
            stream.set_option("use_itn", itn);
            stream.accept_waveform(16_000, wave.samples());
            recognizer.decode(&stream);
            println!(
                "[流选项 use_itn={itn}] {name}: {}",
                stream.get_result().map(|r| r.text.replace('\n', " ")).unwrap_or_default()
            );
        }
    }
}

/// 模拟「VAD 前补静音」后的整段模式 CER（对比当前代码，验证切字头损失）
#[test]
fn probe_corpus_preroll() {
    use sherpa_onnx::VoiceActivityDetector;
    let Some(dir) = model_dir() else {
        eprintln!("跳过：未设置 VOICE_NOTES_MODEL_DIR");
        return;
    };
    let paths = paths(&dir);
    let engine = Engine::new(&paths, "zh", true).expect("加载模型");
    let vad = engine.vad_config().expect("vad");
    let files = all_corpus();
    if files.is_empty() {
        eprintln!("跳过：没有语料");
        return;
    }
    let mut stats: std::collections::BTreeMap<&str, (usize, usize, usize)> = Default::default();
    for (name, file) in &files {
        let wave = sherpa_onnx::Wave::read(file.to_str().unwrap()).expect("读取音频");
        let mut samples = vec![0.0f32; 8000]; // 0.5s 前导静音
        samples.extend_from_slice(wave.samples());
        let Some(mut detector) = VoiceActivityDetector::create(&vad, 62.0) else {
            eprintln!("VAD 创建失败");
            return;
        };
        detector.reset();
        let mut text = String::new();
        for chunk in samples.chunks(512) {
            detector.accept_waveform(chunk);
            while let Some(seg) = detector.front() {
                text.push_str(&engine.recognize(seg.samples()));
                detector.pop();
            }
        }
        detector.flush();
        while let Some(seg) = detector.front() {
            text.push_str(&engine.recognize(seg.samples()));
            detector.pop();
        }
        let cat = category(name);
        let entry = stats.entry(cat).or_insert((0, 0, 0));
        entry.0 += 1;
        let mut cer = String::new();
        if let Some(reference) = reference_for(file) {
            let hyp: Vec<char> = norm(&text).chars().collect();
            let refc: Vec<char> = norm(&reference).chars().collect();
            let dist = edit_distance(&hyp, &refc);
            entry.1 += dist;
            entry.2 += refc.len();
            cer = format!("  CER {}/{}={:.1}%", dist, refc.len(), dist as f64 / refc.len().max(1) as f64 * 100.0);
        }
        println!("[预滚 {cat:>6}] {name:<18} {cer} → {}", text.replace('\n', " "));
    }
    println!("\n=== 补 0.5s 前导静音后汇总 ===");
    for (cat, (files, dist, chars)) in &stats {
        if *chars > 0 {
            println!("{cat:>6}: {files:>2} 条, CER {dist}/{chars} = {:.2}%", *dist as f64 / *chars as f64 * 100.0);
        } else {
            println!("{cat:>6}: {files:>2} 条（无参考文本）");
        }
    }
}

/// 模拟「段起点回扩 0.4s」的整段模式 CER（不扰动 VAD 状态，保留段前音频）
#[test]
fn probe_corpus_preroll_buffer() {
    use sherpa_onnx::VoiceActivityDetector;
    let Some(dir) = model_dir() else {
        eprintln!("跳过：未设置 VOICE_NOTES_MODEL_DIR");
        return;
    };
    let paths = paths(&dir);
    let engine = Engine::new(&paths, "zh", true).expect("加载模型");
    let vad = engine.vad_config().expect("vad");
    let files = all_corpus();
    if files.is_empty() {
        eprintln!("跳过：没有语料");
        return;
    }
    let mut stats: std::collections::BTreeMap<&str, (usize, usize, usize)> = Default::default();
    for (name, file) in &files {
        let wave = sherpa_onnx::Wave::read(file.to_str().unwrap()).expect("读取音频");
        let samples = wave.samples().to_vec();
        let Some(mut detector) = VoiceActivityDetector::create(&vad, 62.0) else {
            eprintln!("VAD 创建失败");
            return;
        };
        detector.reset();
        let mut text = String::new();
        let mut decoded = 0usize;
        for chunk in samples.chunks(512) {
            detector.accept_waveform(chunk);
            while let Some(seg) = detector.front() {
                let start = seg.start() as usize; // 段起点（采样数）
                let len = seg.samples().len();
                // 只对第一段回扩：后续段前面有静音，sherpa 自己的 padding 够用；
                // 全部回扩会吃进上一段的话，造成重复。
                let from = if decoded == 0 { start.saturating_sub(6400) } else { start };
                decoded += 1;
                let to = (start + len).min(samples.len());
                if from < to {
                    text.push_str(&engine.recognize(&samples[from..to]));
                }
                detector.pop();
            }
        }
        detector.flush();
        while let Some(seg) = detector.front() {
            let start = seg.start() as usize;
            let len = seg.samples().len();
            let from = if decoded == 0 { start.saturating_sub(6400) } else { start };
            decoded += 1;
            let to = (start + len).min(samples.len());
            if from < to {
                text.push_str(&engine.recognize(&samples[from..to]));
            }
            detector.pop();
        }
        let cat = category(name);
        let entry = stats.entry(cat).or_insert((0, 0, 0));
        entry.0 += 1;
        let mut cer = String::new();
        if let Some(reference) = reference_for(file) {
            let hyp: Vec<char> = norm(&text).chars().collect();
            let refc: Vec<char> = norm(&reference).chars().collect();
            let dist = edit_distance(&hyp, &refc);
            entry.1 += dist;
            entry.2 += refc.len();
            cer = format!("  CER {}/{}={:.1}%", dist, refc.len(), dist as f64 / refc.len().max(1) as f64 * 100.0);
        }
        println!("[回扩 {cat:>6}] {name:<18} {cer} → {}", text.replace('\n', " "));
    }
    println!("\n=== 段起点回扩 0.4s 后汇总 ===");
    for (cat, (files, dist, chars)) in &stats {
        if *chars > 0 {
            println!("{cat:>6}: {files:>2} 条, CER {dist}/{chars} = {:.2}%", *dist as f64 / *chars as f64 * 100.0);
        } else {
            println!("{cat:>6}: {files:>2} 条（无参考文本）");
        }
    }
}

/// 录制路径：把「开头就是语音」的样本喂进 OfflineSession，确认首字没被切
#[test]
fn probe_offline_session_first_word() {
    use std::sync::Arc;
    let Some(dir) = model_dir() else {
        eprintln!("跳过：未设置 VOICE_NOTES_MODEL_DIR");
        return;
    };
    let paths = paths(&dir);
    let engine = Arc::new(Engine::new(&paths, "zh", true).expect("加载"));
    let vad = engine.vad_config().expect("vad");
    for name in ["tts-01", "tts-05", "tts-09"] {
        let file = format!("tests/fixtures/quality/{name}.wav");
        let Ok(Some(wave)) = std::fs::metadata(&file).map(|_| sherpa_onnx::Wave::read(&file)) else {
            continue;
        };
        let mut session = voice_notes_lib::asr::OfflineSession::new(engine.clone(), vad.clone());
        let mut events = Vec::new();
        for chunk in wave.samples().chunks(3_200) {
            events.extend(session.push(chunk));
        }
        events.extend(session.finish());
        let text: String = events
            .iter()
            .map(|e| match e {
                voice_notes_lib::asr::OfflineEvent::Segment(s) => s.text.as_str(),
            })
            .collect();
        println!("[录制路径] {name}: {}", text.replace('\n', " "));
    }
}

/// 生产路径（StreamingEngine）验证：术语表真的进了识别器
#[test]
fn probe_streaming_engine_hotwords() {
    use voice_notes_lib::asr::StreamingEngine;
    use voice_notes_lib::models::StreamingPaths;
    let Some(dir) = stream_dir() else {
        eprintln!("跳过：未设置 VOICE_NOTES_STREAMING_MODEL_DIR");
        return;
    };
    let paths = StreamingPaths {
        dir: dir.display().to_string(),
        encoder: dir.join("encoder.int8.onnx").display().to_string(),
        decoder: dir.join("decoder.onnx").display().to_string(),
        joiner: dir.join("joiner.int8.onnx").display().to_string(),
        tokens: dir.join("tokens.txt").display().to_string(),
        vad: dir.join("silero_vad.onnx").is_file().then(|| dir.join("silero_vad.onnx").display().to_string()),
    };
    for sample in ["tests/fixtures/quality/tts-17.wav", "tests/fixtures/zh-tail2.wav"] {
    let wave = sherpa_onnx::Wave::read(sample).expect("音频");
    for (label, words) in [("无术语表", vec![]), ("术语表", vec!["再确认一下".to_string()])] {
        let engine = StreamingEngine::new(&paths, &words).expect("加载流式引擎");
        let stream = engine.new_stream();
        for chunk in wave.samples().chunks(3_200) {
            stream.accept_waveform(16_000, chunk);
            while engine.recognizer.is_ready(&stream) {
                engine.recognizer.decode(&stream);
            }
        }
        stream.input_finished();
        while engine.recognizer.is_ready(&stream) {
            engine.recognizer.decode(&stream);
        }
        println!(
            "[流式引擎 {label}] {sample}: {}",
            engine.recognizer.get_result(&stream).map(|r| r.text).unwrap_or_default()
        );
    }
    }
}
