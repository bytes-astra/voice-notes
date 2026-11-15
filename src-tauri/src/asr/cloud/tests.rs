//! 云端会话的单测：用一个假 provider 把协议那头摘掉。

use super::mock::{emit, MockProvider};
use super::*;

/// f32 → i16：越界的幅度要夹住。
#[test]
fn f32_to_i16_clamps_out_of_range() {
    let pcm = to_i16(&[1.5, -1.5, 0.0, 0.5, -0.5]);
    assert_eq!(pcm[0], 32767, "超过 1 要夹到上限");
    assert_eq!(pcm[1], -32768, "低于 -1 要夹到下限");
    assert_eq!(pcm[2], 0);
    assert_eq!(pcm[3], 16384);
    assert_eq!(pcm[4], -16384);
}

/// 时钟只按 16 kHz 采样数推进：丢掉的音频同样占录音时间。
#[test]
fn feeding_audio_advances_timeline() {
    let (provider, _state) = MockProvider::new(EndpointKind::ClientCommit, 16000);
    let mut session = CloudStreamingSession::new(Box::new(provider));
    session.connect().unwrap();
    assert_eq!(session.advance_samples(16000), 16000);
    assert!((session.elapsed_seconds() - 1.0).abs() < 1e-6);
    // 丢掉一段：时钟照走。
    session.advance_dropped(8000);
    assert_eq!(session.advance_samples(0), 24000);
    assert!((session.elapsed_seconds() - 1.5).abs() < 1e-6);
}

/// final → Segment：句子时间由本地时钟给出。
#[test]
fn final_maps_to_segment() {
    let (provider, state) = MockProvider::new(EndpointKind::ClientCommit, 16000);
    let mut session = CloudStreamingSession::new(Box::new(provider));
    session.connect().unwrap();
    session.advance_samples(16000);
    session.push(&[0.2; 16000]);
    // 静音 → 提交这一段。
    session.force_end();
    assert_eq!(state.lock().unwrap().commits, 1, "静音要提交一次");
    emit(
        &state,
        CloudEvent::Final {
            item_id: None,
            text: "你好".into(),
            start_ms: None,
            end_ms: None,
        },
    );
    let events = session.finish();
    let segment = match events.first() {
        Some(StreamEvent::Segment(segment)) => segment.clone(),
        other => panic!("该来一条句子，实际是 {other:?}"),
    };
    assert_eq!(segment.text, "你好");
    assert!((segment.start - 0.0).abs() < 1e-6, "起点该是本地轴：{segment:?}");
    assert!((segment.end - 1.0).abs() < 1e-6, "终点该是本地轴：{segment:?}");
}

/// provider 给的时间戳只是增强：不许覆盖本地轴。
#[test]
fn provider_timestamps_do_not_override_local_timeline() {
    let (provider, state) = MockProvider::new(EndpointKind::ServerEndpoint, 16000);
    let mut session = CloudStreamingSession::new(Box::new(provider));
    session.connect().unwrap();
    session.advance_samples(16000);
    session.push(&[0.2; 16000]);
    emit(
        &state,
        CloudEvent::Final {
            item_id: None,
            text: "你好".into(),
            // provider 说是 5 秒到 9 秒：本地只说了 1 秒。
            start_ms: Some(5000),
            end_ms: Some(9000),
        },
    );
    let events = session.finish();
    let segment = match events.first() {
        Some(StreamEvent::Segment(segment)) => segment.clone(),
        other => panic!("该来一条句子，实际是 {other:?}"),
    };
    assert!((segment.start - 0.0).abs() < 1e-6, "起点不许被 provider 覆盖：{segment:?}");
    assert!((segment.end - 1.0).abs() < 1e-6, "终点不许被 provider 覆盖：{segment:?}");
}

/// 服务端自己断句的 provider：客户端不提交。
#[test]
fn server_endpoint_provider_does_not_commit() {
    let (provider, state) = MockProvider::new(EndpointKind::ServerEndpoint, 16000);
    let mut session = CloudStreamingSession::new(Box::new(provider));
    session.connect().unwrap();
    session.advance_samples(16000);
    session.push(&[0.2; 16000]);
    session.force_end();
    assert_eq!(state.lock().unwrap().commits, 0, "服务端断句时客户端不提交");
}

