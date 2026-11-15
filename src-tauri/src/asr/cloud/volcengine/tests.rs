//! 火山 provider 的协议层单测：结果映射与热词清洗。

use super::*;

/// 官方示例的响应：两个已定稿的句子。
const SAMPLE: &str = r#"{
  "audio_info": {"duration": 3696},
  "result": {
    "text": "这是字节跳动， 今日头条母公司。",
    "utterances": [
      {"definite": true, "end_time": 1705, "start_time": 0, "text": "这是字节跳动，"},
      {"definite": true, "end_time": 3696, "start_time": 2110, "text": "今日头条母公司。"}
    ]
  }
}"#;

/// 已定稿的句子 → 定稿事件（不是实时行）。
#[test]
fn 定稿句子映射为final() {
    let mut emitted = 0;
    let events = map_response(SAMPLE.as_bytes(), &mut emitted);
    let finals: Vec<String> = events
        .iter()
        .filter_map(|event| match event {
            CloudEvent::Final { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(finals, vec!["这是字节跳动，".to_string(), "今日头条母公司。".to_string()]);
    assert_eq!(emitted, 2);
}

/// 最后一条未定稿的句子 → 实时行（当前句的累计串）。
#[test]
fn 未定稿句子映射为partial() {
    let payload = r#"{"result":{"utterances":[{"definite":false,"text":"正在说"}]}}"#;
    let mut emitted = 0;
    let events = map_response(payload.as_bytes(), &mut emitted);
    let partials: Vec<String> = events
        .iter()
        .filter_map(|event| match event {
            CloudEvent::Partial(text) => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(partials, vec!["正在说".to_string()]);
}

/// 同一句定稿后不再重复上抛（服务端会重复发累积结果）。
#[test]
fn 已定稿句子不重复上抛() {
    let mut emitted = 0;
    let first = map_response(SAMPLE.as_bytes(), &mut emitted);
    assert_eq!(first.len(), 2);
    let second = map_response(SAMPLE.as_bytes(), &mut emitted);
    assert!(second.is_empty(), "已上抛过的句子不该再来一遍");
}

/// 热词：控制字符丢弃、超上限（100 条）截断。
#[test]
fn 热词按火山规则清洗() {
    let mut words: Vec<String> = (0..120).map(|i| format!("词{i}")).collect();
    words.push("带\n控制符".into());
    let (clean, ignored) = clean_volcengine_keywords(&words);
    assert_eq!(clean.len(), MAX_KEYWORDS, "超上限要截断");
    assert_eq!(ignored, 1 + 20, "控制符 1 条 + 截断 20 条");
}

/// 术语表 → `corpus.context`（JSON 字符串）。
#[test]
fn 术语表编码为corpus_context() {
    let context = encode_corpus_context(&["玄戒芯片".into()]).expect("应有 context");
    let value: Value = serde_json::from_str(&context).expect("应是合法 JSON");
    assert_eq!(value["hotwords"][0]["word"], "玄戒芯片");
    assert!(encode_corpus_context(&[]).is_none(), "没有热词就不带 context");
}

/// 能力声明：服务端断句、不需要本地 VAD、16 kHz。
#[test]
fn 能力声明为服务端断句() {
    let caps = VolcengineProvider::CAPABILITIES;
    assert_eq!(caps.endpoint_kind, EndpointKind::ServerEndpoint);
    assert!(!caps.needs_local_vad, "服务端断句不需要本地 VAD");
    assert_eq!(caps.target_sample_rate, 16_000, "与内部轴一致，不重采样");
}

/// 首次全量请求的内容：模型名、音频参数、断句参数、热词。
#[test]
fn 首次请求内容正确() {
    let config = VolcengineConfig {
        base_url: String::new(),
        api_key: "k".into(),
        resource_id: String::new(),
        keywords: vec!["玄戒芯片".into()],
        itn: false,
        punct: true,
    };
    let value: Value = serde_json::from_slice(&encode_full_request(&config)).expect("应是合法 JSON");
    assert_eq!(value["request"]["model_name"], "bigmodel");
    assert_eq!(value["audio"]["format"], "pcm");
    assert_eq!(value["audio"]["rate"], 16000);
    assert_eq!(value["audio"]["bits"], 16);
    assert_eq!(value["audio"]["channel"], 1);
    assert_eq!(value["request"]["show_utterances"], true);
    assert_eq!(value["request"]["end_window_size"], 800);
    // ITN / 标点按设置走。
    assert_eq!(value["request"]["enable_itn"], false);
    assert_eq!(value["request"]["enable_punc"], true);
    // 热词在 corpus.context 里（JSON 字符串）。
    let context: Value =
        serde_json::from_str(value["request"]["corpus"]["context"].as_str().unwrap()).expect("context 应是 JSON");
    assert_eq!(context["hotwords"][0]["word"], "玄戒芯片");
}

/// 错误码映射：空音频 → 没有结果；等包超时 → 超时；其余 → 协议错误（带细节）。
#[test]
fn 错误码映射到字典码() {
    assert_eq!(map_error_code(45000002, "empty audio"), "cloud.noResult");
    assert_eq!(map_error_code(45000081, "timeout"), "cloud.timeout");
    let other = map_error_code(45000151, "bad format");
    assert!(other.starts_with("cloud.protocolError"), "实际 {other}");
    assert!(other.contains("45000151"), "细节要带上错误码：{other}");
}

/// 真服务冒烟：连上火山 → 发一段真实语音 → 收中间结果与定稿 → 末包收尾。
///
///   VOICE_NOTES_VOLCENGINE_KEY=... cargo test --manifest-path src-tauri/Cargo.toml \
///     --lib asr::cloud::volcengine::tests::真服务冒烟 -- --nocapture
///
/// **没有环境变量就跳过**（打印一行「跳过：未设置…」，不打印 key）。
/// key 只从环境变量读 —— 不写进仓库、不进日志、不进断言消息。
#[test]
fn 真服务冒烟() {
    let Ok(key) = std::env::var("VOICE_NOTES_VOLCENGINE_KEY") else {
        println!("跳过：未设置 VOICE_NOTES_VOLCENGINE_KEY");
        return;
    };
    let resource_id = std::env::var("VOICE_NOTES_VOLCENGINE_RESOURCE_ID")
        .unwrap_or_else(|_| DEFAULT_RESOURCE_ID.to_string());

    // 真实语音 fixture（16 kHz / 单声道 / 5.6 秒）。
    let wave = sherpa_onnx::Wave::read("tests/fixtures/zh.wav").expect("读取测试音频");
    let samples: Vec<f32> = wave.samples().to_vec();
    assert_eq!(wave.sample_rate(), 16_000, "fixture 应该是 16 kHz");

    let config = VolcengineConfig {
        base_url: String::new(),
        api_key: key,
        resource_id,
        keywords: vec!["玄戒芯片".into()],
        itn: true,
        punct: true,
    };
    use crate::asr::cloud::{CloudStreamingSession, StreamEvent};
    use crate::asr::AsrSession;
    let mut session = CloudStreamingSession::new(Box::new(VolcengineProvider::new(config)));
    session.connect().expect("应能连上火山");

    // 按 60 ms 一批喂进去（编排层就是这个节奏）。
    let mut finals: Vec<String> = Vec::new();
    let mut partials: Vec<String> = Vec::new();
    let chunk = 16_000 / 50; // 20 ms
    for part in samples.chunks(chunk) {
        session.advance_samples(part.len());
        for event in session.push(part) {
            match event {
                StreamEvent::Partial(text) => partials.push(text),
                StreamEvent::Segment(segment) => finals.push(segment.text),
            }
        }
        // 收一收后台事件（final 可能晚于音频到达）。
        for event in session.poll() {
            match event {
                StreamEvent::Partial(text) => partials.push(text),
                StreamEvent::Segment(segment) => finals.push(segment.text),
            }
        }
    }
    // 收尾：force_end 提交这一句，finish 发末包并等最终响应。
    for event in session.force_end() {
        match event {
            StreamEvent::Partial(text) => partials.push(text),
            StreamEvent::Segment(segment) => finals.push(segment.text),
        }
    }
    for event in session.finish() {
        match event {
            StreamEvent::Partial(text) => partials.push(text),
            StreamEvent::Segment(segment) => finals.push(segment.text),
        }
    }
    // 再等一会儿：最终响应可能还在路上（finish 内部已经等过一轮）。
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while std::time::Instant::now() < deadline {
        for event in session.poll() {
            match event {
                StreamEvent::Partial(text) => partials.push(text),
                StreamEvent::Segment(segment) => finals.push(segment.text),
            }
        }
        if !finals.is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    println!("中间结果条数：{}", partials.len());
    println!("定稿句子：{finals:?}");
    println!("最后一条中间结果：{:?}", partials.last());
    assert!(!finals.is_empty(), "应该至少收到一句定稿");
    assert!(
        finals.iter().any(|text| !text.trim().is_empty()),
        "定稿不应全是空的：{finals:?}"
    );
}
