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
fn 解码增量事件() {
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
fn 解码定稿事件() {
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
fn 解码提交回执() {
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
fn 解码错误事件() {
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
fn 认不出来的帧忽略() {
    assert_eq!(decode_event("not json"), ServerEvent::Other);
    assert_eq!(decode_event(r#"{"type":"session.updated"}"#), ServerEvent::Other);
}

/// 音频帧：`input_audio_buffer.append` + base64。
#[test]
fn 编码音频帧() {
    let raw = encode_append("AAECAw==");
    let value: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(value["type"], "input_audio_buffer.append");
    assert_eq!(value["audio"], "AAECAw==");
}

/// 提交帧。
#[test]
fn 编码提交帧() {
    let value: Value = serde_json::from_str(&encode_commit()).unwrap();
    assert_eq!(value["type"], "input_audio_buffer.commit");
}

/// 会话配置帧：转写会话、24 kHz PCM、客户端断句、模型与热词。
#[test]
fn 编码会话配置帧() {
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
fn 编码会话配置帧_空语言不带字段() {
    let mut cfg = config();
    cfg.languages.clear();
    cfg.keywords.clear();
    let value: Value = serde_json::from_str(&encode_session_update(&cfg)).unwrap();
    let transcription = &value["session"]["audio"]["input"]["transcription"];
    assert!(transcription.get("languages").is_none());
    assert!(transcription.get("keywords").is_none());
}

/// final 按提交顺序上抛：后一轮先回来时要等。
#[test]
fn final按提交顺序上抛() {
    let mut order = FinalOrder::default();
    assert!(order.committed(Some("a".into())).is_empty());
    assert!(order.committed(Some("b".into())).is_empty());
    // 后一轮先回来：先存着。
    assert!(order.push(Some("b".into()), "第二句".into()).is_empty());
    // 前一轮到了：两句一起发，顺序按提交来。
    let events = order.push(Some("a".into()), "第一句".into());
    let texts: Vec<String> = events
        .iter()
        .map(|event| match event {
            CloudEvent::Final { text, .. } => text.clone(),
            _ => String::new(),
        })
        .collect();
    assert_eq!(texts, vec!["第一句".to_string(), "第二句".to_string()]);
    assert!(order.is_empty());
}

/// 没有 item_id 的 final：没法排序，直接发。
#[test]
fn 没有item_id的final直接发() {
    let mut order = FinalOrder::default();
    let events = order.push(None, "一句".into());
    assert_eq!(events.len(), 1);
}

/// 收尾记账：客户端断句 —— 提交后结果没回来就还有在等的东西。
#[test]
fn 收尾记账_客户端断句() {
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
fn 收尾记账_服务端断句() {
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

/// 服务端先给 final、后给 committed：那条回执不能再排队等（否则队列永远不空）。
#[test]
fn 迟到的committed回执不再排队() {
    let mut order = FinalOrder::default();
    // final 先到：直接发出去。
    let events = order.push(Some("item_1".into()), "一句".into());
    assert_eq!(events.len(), 1, "final 先到就该直接发");
    // 后到的 committed 回执：不能再排进队。
    assert!(order.committed(Some("item_1".into())).is_empty());
    assert!(order.is_empty(), "队列该是空的：否则收尾会一直等");
}

/// 凭据不能出现在错误详情里。
#[test]
fn 错误详情抹掉凭据() {
    let detail = sanitize("failed with key sk-abc123", "sk-abc123");
    assert!(!detail.contains("sk-abc123"), "详情里不该有凭据：{detail}");
    assert!(detail.contains("***"));
}