/// provider 要 24 kHz：上传前升采样，本地时间轴仍是 16 kHz。
#[test]
fn upsamples_before_upload_when_provider_wants_24k() {
    let (provider, state) = MockProvider::new(EndpointKind::ClientCommit, 24000);
    let mut session = CloudStreamingSession::new(Box::new(provider));
    session.connect().unwrap();
    session.advance_samples(16000);
    session.push(&[0.2; 16000]);
    let sent = state.lock().unwrap().sent.len();
    assert!(
        sent > 16000,
        "1 秒的 16 kHz 音频升到 24 kHz 后不该还是 16000 个样本：{sent}"
    );
    assert!((session.elapsed_seconds() - 1.0).abs() < 1e-6, "本地轴不受上传采样率影响");
}

/// 空 final 也要把区间配掉：否则下一句会配到上一句的区间上（时间戳整段错位）。
#[test]
fn empty_final_does_not_steal_next_span() {
    let (provider, state) = MockProvider::new(EndpointKind::ClientCommit, 16000);
    let mut session = CloudStreamingSession::new(Box::new(provider));
    session.connect().unwrap();
    // 第一句：喂 1 秒 → 静音提交。
    session.advance_samples(16000);
    session.push(&[0.2; 16000]);
    session.force_end();
    // 服务端回了一条空的（这段没认出内容）。
    emit(
        &state,
        CloudEvent::Final {
            item_id: None,
            text: "   ".into(),
            start_ms: None,
            end_ms: None,
        },
    );
    // 第二句：再喂 1 秒 → 静音提交。
    session.advance_samples(16000);
    session.push(&[0.2; 16000]);
    session.force_end();
    emit(
        &state,
        CloudEvent::Final {
            item_id: None,
            text: "第二句".into(),
            start_ms: None,
            end_ms: None,
        },
    );
    let events = session.finish();
    let segment = match events.first() {
        Some(StreamEvent::Segment(segment)) => segment.clone(),
        other => panic!("该来一条句子，实际是 {other:?}"),
    };
    assert_eq!(segment.text, "第二句");
    assert!(
        (segment.start - 1.0).abs() < 1e-6 && (segment.end - 2.0).abs() < 1e-6,
        "第二句该落在 1.0~2.0 秒（空句子占了第一句的区间）：{segment:?}"
    );
}

/// 断连：已定稿的句子留着，错误另外报。
#[test]
fn disconnect_keeps_finalized_segments() {
    let (provider, state) = MockProvider::new(EndpointKind::ClientCommit, 16000);
    let mut session = CloudStreamingSession::new(Box::new(provider));
    session.connect().unwrap();
    session.advance_samples(16000);
    session.push(&[0.2; 16000]);
    session.force_end();
    emit(
        &state,
        CloudEvent::Final {
            item_id: None,
            text: "第一句".into(),
            start_ms: None,
            end_ms: None,
        },
    );
    // 之后连接断了。
    state.lock().unwrap().fail = true;
    session.advance_samples(16000);
    session.push(&[0.2; 16000]);
    session.finish();
    let transcript = session.transcript();
    assert_eq!(transcript.text, "第一句", "断连不能把已定稿的句子弄丢");
    assert_eq!(transcript.segments.len(), 1);
    assert!(
        transcript.warning.is_some(),
        "断连要留下错误：{:?}",
        transcript.warning
    );
}

/// 中间结果由 provider 归一为「当前句的累计串」，会话层原样上抛。
///
/// 归一本身在 `partial::PartialAccumulator` 里测（见 `partial/tests.rs`）；
/// 这里守的是「会话层不再自己累加」。
#[test]
fn delta_accumulates_into_partial() {
    let (provider, state) = MockProvider::new(EndpointKind::ClientCommit, 16000);
    let mut session = CloudStreamingSession::new(Box::new(provider));
    session.connect().unwrap();
    emit(&state, CloudEvent::Partial("你好".into()));
    emit(&state, CloudEvent::Partial("你好，世界".into()));
    let events = session.poll();
    let partials: Vec<String> = events.iter().filter_map(|e| match e { StreamEvent::Partial(t) => Some(t.clone()), _ => None }).collect();
    assert_eq!(partials, vec!["你好".to_string(), "你好，世界".to_string()]);
}

