//! 阿里 `qwen-audio-3.1-asr-flash-streaming` 的协议层单测：请求组装、事件解码、结果映射。

use super::*;

fn config() -> AliyunConfig {
    AliyunConfig {
        base_url: String::new(),
        api_key: "sk-test".into(),
        model: String::new(),
        language_hint: None,
        vocabulary: Vec::new(),
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
    assert_eq!(value["payload"]["model"], "qwen-audio-3.1-asr-flash-streaming");
    assert_eq!(value["payload"]["parameters"]["format"], "pcm");
    assert_eq!(value["payload"]["parameters"]["sample_rate"], 16_000);
    assert_eq!(value["payload"]["parameters"]["max_sentence_silence"], 800);
    assert_eq!(value["payload"]["parameters"]["heartbeat"], true);
    // 保留方言表达（录音工具要用户实际说的话）+ 近场 VAD（桌面麦克风）。
    assert_eq!(value["payload"]["parameters"]["keep_dialect"], true);
    assert_eq!(value["payload"]["parameters"]["vad_model"], "near_meeting_16k");
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
fn run_task_sends_request_level_hotwords() {
    let mut cfg = config();
    cfg.vocabulary = vec![("祺元".into(), 4), ("洛柯".into(), 5)];
    let value: Value = serde_json::from_str(&encode_run_task(&cfg, "t")).unwrap();
    // 即时热词：键值对（词 → 权重），请求级直传，不需要预建词表。
    assert_eq!(value["payload"]["parameters"]["vocabulary"]["祺元"], 4);
    assert_eq!(value["payload"]["parameters"]["vocabulary"]["洛柯"], 5);
    // 本 provider 不带上下文：`input` 恒为 `{}`。
    assert_eq!(value["payload"]["input"], serde_json::json!({}));
}

#[test]
fn finish_task_reuses_task_id_with_empty_input() {
    let value: Value = serde_json::from_str(&encode_finish_task("task-9")).unwrap();
    assert_eq!(value["header"]["action"], "finish-task");
    assert_eq!(value["header"]["task_id"], "task-9");
    assert_eq!(value["payload"]["input"], serde_json::json!({}));
}

// ── 即时热词的组装与过滤 ──────────────────────────────────────────────

#[test]
fn empty_glossary_means_no_hotwords() {
    let (vocabulary, ignored) = build_vocabulary(&[]);
    assert!(vocabulary.is_empty());
    assert_eq!(ignored, 0);
    // 全是空白也算空。
    let (vocabulary, ignored) = build_vocabulary(&["  ".into(), "".into()]);
    assert!(vocabulary.is_empty());
    assert_eq!(ignored, 0);
}

#[test]
fn glossary_becomes_weighted_hotwords() {
    let words = vec!["祺元".to_string(), "洛柯".to_string()];
    let (vocabulary, ignored) = build_vocabulary(&words);
    assert_eq!(
        vocabulary,
        vec![("祺元".to_string(), DEFAULT_HOTWORD_WEIGHT), ("洛柯".to_string(), DEFAULT_HOTWORD_WEIGHT)]
    );
    assert_eq!(ignored, 0);
}

#[test]
fn over_limit_drops_whole_hotwords() {
    let words: Vec<String> = (0..MAX_VOCABULARY + 3).map(|i| format!("词{i}")).collect();
    let (vocabulary, ignored) = build_vocabulary(&words);
    assert_eq!(ignored, 3, "超出的三条整条被丢弃");
    assert_eq!(vocabulary.len(), MAX_VOCABULARY);
    // 丢的是**末尾**的整条，不是把最后一个词截成半个。
    assert_eq!(vocabulary.last().unwrap().0, format!("词{}", MAX_VOCABULARY - 1));
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
            assert_eq!(sentence.id, Some(1));
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
    sentence_with(Some(id), end, text, "", None)
}

/// 造一条句子：`id` 为 `None` 表示服务端没给 `sentence_id`。
fn sentence_with(id: Option<i64>, end: bool, text: &str, draft: &str, draft_id: Option<i64>) -> Sentence {
    Sentence {
        id,
        end,
        text: text.into(),
        draft: draft.into(),
        draft_id,
        heartbeat: false,
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

/// 造一条 `result-generated` 的**原始协议 JSON**（官方形状：
/// `payload.output.sentence`，草稿在 `sentence.stash` 里）。
fn result_generated(sentence_id: i64, text: &str, stash_text: &str, stash_id: i64) -> String {
    json!({
        "header": { "event": "result-generated" },
        "payload": { "output": { "sentence": {
            "sentence_id": sentence_id,
            "sentence_end": false,
            "text": text,
            "heartbeat": false,
            "stash": { "sentence_id": stash_id, "text": stash_text },
        } } },
    })
    .to_string()
}

/// 走**真实协议链路**：JSON → `decode_server_event` → `parse_sentence` →
/// `AliyunMapper::map_sentence`。
///
/// **不能手工造 `Sentence` 结构体**：那会绕过协议解析，解析那一段就没有
/// 守卫了（实测：把 `parse_sentence` 里的 stash 解析改成 `String::new()`，
/// 276 条测试全绿）。
fn map_protocol_json(mapper: &mut AliyunMapper, raw: &str) -> Vec<CloudEvent> {
    let ServerEvent::Sentence(sentence) = decode_server_event(raw).expect("协议帧应能解码") else {
        panic!("该解出句子事件");
    };
    mapper.map_sentence(sentence)
}

/// 实时行文本 = **已确认的前缀 + 草稿**（本模型的结果结构：sentence + stash）。
///
/// 第二次的样例刻意用「**改写**第一次」（更短），这样「追加」实现会得到
/// 「你好世界你好呀」而不是「你好呀」—— 前缀型样例区分不出这两种实现。
///
/// 这也是阿里的 **RC-K1 / RC-K2 契约守卫**（登记在 `docs/REVIEW-CHECKLIST.md`）。
#[test]
fn partial_is_confirmed_prefix_plus_draft() {
    let mut mapper = AliyunMapper::new();
    let first = map_protocol_json(&mut mapper, &result_generated(1, "你好世界", "", 2));
    assert_eq!(partials(&first), vec!["你好世界".to_string()]);

    // 确认前缀变短 + 草稿有内容 → 实时行 = 前缀 + 草稿。
    let second = map_protocol_json(&mut mapper, &result_generated(1, "你好", "呀", 2));
    assert_eq!(
        partials(&second),
        vec!["你好呀".to_string()],
        "aliyun/Preview: 第二次上抛的中间结果应**改写**第一次（整句替换，不是追加）"
    );
}

/// 断言二：**从样例推导形态**，再与能力声明比较。
///
/// 推导只看样例文本（不接收 `PartialSemantics`），所以把声明改错时这条会红。
#[test]
fn declared_partial_semantics_matches_samples() {
    let mut mapper = AliyunMapper::new();
    let first = partials(&map_protocol_json(
        &mut mapper,
        &result_generated(1, "你好世界", "", 2),
    ));
    let second = partials(&map_protocol_json(&mut mapper, &result_generated(1, "你好", "呀", 2)));
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
        "aliyun/{:?}: 声明的形态与协议实际形态不符", derived
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
        id: Some(0),
        end: false,
        text: String::new(),
        draft: String::new(),
        draft_id: None,
        heartbeat: true,
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
    // 粤语：阿里的 language_hints 里没有 yue，靠 zh 模型自动识别。
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
        vocabulary: Vec::new(),
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
        // 术语表走即时热词（请求级直传，带权重）。
        vocabulary: build_vocabulary(&["开放时间".to_string()]).0,
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

/// `task-started` 之前收到 `task-failed`：`connect()` 要带回**真因**，
/// 不能只给笼统的 `cloud.connectFailed`。
///
/// 真实场景：key 无效或模型没开通时，服务端可能直接回 `task-failed` 而不是
/// 握手 401 —— 那时用户看到「连接失败」会去查网络，查错方向。
#[test]
fn task_failed_before_started_reports_the_real_error() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();

    let server = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async move {
            let listener = tokio::net::TcpListener::from_std(listener).unwrap();
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            // 收到 run-task 就直接回 task-failed（不发 task-started）。
            while let Some(Ok(message)) = socket.next().await {
                if matches!(message, Message::Text(_)) {
                    let failed = json!({
                        "header": {
                            "event": "task-failed",
                            "error_code": "InvalidApiKey",
                            "error_message": "Invalid API-key provided.",
                        },
                        "payload": {},
                    });
                    let _ = socket.send(Message::Text(failed.to_string().into())).await;
                    break;
                }
            }
        });
    });

    let mut provider = AliyunProvider::new(AliyunConfig {
        base_url: format!("ws://{addr}"),
        api_key: "sk-test".into(),
        model: String::new(),
        language_hint: None,
        vocabulary: Vec::new(),
    });
    let error = provider
        .connect(Box::new(|_| {}))
        .expect_err("task-failed 之后不该算连上");
    server.join().unwrap();

    assert!(error.starts_with("cloud.protocolError"), "{error}");
    assert!(error.contains("InvalidApiKey"), "要带回服务端给的错误码：{error}");
}

/// `finish-task` 之后服务端直接关连接（没有 `task-finished`）：
/// `finish()` 要立刻报 `cloud.disconnected`，而不是等满 10 秒超时。
#[test]
fn close_without_task_finished_reports_disconnected() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();

    let server = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async move {
            let listener = tokio::net::TcpListener::from_std(listener).unwrap();
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            while let Some(Ok(message)) = socket.next().await {
                let Message::Text(text) = message else { continue };
                let Ok(value) = serde_json::from_str::<Value>(&text) else {
                    continue;
                };
                match value["header"]["action"].as_str().unwrap_or("") {
                    "run-task" => {
                        let started = json!({
                            "header": { "event": "task-started", "attributes": {} },
                            "payload": {},
                        });
                        let _ = socket.send(Message::Text(started.to_string().into())).await;
                    }
                    "finish-task" => {
                        // 不发 task-finished，直接关。
                        let _ = socket.close(None).await;
                        break;
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
        vocabulary: Vec::new(),
    });
    provider
        .connect(Box::new(|_| {}))
        .expect("应当连上假服务端");
    let error = provider.finish().expect_err("没有 task-finished 就不算正常结束");
    server.join().unwrap();

    assert!(error.starts_with("cloud.disconnected"), "{error}");
}

// ── 缺 sentence_id 的容错 ────────────────────────────────────────────

/// 服务端**没给** `sentence_id` 时，两条不同的句子不能被当成同一句 ——
/// 否则第一句定稿后 id 进了去重集合，第二句会被静默丢掉。
#[test]
fn missing_sentence_id_does_not_swallow_the_next_sentence() {
    let mut mapper = AliyunMapper::new();
    // 第一句：没有 id。
    let first = mapper.map_sentence(sentence_with(None, false, "第一句", "", None));
    assert_eq!(partials(&first), vec!["第一句".to_string()]);
    let first_end = mapper.map_sentence(sentence_with(None, true, "第一句", "", None));
    assert_eq!(finals(&first_end), vec!["第一句".to_string()]);

    // 第二句：同样没有 id —— 不能被当成重复句丢掉。
    let second = mapper.map_sentence(sentence_with(None, false, "第二句", "", None));
    assert_eq!(
        partials(&second),
        vec!["第二句".to_string()],
        "缺 id 的第二句不能被当成重复句丢掉"
    );
    let second_end = mapper.map_sentence(sentence_with(None, true, "第二句", "", None));
    assert_eq!(finals(&second_end), vec!["第二句".to_string()]);
}

/// `id == 0` 但没带 `heartbeat` 标记：0 是心跳的 id，按**缺 id**处理，
/// 而且不能把正在打开的句子提前定稿。
#[test]
fn zero_id_without_heartbeat_is_treated_as_missing_id() {
    let mut mapper = AliyunMapper::new();
    mapper.map_sentence(sentence(1, false, "第一句"));
    // 心跳形状（id=0）但没标 heartbeat：不能提前定稿第一句。
    let beat = mapper.map_sentence(sentence_with(Some(0), false, "", "", None));
    assert!(
        finals(&beat).is_empty(),
        "id=0 的空事件不该把打开的句子定稿：{beat:?}"
    );
    // 第一句后面照常能定稿。
    let end = mapper.map_sentence(sentence(1, true, "第一句"));
    assert_eq!(finals(&end), vec!["第一句".to_string()]);
}

/// 缺 id 的句子里，「新句开始」标志负责切句：没有它就当作同一句的快照更新。
#[test]
fn missing_id_uses_sentence_begin_to_split() {
    let mut mapper = AliyunMapper::new();
    // 没有 id 的连续两条：第二条是同一句的快照更新（替换不追加），还是新句？
    // 缺 id 时**不自己切句**（宁可少切，也不要把两句并成一句）。
    let first = mapper.map_sentence(sentence_with(None, false, "说了一半", "", None));
    assert_eq!(partials(&first), vec!["说了一半".to_string()]);
    let update = mapper.map_sentence(sentence_with(None, false, "说完了", "", None));
    assert!(
        finals(&update).is_empty(),
        "缺 id 时不能自己切句：{update:?}"
    );
}

// ── 「已确认句子 + 草稿」的结果结构 ────────────────────────────────────

/// 句中就带草稿（草稿属于**下一句**）时：
///
/// - 不能提前定稿（那句还没结束）；
/// - 下一句的 id 不能被写进「已定稿」集合 —— 否则下一句整句被吞。
#[test]
fn draft_in_middle_does_not_finalize_early_or_swallow_next_sentence() {
    let mut mapper = AliyunMapper::new();
    // 句 1 还没结束，草稿已经是句 2 的了。
    let first = mapper.map_sentence(sentence_with(Some(1), false, "你好", "世界", Some(2)));
    assert_eq!(partials(&first), vec!["你好世界".to_string()]);
    assert!(finals(&first).is_empty(), "句还没结束不能定稿：{first:?}");

    // 句 1 继续：草稿还是句 2 的。
    let second = mapper.map_sentence(sentence_with(Some(1), false, "你好世界", "再见", Some(2)));
    assert_eq!(partials(&second), vec!["你好世界再见".to_string()]);
    assert!(
        finals(&second).is_empty(),
        "草稿的 id 与已确认句不同，不能被当成新句而提前定稿：{second:?}"
    );

    // 句 1 结束 → 只定稿句 1 一次，文本是已确认部分（不含草稿）。
    let end = mapper.map_sentence(sentence_with(Some(1), true, "你好世界。", "", None));
    assert_eq!(finals(&end), vec!["你好世界。".to_string()]);

    // 句 2 的真实事件**必须**还能出字（没被写进「已定稿」集合）。
    let third = mapper.map_sentence(sentence_with(Some(2), false, "再见下一句", "", None));
    assert_eq!(partials(&third), vec!["再见下一句".to_string()]);
    let fourth = mapper.map_sentence(sentence_with(Some(2), true, "再见下一句完", "", None));
    assert_eq!(finals(&fourth), vec!["再见下一句完".to_string()]);
}

/// 句结束但服务端没给已确认文本（内容还在草稿里）：不能拿**下一句的草稿**
/// 当本句的定稿发出去，也不能把下一句的 id 写进「已定稿」。
#[test]
fn end_without_confirmed_text_does_not_finalize_the_next_draft() {
    let mut mapper = AliyunMapper::new();
    // 句 1 没有已确认文本，只有句 2 的草稿。
    let first = mapper.map_sentence(sentence_with(Some(1), false, "", "下一句草稿", Some(2)));
    assert_eq!(partials(&first), vec!["下一句草稿".to_string()]);

    // 句 1 结束，仍没有文本 → 不产 Final（没有可定稿的内容）。
    let end = mapper.map_sentence(sentence_with(Some(1), true, "", "", None));
    assert!(
        finals(&end).is_empty(),
        "没有已确认文本就不该定稿，更不能拿下一句的草稿：{end:?}"
    );

    // 句 2 的真实事件必须还能出字。
    let third = mapper.map_sentence(sentence_with(Some(2), false, "下一句确认", "", None));
    assert_eq!(partials(&third), vec!["下一句确认".to_string()]);
    let fourth = mapper.map_sentence(sentence_with(Some(2), true, "下一句确认完", "", None));
    assert_eq!(finals(&fourth), vec!["下一句确认完".to_string()]);
}

/// 草稿所属的句子在句中就换了（草稿 id 变了）：不能拿新草稿提前定稿。
#[test]
fn changing_draft_id_does_not_finalize_the_draft() {
    let mut mapper = AliyunMapper::new();
    let first = mapper.map_sentence(sentence_with(Some(1), true, "第一句", "草二", Some(2)));
    assert_eq!(finals(&first), vec!["第一句".to_string()]);
    assert_eq!(partials(&first), vec!["草二".to_string()]);

    // 句 2 的事件里带的是句 3 的草稿。
    let second = mapper.map_sentence(sentence_with(Some(2), false, "", "草三", Some(3)));
    assert_eq!(partials(&second), vec!["草三".to_string()]);
    assert!(
        finals(&second).is_empty(),
        "草三属于句 3，不能在句 2 还没结束时把它定稿：{second:?}"
    );

    // 句 2 正常结束 → 只有它自己的定稿。
    let third = mapper.map_sentence(sentence_with(Some(2), true, "第二句", "", None));
    assert_eq!(finals(&third), vec!["第二句".to_string()]);
}

/// 英文的词间空格不能被吃掉：trim 只用来判空，拼接用原文。
#[test]
fn english_word_spacing_is_preserved() {
    let mut mapper = AliyunMapper::new();
    let events = mapper.map_sentence(sentence_with(Some(1), false, "hello ", "world", None));
    assert_eq!(partials(&events), vec!["hello world".to_string()]);
}

/// 热词文本长度按官方规范过滤：含非 ASCII 合计 ≤15 字符；纯 ASCII ≤7 片段。
#[test]
fn overlong_hotwords_are_dropped() {
    let long_cn = "玄".repeat(16);
    let (vocabulary, ignored) = build_vocabulary(&[long_cn]);
    assert!(vocabulary.is_empty(), "16 个中文字超出 15 字符上限");
    assert_eq!(ignored, 1);

    // 15 个中文字正好放得下。
    let (vocabulary, ignored) = build_vocabulary(&["玄".repeat(15)]);
    assert_eq!(vocabulary.len(), 1);
    assert_eq!(ignored, 0);

    // 纯 ASCII：按空格切分，8 个片段太多。
    let (vocabulary, ignored) = build_vocabulary(&["a b c d e f g h".to_string()]);
    assert!(vocabulary.is_empty());
    assert_eq!(ignored, 1);

    let (vocabulary, ignored) = build_vocabulary(&["a b c d e f g".to_string()]);
    assert_eq!(vocabulary.len(), 1);
    assert_eq!(ignored, 0);
}

/// **空文本的定稿不能占住去重键**：同 id 的后续真实文本必须还能出来。
///
/// 反例（2026-11-17 外部复审发现，已实测复现）：`finalized` 表无条件 `insert`，
/// 空文本也进去 → 早退条件只看「id 在不在表里」→ 同 id 的后续文本被整段丢弃，
/// 只留一条 WARN「已定稿的句子又收到了新的文本，已忽略」。
#[test]
fn empty_final_does_not_block_later_text_for_the_same_id() {
    let mut mapper = AliyunMapper::new();
    // 一条**空文本**的定稿（同 id）。
    let empty = mapper.map_sentence(sentence_with(Some(2), true, "", "", None));
    assert!(
        finals(&empty).is_empty(),
        "空文本不该产出定稿：{:?}",
        empty
    );
    // 同一个 id 的**真实文本**必须还能出来。
    let later = mapper.map_sentence(sentence_with(Some(2), false, "第二句", "", None));
    assert_eq!(
        partials(&later),
        vec!["第二句".to_string()],
        "aliyun/Preview: 空文本的定稿不能把同 id 的后续文本吃掉"
    );
}

/// **新句 + 空文本 + `sentence_end`**：上一句必须照样定稿。
///
/// 反例（同上）：「防御性定稿上一句」的判定被圈在 `!confirmed_empty` 里，
/// 而收尾分支又无条件清掉 `self.open` → 上一句既没有 `Final`，文本只剩一条
/// `Partial` —— 下一个 `Partial` 一覆盖就彻底消失。
#[test]
fn a_new_id_with_empty_text_still_finalizes_the_previous_sentence() {
    let mut mapper = AliyunMapper::new();
    let first = mapper.map_sentence(sentence_with(Some(1), false, "第一句", "", None));
    assert_eq!(partials(&first), vec!["第一句".to_string()]);

    // 新句（id=2）带空文本 + sentence_end：上一句必须定稿。
    let second = mapper.map_sentence(sentence_with(Some(2), true, "", "", None));
    assert_eq!(
        finals(&second),
        vec!["第一句".to_string()],
        "aliyun/Preview: 新句（哪怕文本为空）也要把上一句按快照定稿"
    );
}

/// **草稿没有 id 时按「属于本句」处理**：过期草稿不能被再拼一次。
///
/// 反例（2026-11-17 外部复审发现）：草稿的 `stash.sentence_id` 缺失时
/// `draft_id` 是 `None` → 「本句的旧草稿作废」的判��� `draft.id == raw_id`
/// 永远不成立 → 过期草稿长期留着 → 实时行把它再拼一次（文本重复）。
#[test]
fn a_draft_without_id_does_not_survive_the_next_update() {
    let mut mapper = AliyunMapper::new();
    // 第一句 + 一条**没有 id** 的草稿。
    let first = mapper.map_sentence(sentence_with(Some(1), false, "第一句", "旧草稿", None));
    assert_eq!(partials(&first), vec!["第一句旧草稿".to_string()]);
    // 本句的文本往前走 → 过期草稿必须作废。
    let second = mapper.map_sentence(sentence_with(Some(1), false, "第一句更长", "", None));
    assert_eq!(
        partials(&second),
        vec!["第一句更长".to_string()],
        "aliyun/Preview: 没有 id 的旧草稿不能被再拼一次"
    );
}

/// **缺 `sentence_id` + 空文本 + `sentence_end`** → 上一句必须照样定稿。
///
/// 反例（2026-11-17 第四份外部复审发现 —— 是「新句空文本」那条的另一半）：
/// 服务端违约（不发 `sentence_id`）时，这个事件**认不出是哪一句** → 上一句的
/// 快照被直接丢掉 → 它只剩一条 `Partial`，下一个 `Partial` 一覆盖就彻底消失。
#[test]
fn a_missing_id_empty_end_still_finalizes_the_previous_sentence() {
    let mut mapper = AliyunMapper::new();
    let first = mapper.map_sentence(sentence_with(Some(1), false, "第一句", "", None));
    assert_eq!(partials(&first), vec!["第一句".to_string()]);

    // 收尾事件：**没有 id**、没有文本。
    let second = mapper.map_sentence(sentence_with(None, true, "", "", None));
    assert_eq!(
        finals(&second),
        vec!["第一句".to_string()],
        "aliyun/Preview: 认不出是哪一句的收尾事件，也不能把上一句的快照丢掉"
    );
}
