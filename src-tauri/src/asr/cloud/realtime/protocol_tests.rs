//! Realtime 协议的编解码单测：用官方的示例帧当输入。

use super::*;

/// 一份够用的配置。
fn config() -> RealtimeConfig {
    RealtimeConfig {
        base_url: DEFAULT_BASE_URL.to_string(),
        api_key: "test-key".into(),
        model: DEFAULT_MODEL.to_string(),
        languages: vec!["zh".into()],
        keywords: vec!["玄戒芯片".into()],
    }
}

/// 增量事件：字段是 `delta`。
#[test]
fn decodes_delta_event() {
    let raw = r#"{"type":"conversation.item.input_audio_transcription.delta","item_id":"item_1","delta":"你好"}"#;
    assert_eq!(
        decode_event(raw),
        ServerEvent::Partial {
            item_id: Some("item_1".into()),
            delta: "你好".into(),
        }
    );
}

/// 定稿事件：字段是 `transcript`。
#[test]
fn decodes_completed_event() {
    let raw = r#"{"type":"conversation.item.input_audio_transcription.completed","item_id":"item_1","transcript":"你好，世界"}"#;
    assert_eq!(
        decode_event(raw),
        ServerEvent::Final {
            item_id: Some("item_1".into()),
            transcript: "你好，世界".into(),
        }
    );
}

/// 提交回执：带 `item_id`，用来给 final 排序。
#[test]
fn decodes_committed_event() {
    let raw = r#"{"type":"input_audio_buffer.committed","item_id":"item_1"}"#;
    assert_eq!(
        decode_event(raw),
        ServerEvent::Committed {
            item_id: Some("item_1".into()),
        }
    );
}

/// 错误事件。
#[test]
fn decodes_error_event() {
    let raw = r#"{"type":"error","error":{"message":"invalid api key"}}"#;
    assert_eq!(
        decode_event(raw),
        ServerEvent::Error {
            message: "invalid api key".into(),
        }
    );
}