/// 失败的 item 产生空 final，只弹自己的区间，后续句子仍能出来。
#[test]
fn failed_item_does_not_block_later_finals() {
    let (provider, state) = MockProvider::new(EndpointKind::ClientCommit, 16000);
    let mut session = CloudStreamingSession::new(Box::new(provider));
    session.connect().unwrap();
    session.advance_samples(16000);
    session.push(&[0.2; 16000]);
    session.force_end();
    session.advance_samples(16000);
    session.push(&[0.2; 16000]);
    session.force_end();
    emit(&state, CloudEvent::Final { item_id: None, text: String::new(), start_ms: None, end_ms: None });
    emit(&state, CloudEvent::Final { item_id: None, text: "第二句".into(), start_ms: None, end_ms: None });
    let events = session.poll();
    let texts: Vec<String> = events.iter().filter_map(|e| match e { StreamEvent::Segment(s) => Some(s.text.clone()), _ => None }).collect();
    assert_eq!(texts, vec!["第二句".to_string()]);
    let segment = events.iter().find_map(|e| match e { StreamEvent::Segment(s) => Some(s), _ => None }).unwrap();
    assert!((segment.start - 1.0).abs() < 1e-6);
}

/// 提交失败时不能先登记区间：服务端没收到这次 commit，两段音频会在下一次
/// 提交时并成一条 final —— 区间也要合成一段，否则失败点以后每个句子的
/// 时间戳都错位一格。
#[test]
fn failed_commit_merges_span_into_next_commit() {
    let (provider, state) = MockProvider::new(EndpointKind::ClientCommit, 16000);
    let mut session = CloudStreamingSession::new(Box::new(provider));
    session.connect().unwrap();
    // 第一句：0~1 秒，commit 失败（没送出去）。
    session.advance_samples(16000);
    session.push(&[0.2; 16000]);
    state.lock().unwrap().fail_commit = true;
    session.force_end();
    assert_eq!(state.lock().unwrap().commits, 0, "失败的那次不算提交成功");
    // 第二句：1~2 秒，commit 成功；服务端把两段合成一条结果。
    state.lock().unwrap().fail_commit = false;
    session.advance_samples(16000);
    session.push(&[0.2; 16000]);
    session.force_end();
    assert_eq!(state.lock().unwrap().commits, 1);
    emit(
        &state,
        CloudEvent::Final {
            item_id: None,
            text: "第一句第二句".into(),
            start_ms: None,
            end_ms: None,
        },
    );
    let merged = first_segment(session.poll());
    assert!(
        (merged.start - 0.0).abs() < 1e-6 && (merged.end - 2.0).abs() < 1e-6,
        "合并后的句子该覆盖 0~2 秒：{merged:?}"
    );
    // 第三句：2~3 秒，另一条正常的 final。
    session.advance_samples(16000);
    session.push(&[0.2; 16000]);
    session.force_end();
    emit(
        &state,
        CloudEvent::Final {
            item_id: None,
            text: "第三句".into(),
            start_ms: None,
            end_ms: None,
        },
    );
    let third = first_segment(session.poll());
    assert!(
        (third.start - 2.0).abs() < 1e-6 && (third.end - 3.0).abs() < 1e-6,
        "第三句该落在 2~3 秒（失败点之后不能整体错位）：{third:?}"
    );
}

/// 事件里第一条句子。
fn first_segment(events: Vec<StreamEvent>) -> Segment {
    events
        .into_iter()
        .find_map(|event| match event {
            StreamEvent::Segment(segment) => Some(segment),
            _ => None,
        })
        .expect("该来一条句子")
}



/// 能力表：客户端提交族声明「需要本地 VAD」，且能力组合合法。
#[test]
fn provider_requires_local_vad() {
    let caps = provider_capabilities("realtime").expect("realtime 应有能力项");
    assert_eq!(caps.endpoint_kind, EndpointKind::ClientCommit);
    assert!(caps.needs_local_vad, "客户端提交族要本地 VAD 断句");
    assert!(valid_capabilities(&caps), "能力组合应合法");
    assert!(provider_capabilities("不存在的服务").is_none(), "未知 id 没有能力项");
}

