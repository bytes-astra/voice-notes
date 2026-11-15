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

/// delta 是增量：上抛给前端的必须是按 item 累积的文本。
#[test]
fn delta_accumulates_into_partial() {
    let (provider, state) = MockProvider::new(EndpointKind::ClientCommit, 16000);
    let mut session = CloudStreamingSession::new(Box::new(provider));
    session.connect().unwrap();
    emit(&state, CloudEvent::PartialDelta { item_id: Some("a".into()), delta: "你好".into() });
    emit(&state, CloudEvent::PartialDelta { item_id: Some("a".into()), delta: "，世界".into() });
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
    emit(&state, CloudEvent::Final { text: String::new(), start_ms: None, end_ms: None });
    emit(&state, CloudEvent::Final { text: "第二句".into(), start_ms: None, end_ms: None });
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

/// 热词中的协议非法字符必须丢弃，不能整包带出去。
#[test]
fn cloud_keywords_drop_invalid_chars() {
    let (clean, ignored) = clean_cloud_keywords(&["正常".into(), "C<plus>".into(), "换\n行".into()]);
    assert_eq!(clean, vec!["正常".to_string()]);
    assert_eq!(ignored, 2);
}

/// 没有本地 VAD 不能创建 Realtime provider：默认模型不支持 server_vad。
#[test]
fn provider_requires_local_vad() {
    let settings = crate::settings::Settings { cloud_provider: "realtime".into(), ..Default::default() };
    match create_provider(&settings, false) {
        Err(error) => assert_eq!(error, "cloud.vadMissing"),
        Ok(_) => panic!("没有 VAD 不应创建 provider"),
    }
}
