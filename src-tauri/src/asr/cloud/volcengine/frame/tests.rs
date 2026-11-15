//! 二进制帧编解码的单测：按官方文档的字节布局逐位核对。

use super::*;

/// 头：版本 / header size / message type / flags / 序列化 / 压缩 / 保留。
#[test]
fn 头部按大端布局编码() {
    let header = encode_header(MESSAGE_TYPE_FULL_CLIENT_REQUEST, FLAG_NO_SEQUENCE, SERIALIZATION_JSON);
    assert_eq!(header[0], 0b0001_0001, "版本 1 + header size 4 字节");
    assert_eq!(header[1], 0b0001_0000, "full client request + 无 sequence");
    assert_eq!(header[2], 0b0001_0000, "JSON 序列化 + 不压缩");
    assert_eq!(header[3], 0x00, "保留字节");
}

/// 全量请求：头 + payload size(4B 大端) + 负载。
#[test]
fn 全量请求帧布局正确() {
    let payload = b"{\"request\":{}}";
    let frame = encode_full_request(payload);
    assert_eq!(frame[0], 0b0001_0001);
    assert_eq!(&frame[4..8], &(payload.len() as u32).to_be_bytes(), "payload size 大端");
    assert_eq!(&frame[8..], payload);
}

/// 音频包：末包用 flags 0b0010（负包），普通包用 0b0000。
#[test]
fn 音频包末包标志正确() {
    let normal = encode_audio(&[1, 2, 3], false);
    assert_eq!(normal[1], (MESSAGE_TYPE_AUDIO_ONLY_REQUEST << 4) | FLAG_NO_SEQUENCE);
    let last = encode_audio(&[1, 2, 3], true);
    assert_eq!(last[1], (MESSAGE_TYPE_AUDIO_ONLY_REQUEST << 4) | FLAG_LAST_NO_SEQUENCE);
}

/// 服务端响应：头 + sequence(4B) + payload size(4B) + 负载。
#[test]
fn 解析服务端响应() {
    let payload = b"{\"result\":{}}";
    let mut frame = encode_header(MESSAGE_TYPE_FULL_SERVER_RESPONSE, FLAG_POSITIVE_SEQUENCE, SERIALIZATION_JSON).to_vec();
    frame.extend_from_slice(&7i32.to_be_bytes());
    frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    frame.extend_from_slice(payload);
    match decode_server_frame(&frame).expect("应能解析") {
        ServerFrame::Response { sequence, payload: body } => {
            assert_eq!(sequence, 7);
            assert_eq!(body, payload);
        }
        other => panic!("应是响应帧，实际 {other:?}"),
    }
}

/// 最终响应：sequence 为负数。
#[test]
fn 最终响应序列号为负() {
    let payload = b"{}";
    let mut frame = encode_header(MESSAGE_TYPE_FULL_SERVER_RESPONSE, FLAG_NEGATIVE_SEQUENCE, SERIALIZATION_JSON).to_vec();
    frame.extend_from_slice(&(-3i32).to_be_bytes());
    frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    frame.extend_from_slice(payload);
    match decode_server_frame(&frame).expect("应能解析") {
        ServerFrame::Response { sequence, .. } => assert_eq!(sequence, -3),
        other => panic!("应是响应帧，实际 {other:?}"),
    }
}

/// 错误帧：头 + 错误码(4B) + 长度(4B) + 文本。
#[test]
fn 解析错误帧() {
    let message = "invalid audio format";
    let mut frame = encode_header(MESSAGE_TYPE_ERROR_RESPONSE, FLAG_NO_SEQUENCE, SERIALIZATION_JSON).to_vec();
    frame.extend_from_slice(&45000151u32.to_be_bytes());
    frame.extend_from_slice(&(message.len() as u32).to_be_bytes());
    frame.extend_from_slice(message.as_bytes());
    match decode_server_frame(&frame).expect("应能解析") {
        ServerFrame::Error { code, message: text } => {
            assert_eq!(code, 45000151);
            assert_eq!(text, message);
        }
        other => panic!("应是错误帧，实际 {other:?}"),
    }
}

/// 截断的帧要报错，不能 panic。
#[test]
fn 截断帧报错而非崩溃() {
    assert!(decode_server_frame(&[0x11]).is_err(), "太短");
    let mut frame = encode_header(MESSAGE_TYPE_FULL_SERVER_RESPONSE, FLAG_POSITIVE_SEQUENCE, SERIALIZATION_JSON).to_vec();
    frame.extend_from_slice(&1i32.to_be_bytes());
    frame.extend_from_slice(&100u32.to_be_bytes()); // 声称 100 字节，实际没有
    assert!(decode_server_frame(&frame).is_err(), "payload 被截断");
}
