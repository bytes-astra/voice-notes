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
fn out_of_order_final_keeps_its_span() {
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
fn failed_item_without_ack_keeps_spans_aligned() {
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
fn upload_drop_records_gap_position() {
    let (provider, state) = MockProvider::new(EndpointKind::ClientCommit, 16000);
    // 第一次发送就丢 1 秒。
    state.lock().unwrap().dropped_reply = 16_000;
    let mut session = CloudStreamingSession::new(Box::new(provider));
    session.connect().unwrap();
    session.advance_samples(16000);
    session.push(&[0.2; 16000]);
    // 丢包段要等「下一次成功发送」或收尾才闭合 —— 闭合前取不到。
    assert!(session.take_upload_gaps().is_empty(), "还没闭合就不该有缺口");
    session.finish();
    let gaps = session.take_upload_gaps();
    assert_eq!(gaps.len(), 1, "丢包要有缺口：{gaps:?}");
    assert_eq!(gaps[0].1, 16_000, "丢掉的采样数要对");
    assert_eq!(gaps[0].0, 0, "缺口起点是这段音频的起点（16 kHz 采样数）：{gaps:?}");
}


/// 提交失败之后又喂了音频，收尾那条 final 的区间要覆盖到最后 ——
/// 服务端把两段音频合成一条结果，区间也必须合成一段。
#[test]
fn failed_commit_merges_span_into_the_last_one() {
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
fn capability_table_matches_instances() {
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
fn unknown_provider_cannot_be_created() {
    assert!(provider_capabilities("不存在的服务").is_none());
    let settings = crate::settings::Settings {
        cloud_provider: "不存在的服务".into(),
        ..Default::default()
    };
    assert!(create_provider(&settings).is_err(), "未知 provider 不该建出来");
}

/// 能力声明不支持术语表时，一个词都不带（系统不得向它发送术语表）。
#[test]
fn no_keywords_when_capability_says_unsupported() {
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

/// 连续丢包要并成一条缺口：网络持续背压时逐条上报会刷出几百个
/// [[gap:1]]，而且每条都声称丢了 1 秒（累计时长自相矛盾）。
#[test]
fn consecutive_drops_merge_into_one_gap() {
    let (provider, state) = MockProvider::new(EndpointKind::ServerEndpoint, 16000);
    let mut session = CloudStreamingSession::new(Box::new(provider));
    session.connect().unwrap();
    // 连续两批都丢：第一批 0.8 秒，第二批 0.8 秒（接得上）。
    state.lock().unwrap().dropped_reply = 12_800;
    session.advance_samples(12_800);
    session.push(&[0.2; 12_800]);
    // **编排每轮都会取一次缺口** —— 累积状态必须跨这次取走存活，
    // 否则连续丢包永远并不成一条。
    assert!(session.take_upload_gaps().is_empty(), "还没闭合就不该有缺口");
    state.lock().unwrap().dropped_reply = 12_800;
    session.advance_samples(12_800);
    session.push(&[0.2; 12_800]);
    // 收尾闭合这段丢包。
    session.finish();
    let gaps = session.take_upload_gaps();
    assert_eq!(gaps.len(), 1, "连续的丢包要并成一条：{gaps:?}");
    assert_eq!(gaps[0].1, 25_600, "累计采样数要对");
}

/// provider 只是把音频攒进包里（一个包都没发出去）时，**不算投递**：
/// 正在累积的丢包段不能被提前闭合。
///
/// 这是火山的真实形态（60 ms 推送块攒成 200 ms 的包）。
#[test]
fn buffering_is_not_delivery_so_gap_stays_open() {
    let (provider, state) = MockProvider::new(EndpointKind::ServerEndpoint, 16000);
    {
        let mut guard = state.lock().unwrap();
        guard.dropped_reply = 12_800;
        guard.not_delivered = true;
    }
    let mut session = CloudStreamingSession::new(Box::new(provider));
    session.connect().unwrap();
    // 第一批：丢了 0.8 秒（且这次没投递）。
    session.advance_samples(12_800);
    session.push(&[0.2; 12_800]);
    // 第二批：又丢 0.8 秒，中间那次「攒包」不能把段闭合。
    state.lock().unwrap().dropped_reply = 12_800;
    session.advance_samples(12_800);
    session.push(&[0.2; 12_800]);
    session.finish();
    let gaps = session.take_upload_gaps();
    assert_eq!(gaps.len(), 1, "攒包不该把丢包段断开：{gaps:?}");
    assert_eq!(gaps[0].1, 25_600, "累计采样数要对");
}

/// 丢 → 成功发送 → 丢：必须是**两条**缺口（中间的投递把段断开了）。
#[test]
fn delivery_closes_gap_and_next_drop_opens_a_new_one() {
    let (provider, state) = MockProvider::new(EndpointKind::ServerEndpoint, 16000);
    let mut session = CloudStreamingSession::new(Box::new(provider));
    session.connect().unwrap();
    state.lock().unwrap().dropped_reply = 12_800;
    session.advance_samples(12_800);
    session.push(&[0.2; 12_800]);
    // 中间这次成功投递 → 上一段闭合。
    state.lock().unwrap().dropped_reply = 0;
    session.advance_samples(12_800);
    session.push(&[0.2; 12_800]);
    // 再丢一段。
    state.lock().unwrap().dropped_reply = 12_800;
    session.advance_samples(12_800);
    session.push(&[0.2; 12_800]);
    session.finish();
    let gaps = session.take_upload_gaps();
    assert_eq!(gaps.len(), 2, "投递成功把丢包段断开了：{gaps:?}");
    assert_eq!(gaps[0].1, 12_800);
    assert_eq!(gaps[1].1, 12_800);
}

/// 最后一句没定稿时（超时 / 断连），实时行上已经显示的文字不能凭空消失：
/// 收尾要按 partial 兜底落成句子。
#[test]
fn finish_turns_unfinalized_partial_into_sentence() {
    let (provider, state) = MockProvider::new(EndpointKind::ClientCommit, 16000);
    let mut session = CloudStreamingSession::new(Box::new(provider));
    session.connect().unwrap();
    session.advance_samples(16000);
    session.push(&[0.2; 16000]);
    session.force_end();
    // provider 一直没回 final，只有实时文本。
    emit(&state, CloudEvent::Partial("说了半句".into()));
    let events = session.finish();
    let segment = events
        .iter()
        .find_map(|event| match event {
            StreamEvent::Segment(segment) => Some(segment.clone()),
            _ => None,
        })
        .expect("未定的 partial 要落成句子");
    assert_eq!(segment.text, "说了半句");
}

/// 共享的凭据脱敏：空凭据原样返回。
#[test]
fn sanitize_keeps_text_when_secret_empty() {
    assert_eq!(sanitize("一些文本", ""), "一些文本");
}

/// 共享的凭据脱敏：多处出现要全部替换。
#[test]
fn sanitize_replaces_every_occurrence() {
    let out = sanitize("a sk-1 b sk-1 c", "sk-1");
    assert_eq!(out, "a *** b *** c");
    assert!(!out.contains("sk-1"));
}

/// 共享的凭据脱敏：凭据不在文本里时一个字都不动。
#[test]
fn sanitize_leaves_text_without_secret_alone() {
    let text = "上传失败：连接被重置";
    assert_eq!(sanitize(text, "sk-1"), text);
}

/// 阿里只支持白名单里的模型：协议骨架相同但参数与结果结构不同
/// （本模型有 stash 草稿、即时热词、方言开关），不按「名字随便填」处理。
#[test]
fn aliyun_rejects_models_outside_the_whitelist() {
    let settings = crate::settings::Settings {
        cloud_provider: "aliyun".into(),
        cloud_model: "some-other-asr-model".into(),
        ..Default::default()
    };
    let error = match create_provider(&settings) {
        Ok(_) => panic!("白名单外的模型不该建出来"),
        Err(error) => error,
    };
    assert!(error.starts_with("cloud.unsupportedModel"), "{error}");
}

/// 16k 的阿里模型能建出来（默认模型名也走这条）。
#[test]
fn aliyun_16k_model_builds() {
    for model in ["", "qwen-audio-3.1-asr-flash-streaming"] {
        let settings = crate::settings::Settings {
            cloud_provider: "aliyun".into(),
            cloud_model: model.into(),
            ..Default::default()
        };
        assert!(
            create_provider(&settings).is_ok(),
            "模型 {model:?} 应该能建出来"
        );
    }
}

// ── 复现用：同一 drain 批次里 Final 与 Partial 的先后顺序 ──────────────

/// 取最终转写里的句子文本（按本地时间轴排好序）。
fn segment_texts(transcript: Transcript) -> Vec<String> {
    transcript
        .segments
        .into_iter()
        .map(|segment| segment.text)
        .collect()
}

/// 复现：同一个 drain 批次里先到 Final、后到 Partial。
///
/// 服务端断句族不调 `force_end`，收尾时 `finish()` 才把区间推进 `pending`。
/// `drain()` 处理 Final 时先 `self.partial.clear()`，随后到达的 Partial 又把
/// `self.partial` 填上；`finish()` 末尾的兜底（「最后一句还没定稿」）于是再落
/// 一条 Segment。
///
/// 这条断言的是**当前实际行为**（两条句子），用来复现疑似缺陷。
#[test]
fn final_then_partial_in_one_batch_yields_extra_fallback_segment() {
    let (provider, state) = MockProvider::new(EndpointKind::ServerEndpoint, 16000);
    let mut session = CloudStreamingSession::new(Box::new(provider));
    session.connect().unwrap();
    session.advance_samples(16000);
    session.push(&[0.2; 16000]);
    // 同一个 drain 批次：Final 先入队，Partial 后入队。
    emit(
        &state,
        CloudEvent::Final {
            item_id: None,
            text: "完整的一句".into(),
            start_ms: None,
            end_ms: None,
        },
    );
    emit(&state, CloudEvent::Partial("说了一半".into()));
    session.finish();
    let texts = segment_texts(session.transcript());
    assert_eq!(
        texts,
        vec!["完整的一句".to_string(), "说了一半".to_string()],
        "Final 之后的 Partial 被 finish 兜底成了第二条句子"
    );
}

/// 常态顺序（同一批次里 Partial 先、Final 后）：Final 清掉 partial，
/// 不会在 finish 时多出兜底句子。
#[test]
fn partial_then_final_in_one_batch_yields_one_segment() {
    let (provider, state) = MockProvider::new(EndpointKind::ServerEndpoint, 16000);
    let mut session = CloudStreamingSession::new(Box::new(provider));
    session.connect().unwrap();
    session.advance_samples(16000);
    session.push(&[0.2; 16000]);
    emit(&state, CloudEvent::Partial("说了一半".into()));
    emit(
        &state,
        CloudEvent::Final {
            item_id: None,
            text: "完整的一句".into(),
            start_ms: None,
            end_ms: None,
        },
    );
    session.finish();
    let texts = segment_texts(session.transcript());
    assert_eq!(
        texts,
        vec!["完整的一句".to_string()],
        "Partial 先到、Final 后到时不能多出重复句"
    );
}

/// 走真实的 `AliyunMapper`：服务端直接开新句时，映射器先发上一句的 Final、
/// 再发新句的 Partial —— 正是「Final 先、Partial 后」。
///
/// 这里两条文本属于**不同句子**，所以 finish 的兜底落的是新句的实时文本，
/// 不是重复句。
#[test]
fn aliyun_new_sentence_partial_after_final_is_not_a_duplicate() {
    let (provider, state) = MockProvider::new(EndpointKind::ServerEndpoint, 16000);
    let mut session = CloudStreamingSession::new(Box::new(provider));
    session.connect().unwrap();
    session.advance_samples(16000);
    session.push(&[0.2; 16000]);
    let sentence = |id: i64, end: bool, text: &str| aliyun::Sentence {
        id: Some(id),
        end,
        text: text.into(),
        draft: String::new(),
        draft_id: None,
        heartbeat: false,
    };
    let mut mapper = aliyun::AliyunMapper::new();
    for event in mapper.map_sentence(sentence(1, false, "第一句")) {
        emit(&state, event);
    }
    // 服务端没给第一句的 end，直接开新句：映射器先定稿第一句，再给新句 partial。
    for event in mapper.map_sentence(sentence(2, false, "第二句半句")) {
        emit(&state, event);
    }
    session.finish();
    let texts = segment_texts(session.transcript());
    assert_eq!(
        texts,
        vec!["第一句".to_string(), "第二句半句".to_string()],
        "新句的 partial 被兜底落成句子（是另一句，不是重复）"
    );
}

/// 只有在**同一句**的迟到 Partial 也到达会话层时，兜底才会把已定稿的句子
/// 原样再落一遍。阿里的映射器用 `finalized` 把这种事件挡在 provider 内部
/// （见 `aliyun/tests.rs` 的 `finalized_sentence_ignores_later_events`），
/// 这里在会话层直接造出这个形态，确认会话层自身没有这层护栏。
#[test]
fn stale_partial_for_a_finalized_sentence_would_be_duplicated() {
    let (provider, state) = MockProvider::new(EndpointKind::ServerEndpoint, 16000);
    let mut session = CloudStreamingSession::new(Box::new(provider));
    session.connect().unwrap();
    session.advance_samples(16000);
    session.push(&[0.2; 16000]);
    emit(
        &state,
        CloudEvent::Final {
            item_id: None,
            text: "完整的一句".into(),
            start_ms: None,
            end_ms: None,
        },
    );
    emit(&state, CloudEvent::Partial("完整的一句".into()));
    session.finish();
    let texts = segment_texts(session.transcript());
    assert_eq!(
        texts,
        vec!["完整的一句".to_string(), "完整的一句".to_string()],
        "同句迟到 partial 会被 finish 兜底成重复句（会话层无护栏）"
    );
}

/// 映射器侧护栏：同一句定稿之后，服务端再补一条 Partial，映射器直接丢弃，
/// 不会把它送进会话层 —— 这是「重复句」在生产路径上到不了会话层的原因。
#[test]
fn aliyun_mapper_drops_late_partial_for_finalized_sentence() {
    let sentence = |id: i64, end: bool, text: &str| aliyun::Sentence {
        id: Some(id),
        end,
        text: text.into(),
        draft: String::new(),
        draft_id: None,
        heartbeat: false,
    };
    let mut mapper = aliyun::AliyunMapper::new();
    let first = mapper.map_sentence(sentence(1, false, "完整的一句"));
    assert!(
        matches!(first.first(), Some(CloudEvent::Partial(_))),
        "首条应是 Partial：{first:?}"
    );
    let finalized = mapper.map_sentence(sentence(1, true, "完整的一句"));
    assert!(
        finalized
            .iter()
            .any(|event| matches!(event, CloudEvent::Final { .. })),
        "end 要产一条 Final：{finalized:?}"
    );
    let late = mapper.map_sentence(sentence(1, false, "完整的一句"));
    assert!(late.is_empty(), "定稿后的同句 partial 必须被丢弃：{late:?}");
}

/// 同批多句的零长区间：句子之间的先后以**到达顺序**为准。
///
/// 服务端断句族在收尾时一次性 drain 出多条 final 时，第一条占用收尾那段
/// 区间，第 2 条起退化为零长（同 `start`）—— 这时唯一可用的顺序就是到达顺序。
///
/// **这是行为守卫，不能证明内部一定用稳定排序** —— 它只钉住「同 `start` 时
/// 顺序等于到达顺序」这个可观察行为。
#[test]
fn zero_length_segments_keep_arrival_order() {
    let (provider, state) = MockProvider::new(EndpointKind::ServerEndpoint, 16000);
    let mut session = CloudStreamingSession::new(Box::new(provider));
    session.connect().unwrap();
    // **先推进时钟再喂音频**：`fed_end` 只在 `push_at` 里更新 ——
    // 只 `advance_samples` 的话所有 Final 全是零长，测不到「后续句退化」。
    session.advance_samples(16000);
    session.push(&[0.2; 16000]);
    // 连续九条 final、**只 poll 一次**（同一个 drain 批次）。
    // 文本的到达顺序与代码点序、长度都不一致 —— 否则按任何别的字段排序都可能
    // 碰巧通过（八条零长句才够：三条时不稳定排序很可能碰巧保留顺序）。
    let texts = ["丙", "甲甲", "乙", "庚", "丁丁", "戊", "壬壬", "癸", "己"];
    for text in texts {
        emit(
            &state,
            CloudEvent::Final {
                item_id: None,
                text: text.into(),
                start_ms: None,
                end_ms: None,
            },
        );
    }
    session.poll();
    let transcript = session.transcript();
    assert_eq!(
        transcript.segments.len(),
        texts.len(),
        "九条句子都要在：{:?}",
        transcript.segments
    );
    // 第一条占用收尾那段区间。
    assert!(
        transcript.segments[0].start < transcript.segments[0].end,
        "第一条该占一段区间：{:?}",
        transcript.segments[0]
    );
    // 后八条零长，且 `start` 相同。
    for segment in &transcript.segments[1..] {
        assert_eq!(segment.start, segment.end, "后续句该是零长：{segment:?}");
        assert_eq!(
            segment.start, transcript.segments[1].start,
            "零长句的 start 该相同：{segment:?}"
        );
    }
    // 文本顺序 = 到达顺序。
    assert_eq!(
        transcript.text, "丙甲甲乙庚丁丁戊壬壬癸己",
        "同批零长句的先后以到达顺序为准"
    );
}
