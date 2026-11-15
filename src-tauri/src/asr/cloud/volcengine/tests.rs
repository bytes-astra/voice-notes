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

/// 热词：控制字符丢弃、超 100 tokens 截断。
#[test]
fn 热词按火山规则清洗() {
    let mut words: Vec<String> = (0..120).map(|i| format!("词{i}")).collect();
    words.push("带\n控制符".into());
    let (clean, ignored) = clean_volcengine_keywords(&words);
    assert_eq!(clean.len(), MAX_KEYWORD_TOKENS, "超上限要截断");
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
