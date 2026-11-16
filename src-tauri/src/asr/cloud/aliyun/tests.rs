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
fn run_task_carries_required_fields() {
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
fn run_task_sends_single_language_hint() {
    let mut cfg = config();
    cfg.language_hint = Some("zh".into());
    let value: Value = serde_json::from_str(&encode_run_task(&cfg, "t")).unwrap();
    assert_eq!(
        value["payload"]["parameters"]["language_hints"],
        serde_json::json!(["zh"])
    );
}

#[test]
fn run_task_puts_glossary_into_context() {
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
fn finish_task_reuses_task_id_with_empty_input() {
    let value: Value = serde_json::from_str(&encode_finish_task("task-9")).unwrap();
    assert_eq!(value["header"]["action"], "finish-task");
    assert_eq!(value["header"]["task_id"], "task-9");
    assert_eq!(value["payload"]["input"], serde_json::json!({}));
}

// ── input.context 的组装与截断 ──────────────────────────────────────

#[test]
fn empty_glossary_omits_context() {
    let (text, ignored) = build_context(&[]);
    assert_eq!(text, None);
    assert_eq!(ignored, 0);
    // 全是空白也算空。
    let (text, ignored) = build_context(&["  ".into(), "".into()]);
    assert_eq!(text, None);
    assert_eq!(ignored, 0);
}

#[test]
fn glossary_becomes_prefixed_text() {
    let words = vec!["祺元".to_string(), "洛柯".to_string()];
    let (text, ignored) = build_context(&words);
    assert_eq!(text.as_deref(), Some("术语表：祺元、洛柯"));
    assert_eq!(ignored, 0);
}

#[test]
fn over_limit_drops_whole_terms_not_half_a_word() {
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
fn limit_counts_chars_not_bytes() {
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
fn decodes_all_four_server_events() {
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
fn unknown_event_is_ignored() {
    let raw = r#"{"header":{"event":"some-future-event"},"payload":{}}"#;
    assert!(matches!(
        decode_server_event(raw).unwrap(),
        ServerEvent::Other
    ));
}

#[test]
fn non_json_payload_reports_protocol_error() {
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
fn partial_is_whole_snapshot_not_appended() {
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
fn declared_partial_semantics_matches_samples() {
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
fn sentence_end_yields_final() {
    let mut mapper = AliyunMapper::new();
    mapper.map_sentence(sentence(1, false, "好，我知道"));
    let events = mapper.map_sentence(sentence(1, true, "好，我知道了"));
    assert_eq!(finals(&events), vec!["好，我知道了".to_string()]);
}

#[test]
fn heartbeat_is_ignored_and_does_not_finalize() {
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
fn new_sentence_finalizes_previous_by_snapshot() {
    let mut mapper = AliyunMapper::new();
    mapper.map_sentence(sentence(1, false, "说了一半"));
    // 服务端直接开新句（官方没承诺一定会先给 sentence_end=true）。
    let events = mapper.map_sentence(sentence(2, false, "第二句"));
    assert_eq!(finals(&events), vec!["说了一半".to_string()], "上一句按快照定稿");
    assert_eq!(partials(&events), vec!["第二句".to_string()]);
}

#[test]
fn finalized_sentence_ignores_later_events() {
    let mut mapper = AliyunMapper::new();
    mapper.map_sentence(sentence(1, true, "定稿了"));
    let again = mapper.map_sentence(sentence(1, true, "定稿了"));
    assert!(again.is_empty(), "同一句不能产第二个 Final");
}

#[test]
fn late_end_does_not_clear_new_live_line() {
    let mut mapper = AliyunMapper::new();
    mapper.map_sentence(sentence(1, false, "第一句"));
    // 新句开始 → 第一句被防御性定稿。
    mapper.map_sentence(sentence(2, false, "第二句"));
    // 第一句的 end 迟到：不能再产 Final（否则会话层会清掉正在显示的实时行）。
    let late = mapper.map_sentence(sentence(1, true, "第一句"));
    assert!(late.is_empty(), "迟到的 end 必须被忽略");
}

#[test]
fn empty_text_yields_no_partial_and_no_empty_final() {
    let mut mapper = AliyunMapper::new();
    let empty_partial = mapper.map_sentence(sentence(1, false, "   "));
    assert!(empty_partial.is_empty());
    let empty_final = mapper.map_sentence(sentence(1, true, ""));
    assert!(empty_final.is_empty(), "空文本的定稿不该 emit");
}

#[test]
fn empty_end_uses_existing_snapshot() {
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
fn capabilities_are_server_endpoint_without_local_vad() {
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
fn background_error_is_taken_through_trait_method() {
    let mut provider = AliyunProvider::new(config());
    provider.error_tx.send("cloud.disconnected".into()).unwrap();
    let dynamic: &mut dyn CloudProvider = &mut provider;
    assert_eq!(dynamic.take_error().as_deref(), Some("cloud.disconnected"));
}

/// 语言映射：`auto` 与未知值都不传，`yue` 映射成 `zh`（阿里没有 `yue` code）。
#[test]
fn language_hint_maps_supported_codes() {
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

// ── 假服务端：收尾流程（design D1 的三条硬规则） ────────────────────

/// `finish-task` 之后必须**继续收**尾部结果，直到 `task-finished`；而且
/// 不足一包的残包要在 `finish-task` **之前**发出去。
///
/// 用本地假 WebSocket 服务端跑真实链路（`connect → send_audio → finish`）：
/// 服务端收到 `finish-task` 后才发两条尾部定稿，最后才发 `task-finished`。
/// 实现若「发完就关连接/停止读」，第二条定稿必然收不到 → 红。
#[test]
fn finish_keeps_reading_and_flushes_leftover_packet() {
    use std::sync::{Arc, Mutex};

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();

    let leftover_before_finish = Arc::new(Mutex::new(0usize));
    let counter = leftover_before_finish.clone();

    let server = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async move {
            let listener = tokio::net::TcpListener::from_std(listener).unwrap();
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            while let Some(message) = socket.next().await {
                let Ok(message) = message else { break };
                match message {
                    Message::Text(text) => {
                        let Ok(value) = serde_json::from_str::<Value>(&text) else {
                            continue;
                        };
                        match value["header"]["action"].as_str().unwrap_or("") {
                            "run-task" => {
                                let started = json!({
                                    "header": { "event": "task-started", "attributes": {} },
                                    "payload": {},
                                });
                                socket
                                    .send(Message::Text(started.to_string().into()))
                                    .await
                                    .unwrap();
                            }
                            "finish-task" => {
                                // 两条尾部结果：只有「继续读」的实现才能都收到。
                                for (index, text) in ["尾句一", "尾句二"].iter().enumerate() {
                                    let payload = json!({
                                        "header": { "event": "result-generated" },
                                        "payload": { "output": { "sentence": {
                                            "sentence_id": index as i64 + 1,
                                            "sentence_begin": false,
                                            "sentence_end": true,
                                            "text": text,
                                            "heartbeat": false,
                                        } } },
                                    });
                                    socket
                                        .send(Message::Text(payload.to_string().into()))
                                        .await
                                        .unwrap();
                                    tokio::time::sleep(Duration::from_millis(40)).await;
                                }
                                let finished = json!({
                                    "header": { "event": "task-finished", "attributes": {} },
                                    "payload": { "output": {}, "usage": null },
                                });
                                socket
                                    .send(Message::Text(finished.to_string().into()))
                                    .await
                                    .unwrap();
                                break;
                            }
                            _ => {}
                        }
                    }
                    // 音频帧：只统计「finish-task 之前」收到了几个。
                    Message::Binary(_) => {
                        *counter.lock().unwrap() += 1;
                    }
                    _ => {}
                }
            }
        });
    });

    let mut provider = AliyunProvider::new(AliyunConfig {
        base_url: format!("ws://{addr}"),
        api_key: "sk-test".into(),
        model: String::new(),
        language_hint: None,
        context_text: None,
    });
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = events.clone();
    provider
        .connect(Box::new(move |event| sink.lock().unwrap().push(event)))
        .expect("应当连上假服务端");

    // 800 个样本 = 1600 字节 < 3200 → 留在缓冲里，收尾时才发（这就是残包）。
    provider.send_audio(&vec![100i16; 800]).unwrap();
    provider.finish().expect("收尾应当成功");
    server.join().unwrap();

    let finals: Vec<String> = events
        .lock()
        .unwrap()
        .iter()
        .filter_map(|event| match event {
            CloudEvent::Final { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        finals,
        vec!["尾句一".to_string(), "尾句二".to_string()],
        "finish-task 之后的尾部结果一条都不能丢"
    );
    assert_eq!(
        *leftover_before_finish.lock().unwrap(),
        1,
        "不足一包的残包必须在 finish-task 之前发出去（否则句尾少字）"
    );
}

// ── 真机冒烟 ────────────────────────────────────────────────────────

/// live_smoke：连上百炼 → 喂一段真实语音 → 收中间结果与定稿 → 收尾。
///
///   VOICE_NOTES_ALIYUN_KEY=... cargo test --manifest-path src-tauri/Cargo.toml \
///     --lib asr::cloud::aliyun::tests::live_smoke -- --nocapture
///
/// **没有环境变量就跳过**（打印一行「跳过」，不打印 key）。key 只从环境变量
/// 读 —— 不写进仓库、不进日志、不进断言消息。
///
/// 冒烟时顺带确认三件官方口径存疑的事（结果记进 `docs/HOW_TO_VERIFY.md`）：
/// ① 不传 `language_hints` 时是否真的自动识别多语种；
/// ② 同一句的连续 `text` 是否「越改越完整」而不是只在末尾追加；
/// ③ `heartbeat: true` 会不会产生额外事件。
#[test]
fn live_smoke() {
    let Ok(key) = std::env::var("VOICE_NOTES_ALIYUN_KEY") else {
        println!("跳过：未设置 VOICE_NOTES_ALIYUN_KEY");
        return;
    };

    // 真实语音 fixture（16 kHz / 单声道 / 5.6 秒）。
    let wave = sherpa_onnx::Wave::read("tests/fixtures/zh.wav").expect("读取测试音频");
    let samples: Vec<f32> = wave.samples().to_vec();
    assert_eq!(wave.sample_rate(), 16_000, "fixture 应该是 16 kHz");

    let config = AliyunConfig {
        base_url: String::new(),
        api_key: key,
        model: String::new(),
        // 不传语言：确认「自动识别」真的可用。
        language_hint: None,
        context_text: build_context(&["开放时间".to_string()]).0,
    };
    use crate::asr::cloud::{CloudStreamingSession, StreamEvent};
    use crate::asr::AsrSession;
    let mut session = CloudStreamingSession::new(Box::new(AliyunProvider::new(config)));
    session.connect().expect("应能连上百炼");

    let mut finals: Vec<String> = Vec::new();
    let mut partials: Vec<String> = Vec::new();
    let collect = |events: Vec<StreamEvent>, finals: &mut Vec<String>, partials: &mut Vec<String>| {
        for event in events {
            match event {
                StreamEvent::Partial(text) => partials.push(text),
                StreamEvent::Segment(segment) => finals.push(segment.text),
            }
        }
    };
    // 按 20 ms 一批喂进去，每批之后收一次后台事件。
    for part in samples.chunks(16_000 / 50) {
        session.advance_samples(part.len());
        collect(session.push(part), &mut finals, &mut partials);
        collect(session.poll(), &mut finals, &mut partials);
    }
    // 收尾：服务端断句族不需要 force_end，直接 finish 等 task-finished。
    collect(session.finish(), &mut finals, &mut partials);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while std::time::Instant::now() < deadline && finals.is_empty() {
        collect(session.poll(), &mut finals, &mut partials);
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
