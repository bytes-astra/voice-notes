//! 后端音频解码：用 symphonia 从磁盘流式解码。
//!
//! 之前是前端用 WebView 的 `decodeAudioData` 解码：整条文件进内存，
//! 1 小时 48k 立体声 float32 ≈ 1.4GB，WebView 大概率直接失败；
//! 而且 WebView 解不了 mp4/mov（Windows 上尤其）。
//!
//! 现在解码在后端做：逐包读、逐块回调，内存恒定，
//! 格式支持交给 symphonia（mp3 / wav / flac / ogg / m4a / mp4 / mov，视频轨忽略）。

use std::fs::File;
use std::path::Path;

use sherpa_onnx::LinearResampler;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

use crate::asr::SAMPLE_RATE;
use crate::audio::Agc;
use crate::{code, code_detail};

/// 解码结果的基本信息。
#[derive(Clone, Debug)]
pub struct DecodeInfo {
    /// 输出采样率（固定 16 kHz）
    pub sample_rate: u32,
    /// 输出的采样总数
    pub samples: u64,
    /// 部分解码失败时的警告（已经解出一些音频，但有包出错）。
    ///
    /// `None` = 整个文件干净地解完了。调用方要把警告报给用户（前端 `decode.partial`），
    /// 不能静默截断。
    pub warning: Option<String>,
}

/// 正常读到文件末尾：symphonia 用 `UnexpectedEof` 表示「没有更多包了」。
fn is_end_of_stream(err: &SymphoniaError) -> bool {
    matches!(
        err,
        SymphoniaError::IoError(io) if io.kind() == std::io::ErrorKind::UnexpectedEof
    )
}

/// 从磁盘流式解码音频，逐块回调 16 kHz 单声道 f32。
///
/// 回调返回 Err 会立刻中止解码（用来打断识别线程）。
///
/// 解码出错不静默：
/// - 一个包都没解出来 → 返回 `Err`（`decode.failed`）
/// - 解出了一部分 → 返回 `Ok`，并在 `DecodeInfo::warning` 里带上原因
pub fn decode_to_16k(
    path: &Path,
    mut on_chunk: impl FnMut(&[f32]) -> Result<(), String>,
) -> Result<DecodeInfo, String> {
    let file = File::open(path).map_err(|e| code_detail("decode.openFailed", e))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|e| code_detail("decode.unknownFormat", e))?;
    let mut format = probed.format;

    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or_else(|| code("decode.noAudioTrack"))?;
    let track_id = track.id;
    let source_rate = track.codec_params.sample_rate.unwrap_or(0).max(1) as i32;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| code_detail("decode.failed", e))?;

    let mut resampler = LinearResampler::create(source_rate, SAMPLE_RATE);
    let mut total = 0u64;
    // 电平自动增益：偏小的录音会被 VAD 整段当成非语音丢掉（见 audio::Agc）。
    let mut agc = Agc::new();
    // 中途出错的原因（读包失败）
    let mut stream_error: Option<String> = None;
    // 单个包解码失败的次数与最后一次的原因（末尾垃圾数据很常见，但也不能完全无视）
    let mut codec_errors: u32 = 0;
    let mut last_codec_error: Option<String> = None;
    // 容器声明的帧数：用来发现「文件被截断」——读包时不会报错，但内容少了。
    let declared_frames = track.codec_params.n_frames;

    loop {
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(e) => {
                // UnexpectedEof = 文件正常读完；其他错误 = 中途出错，记下来。
                if !is_end_of_stream(&e) {
                    stream_error = Some(e.to_string());
                }
                break;
            }
        };
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(decoded) => decoded,
            // 解码器有时会对个别包报错，跳过即可（末尾的垃圾数据很常见），但要计数。
            Err(e) => {
                codec_errors += 1;
                last_codec_error = Some(e.to_string());
                continue;
            }
        };
        let spec = *decoded.spec();
        let channels = spec.channels.count().max(1) as u64;
        let mut sample_buf = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
        sample_buf.copy_interleaved_ref(decoded);
        let samples = sample_buf.samples();
        // 多声道混成单声道
        let mono: Vec<f32> = if channels == 1 {
            samples.to_vec()
        } else {
            samples
                .chunks(channels as usize)
                .map(|frame| frame.iter().sum::<f32>() / channels as f32)
                .collect()
        };
        let mut resampled = match resampler.as_mut() {
            Some(resampler) => resampler.resample(&mono, false),
            None => mono,
        };
        if !resampled.is_empty() {
            agc.process(&mut resampled);
            total += resampled.len() as u64;
            on_chunk(&resampled)?;
        }
    }

    // 收尾：把重采样器内部缓冲冲出来
    if let Some(resampler) = resampler.as_mut() {
        let mut tail = resampler.resample(&[], true);
        if !tail.is_empty() {
            agc.process(&mut tail);
            total += tail.len() as u64;
            on_chunk(&tail)?;
        }
    }

    // 一个采样都没解出来：不能当成成功（前端会显示空结果，用户以为文件是空的）。
    if total == 0 {
        let reason = stream_error
            .or(last_codec_error)
            .unwrap_or_else(|| "no audio data".to_string());
        return Err(code_detail("decode.failed", reason));
    }

    // 解出了一部分：把原因带回去，调用方要报给用户，不能静默截断。
    let mut problems: Vec<String> = Vec::new();
    if codec_errors > 0 {
        problems.push(format!("{codec_errors} packet(s) failed to decode"));
    }
    if let Some(err) = &last_codec_error {
        problems.push(err.clone());
    }
    if let Some(err) = &stream_error {
        problems.push(err.clone());
    }
    // 文件被截断：读包一路正常结束，但解出来的帧比容器声明的少。
    // 不报的话用户只会看到一段残缺的转写，完全不知道文件缺了内容。
    if let Some(declared) = declared_frames {
        let expected = declared
            .saturating_mul(SAMPLE_RATE as u64)
            .checked_div(source_rate as u64)
            .unwrap_or(declared);
        // 留一点余量：重采样和编解码器的延迟/填充会造成少量差异。
        let tolerance = SAMPLE_RATE as u64;
        if expected > tolerance && total + tolerance < expected {
            problems.push(format!(
                "truncated: decoded {total} of {expected} samples"
            ));
        }
    }
    let warning = if problems.is_empty() {
        None
    } else {
        Some(problems.join("; "))
    };

    Ok(DecodeInfo {
        sample_rate: SAMPLE_RATE as u32,
        samples: total,
        warning,
    })
}

#[cfg(test)]
mod tests;
