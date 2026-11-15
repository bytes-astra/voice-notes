//! 火山（豆包）双向流式 ASR 的二进制帧编解码。
//!
//! 协议（官方文档 2026-11 核对）：WebSocket frame 的 payload 里装的是
//! **自研二进制协议** —— 4 字节头 + payload size(4B) + payload，整数一律大端。
//!
//! ```text
//! byte 0: [7..4] 协议版本(0b0001)   [3..0] header size(×4)
//! byte 1: [7..4] message type        [3..0] message type specific flags
//! byte 2: [7..4] 序列化(0=无,1=JSON)  [3..0] 压缩(0=无,1=gzip)
//! byte 3: 保留(0x00)
//! 之后: [可选 sequence(4B)] + payload size(4B) + payload
//! ```
//!
//! **压缩选「不压缩」**：协议允许（服务端跟随客户端的选择），省掉一个依赖；
//! 带宽 = 16 kHz PCM16 = 32 KB/s。

/// 首次全量请求（带请求参数）。
pub const MESSAGE_TYPE_FULL_CLIENT_REQUEST: u8 = 0b0001;
/// 音频包。
pub const MESSAGE_TYPE_AUDIO_ONLY_REQUEST: u8 = 0b0010;
/// 服务端响应。
pub const MESSAGE_TYPE_FULL_SERVER_RESPONSE: u8 = 0b1001;
/// 服务端错误帧。
pub const MESSAGE_TYPE_ERROR_RESPONSE: u8 = 0b1111;

/// flags：header 后 4 字节不是 sequence。
pub const FLAG_NO_SEQUENCE: u8 = 0b0000;
/// flags：header 后 4 字节是**正数** sequence。
#[allow(dead_code)]
pub const FLAG_POSITIVE_SEQUENCE: u8 = 0b0001;
/// flags：header 后 4 字节不是 sequence，仅指示这是**最后一包**（负包）。
pub const FLAG_LAST_NO_SEQUENCE: u8 = 0b0010;
/// flags：header 后 4 字节是**负数** sequence（最后一包）。
#[allow(dead_code)]
pub const FLAG_NEGATIVE_SEQUENCE: u8 = 0b0011;

/// 序列化：原始字节。
pub const SERIALIZATION_NONE: u8 = 0b0000;
/// 序列化：JSON。
pub const SERIALIZATION_JSON: u8 = 0b0001;

/// 压缩：不压缩。
pub const COMPRESSION_NONE: u8 = 0b0000;

/// 4 字节头。
fn encode_header(message_type: u8, flags: u8, serialization: u8) -> [u8; 4] {
    [
        // 版本 0b0001 + header size 0b0001（×4 = 4 字节）
        0b0001_0001,
        (message_type << 4) | (flags & 0x0f),
        (serialization << 4) | COMPRESSION_NONE,
        0x00,
    ]
}

/// 首次全量请求：头 + payload size + JSON 负载。
pub fn encode_full_request(payload: &[u8]) -> Vec<u8> {
    let mut out = encode_header(
        MESSAGE_TYPE_FULL_CLIENT_REQUEST,
        FLAG_NO_SEQUENCE,
        SERIALIZATION_JSON,
    )
    .to_vec();
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(payload);
    out
}

/// 音频包：头 + payload size + 原始音频。`last` 为真时是末包（负包）。
pub fn encode_audio(payload: &[u8], last: bool) -> Vec<u8> {
    let flags = if last { FLAG_LAST_NO_SEQUENCE } else { FLAG_NO_SEQUENCE };
    let mut out = encode_header(MESSAGE_TYPE_AUDIO_ONLY_REQUEST, flags, SERIALIZATION_NONE).to_vec();
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(payload);
    out
}

/// 服务端帧。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerFrame {
    /// 识别结果（`sequence` 为负数、或 flags 带末包位，都表示这是最终响应）。
    Response { sequence: i32, flags: u8, payload: Vec<u8> },
    /// 协议错误帧。
    Error { code: u32, message: String },
}

/// 解析服务端帧。
pub fn decode_server_frame(data: &[u8]) -> Result<ServerFrame, String> {
    if data.len() < 4 {
        return Err(crate::code_detail("cloud.protocolError", "帧太短：连 4 字节头都不够"));
    }
    let message_type = data[1] >> 4;
    let flags = data[1] & 0x0f;
    match message_type {
        MESSAGE_TYPE_FULL_SERVER_RESPONSE => {
            let mut offset = 4;
            // flags 的最低位表示 header 后带 sequence。
            let sequence = if flags & 0b0001 != 0 {
                let bytes = read_bytes(data, &mut offset, 4)?;
                i32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
            } else {
                0
            };
            let size = read_u32(data, &mut offset)?;
            let end = offset + size as usize;
            if end > data.len() {
                return Err(crate::code_detail("cloud.protocolError", "帧长度不够：payload 被截断"));
            }
            Ok(ServerFrame::Response {
                sequence,
                flags,
                payload: data[offset..end].to_vec(),
            })
        }
        MESSAGE_TYPE_ERROR_RESPONSE => {
            let mut offset = 4;
            let code = read_u32(data, &mut offset)?;
            let size = read_u32(data, &mut offset)?;
            let end = offset + size as usize;
            if end > data.len() {
                return Err(crate::code_detail("cloud.protocolError", "错误帧长度不够：消息被截断"));
            }
            Ok(ServerFrame::Error {
                code,
                message: String::from_utf8_lossy(&data[offset..end]).into_owned(),
            })
        }
        other => Err(crate::code_detail(
            "cloud.protocolError",
            format!("不认识的服务端消息类型：{other:b}"),
        )),
    }
}

fn read_bytes<'a>(data: &'a [u8], offset: &mut usize, len: usize) -> Result<&'a [u8], String> {
    let end = *offset + len;
    if end > data.len() {
        return Err(crate::code_detail("cloud.protocolError", "帧长度不够"));
    }
    let slice = &data[*offset..end];
    *offset = end;
    Ok(slice)
}

fn read_u32(data: &[u8], offset: &mut usize) -> Result<u32, String> {
    let bytes = read_bytes(data, offset, 4)?;
    Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

#[cfg(test)]
mod tests;