/// 认不出来的帧不该当成错误：直接忽略。
#[test]
fn ignores_unknown_frames() {
    assert_eq!(decode_event("not json"), ServerEvent::Other);
    assert_eq!(
        decode_event(r#"{"type":"session.created"}"#),
        ServerEvent::Other
    );
}

/// 音频帧：`input_audio_buffer.append` + base64。
#[test]
fn encodes_append_frame() {
    let raw = encode_append("AAECAw==");
    let value: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(value["type"], "input_audio_buffer.append");
    assert_eq!(value["audio"], "AAECAw==");
}

/// 提交帧。
#[test]
fn encodes_commit_frame() {
    let value: Value = serde_json::from_str(&encode_commit()).unwrap();
    assert_eq!(value["type"], "input_audio_buffer.commit");
}

/// 会话配置帧：转写会话、24 kHz PCM、客户端断句、模型与热词。
#[test]
fn encodes_session_update() {
    let value: Value = serde_json::from_str(&encode_session_update(&config())).unwrap();
    assert_eq!(value["type"], "session.update");
    let session = &value["session"];
    assert_eq!(session["type"], "transcription");
    let input = &session["audio"]["input"];
    assert_eq!(input["format"]["type"], "audio/pcm");
    assert_eq!(input["format"]["rate"], 24000);
    // 断句由客户端决定：服务端那套要关掉。
    assert!(input["turn_detection"].is_null());
    assert_eq!(input["transcription"]["model"], DEFAULT_MODEL);
    // 语言参数是 `languages`（复数）。
    assert_eq!(input["transcription"]["languages"][0], "zh");
    assert_eq!(input["transcription"]["keywords"][0], "玄戒芯片");
}

/// 没给语言/热词时不发这两个字段（发了空数组也可能被服务当参数）。
#[test]
fn session_update_omits_empty_languages_and_keywords() {
    let mut cfg = config();
    cfg.languages.clear();
    cfg.keywords.clear();
    let value: Value = serde_json::from_str(&encode_session_update(&cfg)).unwrap();
    let transcription = &value["session"]["audio"]["input"]["transcription"];
    assert!(transcription.get("languages").is_none());
    assert!(transcription.get("keywords").is_none());
}

/// 结果按**本地提交顺序**上抛：后一轮先回来时要等前面那轮。
#[test]
fn finals_emitted_in_commit_order() {
    let mut tracker = CommitTracker::default();
    tracker.commit();
    tracker.commit();
    // 两轮回执到了：先发两条回执（会话层靠它把区间绑到 id 上）。
    let acks = tracker.committed(Some("a".into()));
    assert_eq!(acks.len(), 1, "先发第一条回执");
    assert!(matches!(acks[0], CloudEvent::CommitAck { .. }));
    tracker.committed(Some("b".into()));
    // 后一轮先回来：先存着。
    tracker.resolve(Some("b".into()), Outcome::Text("第二句".into()));
    assert!(tracker.flush().is_empty(), "前一轮没回来，不该上抛");
    // 前一轮到了：两句一起发，顺序按提交来。
    tracker.resolve(Some("a".into()), Outcome::Text("第一句".into()));
    let events = tracker.flush();
    let texts: Vec<String> = events
        .iter()
        .filter_map(|event| match event {
            CloudEvent::Final { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(texts, vec!["第一句".to_string(), "第二句".to_string()]);
    assert!(tracker.is_empty());
}

/// 没有 item_id 的 final：没法排序，直接发。
#[test]
fn final_without_item_id_is_emitted_directly() {
    let mut tracker = CommitTracker::default();
    tracker.commit();
    tracker.resolve(None, Outcome::Text("一句".into()));
    let events = tracker.flush();
    assert_eq!(events.len(), 1);
}

/// 收尾记账：客户端断句 —— 提交后结果没回来就还有在等的东西。
#[test]
fn pending_accounting_client_commit() {
    let mut pending = Pending::default();
    // 还没喂音频：没什么在等的。
    assert!(!pending.outstanding(true, true), "空会话不该等");
    // 喂了音频但还没提交：在等。
    pending.on_audio();
    assert!(pending.outstanding(true, true), "没提交的音频还在等");
    // 提交了，结果还没回来：还在等。
    pending.on_commit(true);
    assert!(pending.outstanding(true, true), "已提交的结果还没回来");
    // 结果到了：不等了。
    pending.on_final(&Some("item_1".into()), true);
    assert!(!pending.outstanding(true, true), "结果都回来了就不该等");
}

/// 收尾记账：服务端断句 —— 迟到的旧 final 不能抹掉新音频的等待。
#[test]
fn pending_accounting_server_endpoint() {
    let mut pending = Pending::default();
    pending.on_audio();
    assert!(pending.outstanding(false, true), "喂了音频就在等");
    // 另一件（更新的 item）先出了声。
    pending.note_item(&Some("item_2".into()));
    // 迟到的旧 final：不能把等待抹掉。
    pending.on_final(&Some("item_1".into()), false);
    assert!(
        pending.outstanding(false, true),
        "迟到的旧 final 不该抹掉新音频的等待"
    );
    // 最新那一件的 final 到了：可以交差了。
    pending.on_final(&Some("item_2".into()), false);
    assert!(!pending.outstanding(false, true), "最新结果到了就不该等");
}

/// 服务端先给 final、后给 committed：回执仍要发（会话层靠它绑区间），
/// 但这一轮不再排第二次（否则永远等不到）。
#[test]
fn late_committed_receipt_is_not_queued() {
    let mut tracker = CommitTracker::default();
    tracker.commit();
    // final 先到：回执 + 结果一起上抛（回执在前：会话层先绑区间）。
    tracker.resolve(Some("item_1".into()), Outcome::Text("一句".into()));
    let events = tracker.flush();
    assert_eq!(events.len(), 2, "回执 + 结果");
    assert!(matches!(events[0], CloudEvent::CommitAck { .. }));
    // 后到的 committed 回执：这一轮已经出结果了，不该再排队。
    let events = tracker.committed(Some("item_1".into()));
    assert!(events.is_empty());
    assert!(tracker.is_empty(), "该是空的：否则收尾会一直等");
}

/// 凭据不能出现在错误详情里。
#[test]
fn error_detail_redacts_secret() {
    let detail = super::super::sanitize("failed with key sk-abc123", "sk-abc123");
    assert!(!detail.contains("sk-abc123"), "详情里不该有凭据：{detail}");
    assert!(detail.contains("***"));
}

/// 失败事件必须解码，不能落到 Other 让 FinalOrder 永久堵死。
/// 服务端给的原因要一起带出来（只进日志）。
#[test]
fn decodes_failed_event() {
    let raw = r#"{"type":"conversation.item.input_audio_transcription.failed","item_id":"item_1","error":{"type":"server_error","code":"audio_too_short","message":"Audio is too short"}}"#;
    assert_eq!(
        decode_event(raw),
        ServerEvent::Failed {
            item_id: Some("item_1".into()),
            reason: "Audio is too short".into(),
        }
    );
}

/// 失败事件没有原因时也不能当成解析失败。
#[test]
fn failed_event_without_reason_decodes() {
    let raw = r#"{"type":"conversation.item.input_audio_transcription.failed","item_id":"item_1"}"#;
    assert_eq!(
        decode_event(raw),
        ServerEvent::Failed {
            item_id: Some("item_1".into()),
            reason: String::new(),
        }
    );
}

/// 控制帧有空位时直接进队。
#[test]
fn control_frame_enqueues_when_queue_has_room() {
    let (tx, mut rx) = tokio_mpsc::channel(4);
    send_control(&tx, Command::Commit, Duration::from_millis(50)).unwrap();
    assert!(matches!(rx.try_recv(), Ok(Command::Commit)));
}

/// 控制帧排不进去时要**有界**返回，不能无限阻塞录音线程。
#[test]
fn control_frame_times_out_when_queue_full() {
    let (tx, _rx) = tokio_mpsc::channel(1);
    tx.try_send(Command::Commit).unwrap();
    let started = std::time::Instant::now();
    let error = send_control(&tx, Command::Commit, Duration::from_millis(50)).unwrap_err();
    assert_eq!(error, "cloud.sendFailed");
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "等待必须贴着 timeout 封顶：{:?}",
        started.elapsed()
    );
}

/// 队列让出空位后重试成功，而且送进去的仍是**原来那条**命令。
#[test]
fn control_frame_retries_after_queue_drains() {
    let (tx, mut rx) = tokio_mpsc::channel(1);
    tx.try_send(Command::Commit).unwrap();
    let drainer = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(30));
        // 腾出一个空位，重试的那条应该立刻进来。
        let _ = rx.try_recv();
        std::thread::sleep(Duration::from_millis(100));
        matches!(rx.try_recv(), Ok(Command::Commit))
    });
    send_control(&tx, Command::Commit, Duration::from_secs(2)).unwrap();
    assert!(drainer.join().unwrap(), "重试送进去的应是原来那条 Commit");
}

