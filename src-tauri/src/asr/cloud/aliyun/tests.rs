//! 阿里 `fun-asr-realtime` 的协议层单测：请求组装、事件解码、结果映射。

use super::*;

fn config() -> AliyunConfig {
    AliyunConfig {
        base_url: String::new(),
        api_key: "sk-test".into(),
        model: String::new(),
        language_hint: None,
        context_text: None,
    }
}

// ── run-task / finish-task ──────────────────────────────────────────

#[test]
fn run_task_携带必需的字段与固定取值() {
    let raw = encode_run_task(&config(), "task-1");
    let value: Value = serde_json::from_str(&raw).unwrap();

    assert_eq!(value["header"]["action"], "run-task");
    assert_eq!(value["header"]["task_id"], "task-1");
    assert_eq!(value["header"]["streaming"], "duplex");
    assert_eq!(value["payload"]["task_group"], "audio");
    assert_eq!(value["payload"]["task"], "asr");
    assert_eq!(value["payload"]["function"], "recognition");
    assert_eq!(value["payload"]["model"], "fun-asr-realtime");
    assert_eq!(value["payload"]["parameters"]["format"], "pcm");
    assert_eq!(value["payload"]["parameters"]["sample_rate"], 16_000);
    assert_eq!(value["payload"]["parameters"]["max_sentence_silence"], 1300);
    assert_eq!(value["payload"]["parameters"]["heartbeat"], true);
    // 没指定语言：不传 language_hints，让模型自动识别。
    assert!(value["payload"]["parameters"].get("language_hints").is_none());
    // 没术语表：input 是空对象。
    assert_eq!(value["payload"]["input"], serde_json::json!({}));
}

#[test]
fn run_task_的语言提示只放一个值() {
    let mut cfg = config();
    cfg.language_hint = Some("zh".into());
    let value: Value = serde_json::from_str(&encode_run_task(&cfg, "t")).unwrap();
    assert_eq!(
        value["payload"]["parameters"]["language_hints"],
        serde_json::json!(["zh"])
    );
}

#[test]
fn run_task_带术语表时放进_context() {
    let mut cfg = config();
    cfg.context_text = Some("术语表：祺元、洛柯".into());
    let value: Value = serde_json::from_str(&encode_run_task(&cfg, "t")).unwrap();
    assert_eq!(
        value["payload"]["input"]["context"][0]["role"],
        "user"
    );
    assert_eq!(
        value["payload"]["input"]["context"][0]["content"][0]["type"],
        "input_text"
    );
    assert_eq!(
        value["payload"]["input"]["context"][0]["content"][0]["text"],
        "术语表：祺元、洛柯"
    );
}

#[test]
fn finish_task_复用同一个_task_id_且_input_为空对象() {
    let value: Value = serde_json::from_str(&encode_finish_task("task-9")).unwrap();
    assert_eq!(value["header"]["action"], "finish-task");
    assert_eq!(value["header"]["task_id"], "task-9");
    assert_eq!(value["payload"]["input"], serde_json::json!({}));
}

// ── input.context 的组装与截断 ──────────────────────────────────────

#[test]
fn 空词表不带上下文() {
    let (text, ignored) = build_context(&[]);
    assert_eq!(text, None);
    assert_eq!(ignored, 0);
    // 全是空白也算空。
    let (text, ignored) = build_context(&["  ".into(), "".into()]);
    assert_eq!(text, None);
    assert_eq!(ignored, 0);
}

#[test]
fn 词表拼成一段带前缀的文本() {
    let words = vec!["祺元".to_string(), "洛柯".to_string()];
    let (text, ignored) = build_context(&words);
    assert_eq!(text.as_deref(), Some("术语表：祺元、洛柯"));
    assert_eq!(ignored, 0);
}

#[test]
fn 超限时在术语边界上丢整条而不是截半个词() {
    // 每个词 100 个中文字：前缀 4 字 + 100 + 「、」+ 100 + 「、」+ 100 = 306，
    // 再加第 4 个词就是 407 > 400 → 第 4 条整条丢弃。
    let words: Vec<String> = (0..4).map(|_| "甲".repeat(100)).collect();
    let (text, ignored) = build_context(&words);
    let text = text.expect("前三条放得下");
    assert_eq!(ignored, 1, "第 4 条整条被丢弃");
    assert_eq!(text.chars().count(), 306);
    // 关键：结尾必须是完整的词，不能是半个。
    assert!(text.ends_with(&"甲".repeat(100)));
    // 必须是 3 条而不是 3 条多一点点。
    assert_eq!(text.matches('甲').count(), 300);
}

#[test]
fn 上限按字符计而不是字节() {
    // 397 个中文字 + 前缀 4 字 = 401 > 400 → 一条都放不下。
    let (text, ignored) = build_context(&["甲".repeat(397)]);
    assert_eq!(text, None);
    assert_eq!(ignored, 1);
    // 396 个中文字 + 前缀 4 字 = 400，正好放得下（按字节算是 1188，会误判超限）。
    let (text, ignored) = build_context(&["甲".repeat(396)]);
    assert!(text.is_some(), "396 个中文字按字符算没超 400");
    assert_eq!(ignored, 0);
}

