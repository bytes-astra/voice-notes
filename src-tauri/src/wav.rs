//! 解析前端推来的 16 kHz 单声道 PCM16 WAV（与 DSH 语音输入同一种载荷）。

use base64::Engine as _;

pub const TARGET_RATE: u32 = 16_000;

pub fn decode_base64(text: &str) -> Result<Vec<u8>, String> {
    let payload = text.rsplit(',').next().unwrap_or(text).trim();
    base64::engine::general_purpose::STANDARD
        .decode(payload.as_bytes())
        .map_err(|e| format!("音频数据不是合法的 base64: {e}"))
}

/// 读取 WAV：只接受 16 kHz 单声道 PCM16 / Float32。
pub fn wav_to_samples(bytes: &[u8]) -> Result<Vec<f32>, String> {
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("不是合法的 WAV 文件".into());
    }
    let mut offset = 12usize;
    let mut format: Option<(u16, u16, u32, u16)> = None;
    let mut samples: Option<Vec<f32>> = None;

    while offset + 8 <= bytes.len() {
        let id = &bytes[offset..offset + 4];
        let size = u32::from_le_bytes([
            bytes[offset + 4],
            bytes[offset + 5],
            bytes[offset + 6],
            bytes[offset + 7],
        ]) as usize;
        let body = offset + 8;
        let end = body.saturating_add(size).min(bytes.len());

        if id == b"fmt " && size >= 16 && end >= body + 16 {
            let audio_format = u16::from_le_bytes([bytes[body], bytes[body + 1]]);
            let channels = u16::from_le_bytes([bytes[body + 2], bytes[body + 3]]);
            let sample_rate = u32::from_le_bytes([
                bytes[body + 4],
                bytes[body + 5],
                bytes[body + 6],
                bytes[body + 7],
            ]);
            let bits = u16::from_le_bytes([bytes[body + 14], bytes[body + 15]]);
            format = Some((audio_format, channels, sample_rate, bits));
        } else if id == b"data" {
            let Some((audio_format, channels, sample_rate, bits)) = format else {
                return Err("WAV 头缺少 fmt 块".into());
            };
            if channels != 1 {
                return Err("音频必须是单声道".into());
            }
            if sample_rate != TARGET_RATE {
                return Err(format!("音频采样率必须是 16 kHz，收到 {sample_rate} Hz"));
            }
            let data = &bytes[body..end];
            samples = Some(match (audio_format, bits) {
                (1, 16) => data
                    .chunks_exact(2)
                    .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
                    .collect(),
                (3, 32) => data
                    .chunks_exact(4)
                    .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                    .collect(),
                _ => {
                    return Err(format!(
                        "不支持的 WAV 编码（format={audio_format}, bits={bits}）"
                    ))
                }
            });
        }

        // 块按偶数字节对齐。
        offset = body + size + (size & 1);
    }

    samples
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "WAV 里没有音频数据".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pcm16_wav(samples: &[i16]) -> Vec<u8> {
        let mut out = Vec::new();
        let data_len = (samples.len() * 2) as u32;
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + data_len).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&16000u32.to_le_bytes());
        out.extend_from_slice(&32000u32.to_le_bytes());
        out.extend_from_slice(&2u16.to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&data_len.to_le_bytes());
        for s in samples {
            out.extend_from_slice(&s.to_le_bytes());
        }
        out
    }

    #[test]
    fn parses_pcm16_mono() {
        let wav = pcm16_wav(&[0, 16384, -32768]);
        let samples = wav_to_samples(&wav).unwrap();
        assert_eq!(samples.len(), 3);
        assert!((samples[1] - 0.5).abs() < 1e-4);
        assert!((samples[2] + 1.0).abs() < 1e-4);
    }

    #[test]
    fn rejects_stereo() {
        let mut wav = pcm16_wav(&[0, 0]);
        wav[22] = 2; // channels
        assert!(wav_to_samples(&wav).is_err());
    }

    #[test]
    fn base64_round_trip() {
        let wav = pcm16_wav(&[1, 2, 3]);
        let encoded = base64::engine::general_purpose::STANDARD.encode(&wav);
        assert_eq!(decode_base64(&encoded).unwrap(), wav);
        assert_eq!(
            decode_base64(&format!("data:audio/wav;base64,{encoded}")).unwrap(),
            wav
        );
    }
}
