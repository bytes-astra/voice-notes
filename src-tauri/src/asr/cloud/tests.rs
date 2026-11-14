//! 云端会话的单测：用一个假 provider 把协议那头摘掉。

use super::mock::{emit, MockProvider};
use super::*;

/// f32 → i16：越界的幅度要夹住。
#[test]
fn f32转i16夹住越界() {
    let pcm = to_i16(&[1.5, -1.5, 0.0, 0.5, -0.5]);
    assert_eq!(pcm[0], 32767, "超过 1 要夹到上限");
    assert_eq!(pcm[1], -32768, "低于 -1 要夹到下限");
    assert_eq!(pcm[2], 0);
    assert_eq!(pcm[3], 16384);
    assert_eq!(pcm[4], -16384);
}

/// 时钟只按 16 kHz 采样数推进：丢掉的音频同样占录音时间。
#[test]
fn 喂音频推进时间轴() {
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
fn final映射成句子() {
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
fn provider时间戳不覆盖本地轴() {
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
fn 服务端断句的provider不提交() {
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
fn provider要24k时上传前升采样() {
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
fn 空final不抢下一句的区间() {
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
fn 断连保留已定稿句子() {
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