// ── 事件解码 ────────────────────────────────────────────────────────

#[test]
fn 解码四种服务端事件() {
    let started = r#"{"header":{"task_id":"t","event":"task-started","attributes":{}},"payload":{}}"#;
    assert!(matches!(
        decode_server_event(started).unwrap(),
        ServerEvent::TaskStarted
    ));

    let finished =
        r#"{"header":{"task_id":"t","event":"task-finished","attributes":{}},"payload":{"output":{},"usage":null}}"#;
    assert!(matches!(
        decode_server_event(finished).unwrap(),
        ServerEvent::TaskFinished
    ));

    let failed = r#"{"header":{"event":"task-failed","error_code":"CLIENT_ERROR","error_message":"request timeout after 23 seconds."},"payload":{}}"#;
    match decode_server_event(failed).unwrap() {
        ServerEvent::TaskFailed { code, message } => {
            assert_eq!(code, "CLIENT_ERROR");
            assert_eq!(message, "request timeout after 23 seconds.");
        }
        other => panic!("应当是 TaskFailed：{other:?}"),
    }

    let result = r#"{"header":{"event":"result-generated"},"payload":{"output":{"sentence":{"sentence_id":1,"sentence_begin":false,"sentence_end":true,"begin_time":170,"end_time":920,"text":"好，我知道了","heartbeat":false}},"usage":{"duration":3}}}"#;
    match decode_server_event(result).unwrap() {
        ServerEvent::Sentence(sentence) => {
            assert_eq!(sentence.id, 1);
            assert!(!sentence.begin);
            assert!(sentence.end);
            assert!(!sentence.heartbeat);
            assert_eq!(sentence.text, "好，我知道了");
        }
        other => panic!("应当是 Sentence：{other:?}"),
    }
}

#[test]
fn 未知事件不报错() {
    let raw = r#"{"header":{"event":"some-future-event"},"payload":{}}"#;
    assert!(matches!(
        decode_server_event(raw).unwrap(),
        ServerEvent::Other
    ));
}

#[test]
fn 非_json_报协议错误() {
    let error = decode_server_event("not json").unwrap_err();
    assert!(error.starts_with("cloud.protocolError"), "{error}");
}

// ── 结果映射（有状态） ──────────────────────────────────────────────

fn sentence(id: i64, end: bool, text: &str) -> Sentence {
    Sentence {
        id,
        begin: false,
        end,
        heartbeat: false,
        text: text.into(),
    }
}

fn partials(events: &[CloudEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            CloudEvent::Partial(text) => Some(text.clone()),
            _ => None,
        })
        .collect()
}