/// 后台任务没了：控制帧直接报断连，不空等。
#[test]
fn control_frame_reports_disconnect_when_closed() {
    let (tx, rx) = tokio_mpsc::channel(1);
    drop(rx);
    assert_eq!(
        send_control(&tx, Command::Commit, Duration::from_millis(50)).unwrap_err(),
        "cloud.disconnected"
    );
}

/// 配置确认事件：connect 要等它才能报告成功。
#[test]
fn decodes_session_updated_event() {
    assert_eq!(
        decode_event(r#"{"type":"session.updated"}"#),
        ServerEvent::SessionUpdated
    );
}

/// 失败的 item 用空 final 占住提交队列里的位置：它后面那些**先到**的
/// final 必须按提交顺序跟着放行，不能卡在队列里等超时。
#[test]
fn failed_item_releases_later_final_in_commit_order() {
    let mut tracker = CommitTracker::default();
    tracker.commit();
    tracker.commit();
    tracker.committed(Some("a".into()));
    tracker.committed(Some("b".into()));
    // 后一轮的结果先回来：排在 a 后面等着。
    tracker.resolve(Some("b".into()), Outcome::Text("第二句".into()));
    assert!(tracker.flush().is_empty());
    // a 失败：走同一条通道，a 的空 final 与 b 一起放行。
    tracker.resolve(Some("a".into()), Outcome::Failed);
    let events = tracker.flush();
    let texts: Vec<String> = events
        .iter()
        .filter_map(|event| match event {
            CloudEvent::Final { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        texts,
        vec![String::new(), "第二句".to_string()],
        "顺序必须是 a（空）→ b"
    );
    assert!(tracker.is_empty());
}

/// 默认模型永远不发 server_vad：句边界恒为客户端提交。
///
/// 合并自两处：能力自报（`CAPABILITIES.endpoint_kind`）与
/// `encodes_session_update` 的 `turn_detection` 为 null 断言。
#[test]
fn default_model_never_sends_server_vad() {
    assert_eq!(
        RealtimeProvider::CAPABILITIES.endpoint_kind,
        EndpointKind::ClientCommit
    );
    let value: Value = serde_json::from_str(&encode_session_update(&config())).unwrap();
    assert!(value["session"]["audio"]["input"]["turn_detection"].is_null());
}

/// 服务端错误里的 key 也必须抹掉。
#[test]
fn server_error_redacts_secret() {
    let message = "401 unauthorized for sk-abc123";
    let detail = super::super::sanitize(message, "sk-abc123");
    assert!(!detail.contains("sk-abc123"));
    assert!(detail.contains("401"));
}

/// 外发热词按 Realtime 协议过滤：`<`、`>`、CR、LF 不能进 keywords。
#[test]
fn clean_cloud_keywords_drops_protocol_illegal_chars() {
    let (clean, ignored) =
        clean_cloud_keywords(&["正常".into(), "C<plus>".into(), "换\n行".into()]);
    assert_eq!(clean, vec!["正常".to_string()]);
    assert_eq!(ignored, 2);
}

/// 服务端报错时提交槽必须被解开：不解决它，后面所有 final 都会被扣住
/// （转写里静默少了后文），收尾还要白等 10 秒。
#[test]
fn error_releases_the_head_commit_slot() {
    let mut tracker = CommitTracker::default();
    tracker.commit();
    tracker.committed(Some("a".into()));
    // 服务端报错：把队首按「失败」解决掉。
    tracker.resolve(None, Outcome::Failed);
    let events = tracker.flush();
    assert_eq!(events.len(), 1, "失败的槽要放出来");
    assert!(tracker.is_empty(), "队列不该还悬着");
    // 后续的提交与结果要能继续流动。
    tracker.commit();
    tracker.committed(Some("b".into()));
    tracker.resolve(Some("b".into()), Outcome::Text("后面这句".into()));
    let events = tracker.flush();
    let texts: Vec<String> = events
        .iter()
        .filter_map(|event| match event {
            CloudEvent::Final { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(texts, vec!["后面这句".to_string()]);
}

// ── provider 契约守卫（RC-K1 / RC-K2）────────────────────────────────────
//
// 两条不变量：
// - RC-K1：声明的 `partial_semantics` 与协议实际形态一致；
// - RC-K2：上抛的中间结果是当前未定稿句的**累计文本**（句内整体替换，不追加）。
//
// 守卫要躲开三种「恒真」写法（OpenSpec change `cloud-contract-guards` 的
// design D2）：① 期望值不能从样例里再读一遍；② 第二次更新必须**改写**第一次
// （对增量协议不成立 —— `.delta` 按协议**就是**后缀增量，伪造「改写型」样例
// 反而会让守卫测不到东西）；③ 形态要从**样例本身**推导。

/// 两条增量帧：**同一个 `item_id`** —— 不同 id 会开两个桶，第二次 `apply`
/// 返回的只是半句，看起来像「追加丢了」。
const DELTA_FRAMES: [&str; 2] = [
    r#"{"type":"conversation.item.input_audio_transcription.delta","item_id":"item_1","delta":"你好"}"#,
    r#"{"type":"conversation.item.input_audio_transcription.delta","item_id":"item_1","delta":"，世界"}"#,
];

/// 从两条 `.delta` 推导协议形态。
///
/// **不接收任何声明值** —— 推导结果一旦被 `CAPABILITIES` 污染，断言就恒真。
///
/// 判据：第二次的 delta **不含**第一次的内容（若是整句覆盖，第二次必须带着
/// 整句），而累计结果**保留**了第一次 → 后缀增量（`Delta`）；否则整句替换。
fn derive_shape(first_delta: &str, second_delta: &str, accumulated: &str) -> PartialSemantics {
    if !second_delta.contains(first_delta) && accumulated.starts_with(first_delta) {
        PartialSemantics::Delta
    } else {
        PartialSemantics::Preview
    }
}

/// 契约守卫：Realtime 的中间结果是**后缀增量**，累计串是当前句的全文；
/// 声明的形态与样例相符。
#[test]
fn deltas_accumulate_into_the_sentence_so_far() {
    // 归一器用**硬编码的 `Delta`** 构造：不能拿声明值来构造 ——
    // `partial.rs` 的兜底分支（形态不匹配也照收）会让错误的声明照样通过。
    let mut accumulator = partial::PartialAccumulator::new(PartialSemantics::Delta);
    let mut deltas: Vec<String> = Vec::new();
    let mut accumulated: Vec<String> = Vec::new();
    for raw in DELTA_FRAMES {
        let delta = match decode_event(raw) {
            ServerEvent::Partial { item_id, delta } => {
                assert_eq!(
                    item_id.as_deref(),
                    Some("item_1"),
                    "realtime/Delta: 两条 delta 帧必须用同一个 item_id"
                );
                delta
            }
            other => panic!("realtime/Delta: 该解出增量事件，实际是 {other:?}"),
        };
        deltas.push(delta.clone());
        accumulated
            .push(accumulator.apply(Some("item_1".into()), partial::PartialUpdate::Delta(delta)));
    }
    // 期望值是**手写字面量**。
    assert_eq!(
        accumulated[0], "你好",
        "realtime/Delta: 第一次累计串应是第一条 delta"
    );
    assert_eq!(
        accumulated[1], "你好，世界",
        "realtime/Delta: 累计串应是当前句的全文（不是只留最后一段）"
    );
    // 形态推导 → 与声明值比较（不是只钉住常量）。
    let derived = derive_shape(&deltas[0], &deltas[1], &accumulated[1]);
    assert_eq!(
        RealtimeProvider::CAPABILITIES.partial_semantics,
        derived,
        "realtime/{:?}: 声明的形态与协议实际形态不符",
        derived
    );
}

/// 契约守卫（**生产链路**）：Realtime 的中间结果是**当前句的累计串**。
///
/// 这条守卫跑**真实 run 循环** —— 本地假 WebSocket 服务端回两帧 `.delta`
/// （同一个 `item_id`），断言 provider 上抛的 `CloudEvent::Partial` 是
/// 「你好」与「你好，世界」。
///
/// **为什么不能只在测试里自建归一器**：那样只守住「归一器自己」，守不住
/// 「provider → 归一器 → 上抛」这段接线 —— 生产调用点被绕开时不会有任何
/// 守卫变红（实测过：把 `realtime.rs` 的 `accumulator.apply(...)` 改成
/// `let text = delta;`，276 条测试全绿）。
#[test]
fn deltas_accumulate_on_the_production_path() {
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
            // 先等客户端的 session.update，回 session.updated（connect 在等它）。
            while let Some(message) = socket.next().await {
                let Ok(message) = message else { break };
                let Message::Text(text) = message else {
                    continue;
                };
                let Ok(value) = serde_json::from_str::<Value>(&text) else {
                    continue;
                };
                if value["type"].as_str() == Some("session.update") {
                    socket
                        .send(Message::Text(
                            json!({ "type": "session.updated" }).to_string().into(),
                        ))
                        .await
                        .unwrap();
                    break;
                }
            }
            // 两帧 delta：**同一个 item_id** —— 不同 id 会开两个桶，第二次
            // 上抛的只是半句。
            for delta in ["你好", "，世界"] {
                let frame = json!({
                    "type": "conversation.item.input_audio_transcription.delta",
                    "item_id": "item_1",
                    "delta": delta,
                });
                socket
                    .send(Message::Text(frame.to_string().into()))
                    .await
                    .unwrap();
                tokio::time::sleep(Duration::from_millis(40)).await;
            }
            // 让客户端把两帧都读完。
            tokio::time::sleep(Duration::from_millis(200)).await;
        });
    });

    let config = RealtimeConfig {
        base_url: format!("ws://{addr}/v1/realtime?intent=transcription"),
        api_key: "test-key".into(),
        model: DEFAULT_MODEL.to_string(),
        languages: vec!["zh".into()],
        keywords: vec![],
    };
    let (tx, rx) = std::sync::mpsc::channel();
    let mut provider = RealtimeProvider::new(config);
    provider
        .connect(Box::new(move |event| {
            let _ = tx.send(event);
        }))
        .expect("应能连上假服务端");

    let mut partials: Vec<String> = Vec::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while std::time::Instant::now() < deadline && partials.len() < 2 {
        if let Ok(CloudEvent::Partial(text)) = rx.recv_timeout(Duration::from_millis(200)) {
            partials.push(text);
        }
    }
    let _ = server.join();

    // 期望值是**手写字面量**。
    assert_eq!(
        partials,
        vec!["你好".to_string(), "你好，世界".to_string()],
        "realtime/Delta: 上抛的中间结果必须是当前句的累计串（生产链路）"
    );
}
