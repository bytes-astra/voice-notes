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
        local_vad: true,
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
    assert_eq!(decode_event(r#"{"type":"session.created"}"#), ServerEvent::Other);
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
    let detail = sanitize("failed with key sk-abc123", "sk-abc123");
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
    assert_eq!(decode_event(r#"{"type":"session.updated"}"#), ServerEvent::SessionUpdated);
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
    assert_eq!(texts, vec![String::new(), "第二句".to_string()], "顺序必须是 a（空）→ b");
    assert!(tracker.is_empty());
}

/// 默认模型永远不发 server_vad。
#[test]
fn default_model_never_sends_server_vad() {
    let mut cfg = config();
    cfg.local_vad = false;
    let value: Value = serde_json::from_str(&encode_session_update(&cfg)).unwrap();
    assert!(value["session"]["audio"]["input"]["turn_detection"].is_null());
}

/// 服务端错误里的 key 也必须抹掉。
#[test]
fn server_error_redacts_secret() {
    let message = "401 unauthorized for sk-abc123";
    let detail = sanitize(message, "sk-abc123");
    assert!(!detail.contains("sk-abc123"));
    assert!(detail.contains("401"));
}