/// 能力组合非法时拒绝建 provider（避免「缺 VAD 被读成服务端断句」）。
#[test]
fn invalid_capability_combination_is_rejected() {
    let bad = Capabilities {
        endpoint_kind: EndpointKind::ClientCommit,
        needs_local_vad: false,
        partial_semantics: PartialSemantics::Delta,
        supports_keywords: true,
        target_sample_rate: 16000,
    };
    assert!(!valid_capabilities(&bad), "客户端提交族不声明需要 VAD 是非法组合");
}


/// final 乱序到达时：句子时间不能串位，文本顺序也要按本地时间轴。
///
/// 区间按 `item_id` 绑定（不是按到达顺序）—— 服务端先回后一轮的 completed
/// 是协议允许的。
#[test]
fn 乱序final不串位() {
    let (provider, state) = MockProvider::new(EndpointKind::ClientCommit, 16000);
    let mut session = CloudStreamingSession::new(Box::new(provider));
    session.connect().unwrap();
    session.advance_samples(16000);
    session.push(&[0.2; 16000]);
    session.force_end();
    session.advance_samples(16000);
    session.push(&[0.2; 16000]);
    session.force_end();
    // 回执按提交顺序到，区间依次绑到 a / b。
    emit(&state, CloudEvent::CommitAck { item_id: Some("a".into()) });
    emit(&state, CloudEvent::CommitAck { item_id: Some("b".into()) });
    // 结果乱序到：b 先到。
    emit(
        &state,
        CloudEvent::Final {
            item_id: Some("b".into()),
            text: "第二句".into(),
            start_ms: None,
            end_ms: None,
        },
    );
    emit(
        &state,
        CloudEvent::Final {
            item_id: Some("a".into()),
            text: "第一句".into(),
            start_ms: None,
            end_ms: None,
        },
    );
    session.poll();
    let transcript = session.transcript();
    assert_eq!(transcript.text, "第一句第二句", "文本顺序要按本地时间轴");
    assert_eq!(transcript.segments.len(), 2);
    assert!(
        (transcript.segments[0].start - 0.0).abs() < 1e-6
            && (transcript.segments[0].end - 1.0).abs() < 1e-6,
        "第一句该是 0.0~1.0：{:?}",
        transcript.segments[0]
    );
    assert!(
        (transcript.segments[1].start - 1.0).abs() < 1e-6
            && (transcript.segments[1].end - 2.0).abs() < 1e-6,
        "第二句该是 1.0~2.0：{:?}",
        transcript.segments[1]
    );
}

/// 失败的 item 没有回执时（回执晚到或没到），后面的句子也不能串位：
/// 失败事件自带 item_id，回执随后照常补上。
#[test]
fn 失败item无回执不串位() {
    let (provider, state) = MockProvider::new(EndpointKind::ClientCommit, 16000);
    let mut session = CloudStreamingSession::new(Box::new(provider));
    session.connect().unwrap();
    session.advance_samples(16000);
    session.push(&[0.2; 16000]);
    session.force_end();
    session.advance_samples(16000);
    session.push(&[0.2; 16000]);
    session.force_end();
    // 第一轮失败：provider 先补回执（会话层靠它把区间绑上去），再上空 final。
    emit(&state, CloudEvent::CommitAck { item_id: Some("a".into()) });
    emit(
        &state,
        CloudEvent::Final {
            item_id: Some("a".into()),
            text: String::new(),
            start_ms: None,
            end_ms: None,
        },
    );
    emit(&state, CloudEvent::CommitAck { item_id: Some("b".into()) });
    emit(
        &state,
        CloudEvent::Final {
            item_id: Some("b".into()),
            text: "第二句".into(),
            start_ms: None,
            end_ms: None,
        },
    );
    session.poll();
    let transcript = session.transcript();
    assert_eq!(transcript.text, "第二句");
    assert_eq!(transcript.segments.len(), 1);
    assert!(
        (transcript.segments[0].start - 1.0).abs() < 1e-6,
        "第二句该是 1.0~2.0（第一句的区间已被失败那一轮弹掉）：{:?}",
        transcript.segments[0]
    );
}