fn finals(events: &[CloudEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            CloudEvent::Final { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

/// design D2 第 2 条：`text` 是**整句快照**，替换不追加。
///
/// 样例刻意用「第二次**改写**第一次」（更短），这样「追加」实现会得到
/// 「你好世界你好」而不是「你好」—— 前缀型样例区分不出这两种实现。
#[test]
fn 中间结果是整句快照_替换不追加() {
    let mut mapper = AliyunMapper::new();
    let first = mapper.map_sentence(sentence(1, false, "你好世界"));
    assert_eq!(partials(&first), vec!["你好世界".to_string()]);

    let second = mapper.map_sentence(sentence(1, false, "你好"));
    assert_eq!(partials(&second), vec!["你好".to_string()]);
    // 展示形态：第二次比第一次短，且不是「第一次 + 第二次」。
    assert!(second.is_empty() || partials(&second)[0].len() < "你好世界".len());
    assert_ne!(partials(&second)[0], "你好世界你好");
}

/// 断言二：**从样例推导形态**，再与能力声明比较。
///
/// 推导只看样例文本（不接收 `PartialSemantics`），所以把声明改错时这条会红。
#[test]
fn 声明的中间结果形态与样例一致() {
    let mut mapper = AliyunMapper::new();
    let first = partials(&mapper.map_sentence(sentence(1, false, "你好世界")));
    let second = partials(&mapper.map_sentence(sentence(1, false, "你好")));
    let (first, second) = (first[0].clone(), second[0].clone());

    // 推导：第二次若包含第一次的全部内容且更长 → 增量；否则（更短/改写）→ 整句替换。
    let looks_incremental = second.len() > first.len() && second.starts_with(&first);
    let derived = if looks_incremental {
        PartialSemantics::Delta
    } else {
        PartialSemantics::Preview
    };
    assert_eq!(
        AliyunProvider::CAPABILITIES.partial_semantics,
        derived,
        "声明的形态与样例推导出的形态不一致"
    );
}

#[test]
fn 句子结束时产定稿() {
    let mut mapper = AliyunMapper::new();
    mapper.map_sentence(sentence(1, false, "好，我知道"));
    let events = mapper.map_sentence(sentence(1, true, "好，我知道了"));
    assert_eq!(finals(&events), vec!["好，我知道了".to_string()]);
}

#[test]
fn 心跳被忽略且不触发定稿() {
    let mut mapper = AliyunMapper::new();
    mapper.map_sentence(sentence(1, false, "第一句"));
    // 心跳的 sentence_id 固定为 0：若先判「新 id」就会把第一句提前定稿。
    let beat = mapper.map_sentence(Sentence {
        id: 0,
        begin: false,
        end: false,
        heartbeat: true,
        text: String::new(),
    });
    assert!(beat.is_empty(), "心跳不产事件");
    let again = mapper.map_sentence(sentence(1, false, "第一句"));
    assert!(finals(&again).is_empty(), "心跳不该把上一句定稿");
}

#[test]
fn 新句开始而上一句未定稿时按快照定稿() {
    let mut mapper = AliyunMapper::new();
    mapper.map_sentence(sentence(1, false, "说了一半"));
    // 服务端直接开新句（官方没承诺一定会先给 sentence_end=true）。
    let events = mapper.map_sentence(sentence(2, false, "第二句"));
    assert_eq!(finals(&events), vec!["说了一半".to_string()], "上一句按快照定稿");
    assert_eq!(partials(&events), vec!["第二句".to_string()]);
}

#[test]
fn 已定稿的句再收到事件一律忽略() {
    let mut mapper = AliyunMapper::new();
    mapper.map_sentence(sentence(1, true, "定稿了"));
    let again = mapper.map_sentence(sentence(1, true, "定稿了"));
    assert!(again.is_empty(), "同一句不能产第二个 Final");
}

#[test]
fn 迟到的_end_不会清掉新句的实时行() {
    let mut mapper = AliyunMapper::new();
    mapper.map_sentence(sentence(1, false, "第一句"));
    // 新句开始 → 第一句被防御性定稿。
    mapper.map_sentence(sentence(2, false, "第二句"));
    // 第一句的 end 迟到：不能再产 Final（否则会话层会清掉正在显示的实时行）。
    let late = mapper.map_sentence(sentence(1, true, "第一句"));
    assert!(late.is_empty(), "迟到的 end 必须被忽略");
}

#[test]
fn 空文本不产_partial_也不产空定稿() {
    let mut mapper = AliyunMapper::new();
    let empty_partial = mapper.map_sentence(sentence(1, false, "   "));
    assert!(empty_partial.is_empty());
    let empty_final = mapper.map_sentence(sentence(1, true, ""));
    assert!(empty_final.is_empty(), "空文本的定稿不该 emit");
}

#[test]
fn 同句结束时文本为空则用已有快照定稿() {
    let mut mapper = AliyunMapper::new();
    mapper.map_sentence(sentence(1, false, "有内容"));
    let events = mapper.map_sentence(sentence(1, true, ""));
    assert_eq!(
        finals(&events),
        vec!["有内容".to_string()],
        "结束事件没带文本时用该句最后一次快照"
    );
}

// ── 能力声明 ────────────────────────────────────────────────────────

#[test]
fn 能力声明为服务端断句且不需要本地_vad() {
    let caps = AliyunProvider::CAPABILITIES;
    assert_eq!(caps.endpoint_kind, EndpointKind::ServerEndpoint);
    assert!(!caps.needs_local_vad);
    assert!(caps.supports_keywords);
    assert_eq!(caps.target_sample_rate, 16_000);
    assert!(super::super::valid_capabilities(&caps));
}

/// 后台错误必须能从 `&mut dyn CloudProvider` 取到 —— 只写固有方法会让
/// trait 的默认实现遮蔽它（恒 `None`）。
#[test]
fn 后台错误能从_trait_方法取到() {
    let mut provider = AliyunProvider::new(config());
    provider.error_tx.send("cloud.disconnected".into()).unwrap();
    let dynamic: &mut dyn CloudProvider = &mut provider;
    assert_eq!(dynamic.take_error().as_deref(), Some("cloud.disconnected"));
}

/// 语言映射：`auto` 与未知值都不传，`yue` 映射成 `zh`（阿里没有 `yue` code）。
#[test]
fn 语言映射按官方支持的_code() {
    assert_eq!(language_hint("auto"), None);
    assert_eq!(language_hint(""), None);
    assert_eq!(language_hint("zh").as_deref(), Some("zh"));
    // 粤语：阿里 fun-asr 的 language_hints 里没有 yue，靠 zh 模型自动识别。
    assert_eq!(language_hint("yue").as_deref(), Some("zh"));
    assert_eq!(language_hint("en").as_deref(), Some("en"));
    assert_eq!(language_hint("ja").as_deref(), Some("ja"));
    assert_eq!(language_hint("ko").as_deref(), Some("ko"));
    // 不在支持列表里的值：宁可不传，也不要传非法值。
    assert_eq!(language_hint("fr"), None);
}