/// 上传丢包要留下缺口标记，并记下它发生在哪一秒。
#[test]
fn 上传丢包记下缺口位置() {
    let (provider, state) = MockProvider::new(EndpointKind::ClientCommit, 16000);
    // 第一次发送就丢 1 秒。
    state.lock().unwrap().dropped_reply = 16_000;
    let mut session = CloudStreamingSession::new(Box::new(provider));
    session.connect().unwrap();
    session.advance_samples(16000);
    session.push(&[0.2; 16000]);
    let gaps = session.take_upload_gaps();
    assert_eq!(gaps.len(), 1, "丢包要有缺口：{gaps:?}");
    assert_eq!(gaps[0].1, 16_000, "丢掉的采样数要对");
    assert!(gaps[0].0 < 0.01, "缺口起点是这段音频的起点：{gaps:?}");
}


/// 提交失败之后又喂了音频，收尾那条 final 的区间要覆盖到最后 ——
/// 服务端把两段音频合成一条结果，区间也必须合成一段。
#[test]
fn 提交失败后收尾区间覆盖到最后() {
    let (provider, state) = MockProvider::new(EndpointKind::ClientCommit, 16000);
    {
        let mut guard = state.lock().unwrap();
        guard.fail_commit = true;
        guard.reply_on_finish = Some("合并的一句".into());
    }
    let mut session = CloudStreamingSession::new(Box::new(provider));
    session.connect().unwrap();
    session.advance_samples(16000);
    session.push(&[0.2; 16000]);
    // 这一句提交失败：区间攒着，等下一次提交（收尾那次）一起登记。
    session.force_end();
    // 又喂了 1 秒，没再触发静音收尾。
    session.advance_samples(16000);
    session.push(&[0.2; 16000]);
    let events = session.finish();
    let segment = events
        .iter()
        .find_map(|event| match event {
            StreamEvent::Segment(segment) => Some(segment.clone()),
            _ => None,
        })
        .expect("该有一条句子");
    assert!(
        (segment.start - 0.0).abs() < 1e-6 && (segment.end - 2.0).abs() < 1e-6,
        "收尾这条要覆盖 0.0~2.0 秒：{segment:?}"
    );
}

/// 能力表与 provider 实例自报的值必须一致（两处漂移会让状态与行为分叉）。
#[test]
fn 能力表与实例一致() {
    for id in crate::settings::CLOUD_PROVIDERS {
        let caps = provider_capabilities(id).unwrap_or_else(|| panic!("{id} 应有能力项"));
        let provider = create_provider(&crate::settings::Settings {
            cloud_provider: id.into(),
            ..Default::default()
        })
        .unwrap_or_else(|error| panic!("{id} 应能建出 provider：{error}"));
        assert_eq!(provider.capabilities(), caps, "{id} 的表项与实例不一致");
    }
}

/// 未知 provider：没有能力项，也建不出 provider。
#[test]
fn 未知provider建不出来() {
    assert!(provider_capabilities("不存在的服务").is_none());
    let settings = crate::settings::Settings {
        cloud_provider: "不存在的服务".into(),
        ..Default::default()
    };
    assert!(create_provider(&settings).is_err(), "未知 provider 不该建出来");
}

/// 能力声明不支持术语表时，一个词都不带（系统不得向它发送术语表）。
#[test]
fn 能力不支持术语表时不带词() {
    let settings = crate::settings::Settings {
        hotwords: vec!["玄戒芯片".into()],
        ..Default::default()
    };
    let caps = Capabilities {
        endpoint_kind: EndpointKind::ClientCommit,
        needs_local_vad: true,
        partial_semantics: PartialSemantics::Delta,
        supports_keywords: false,
        target_sample_rate: 16_000,
    };
    let (words, ignored) = cloud_keywords(&settings, &caps);
    assert!(words.is_empty(), "不支持就不该带词：{words:?}");
    assert_eq!(ignored, 1, "被丢弃的词数要报出来");
}
