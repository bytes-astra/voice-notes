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
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

use crate::asr::SAMPLE_RATE;

/// 解码结果的基本信息。
#[derive(Clone, Copy, Debug)]
pub struct DecodeInfo {
    /// 输出采样率（固定 16 kHz）
    pub sample_rate: u32,
    /// 输出的采样总数
    pub samples: u64,
}

/// 从磁盘流式解码音频，逐块回调 16 kHz 单声道 f32。
///
/// 回调返回 Err 会立刻中止解码（用来打断识别线程）。
pub fn decode_to_16k(
    path: &Path,
    mut on_chunk: impl FnMut(&[f32]) -> Result<(), String>,
) -> Result<DecodeInfo, String> {
    let file = File::open(path).map_err(|e| format!("无法打开文件：{e}"))?;
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
        .map_err(|e| format!("无法识别这个文件的格式：{e}"))?;
    let mut format = probed.format;

    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or("这个文件里没有找到音频轨道")?;
    let track_id = track.id;
    let source_rate = track.codec_params.sample_rate.unwrap_or(0).max(1) as i32;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| format!("无法解码这个文件：{e}"))?;

    let mut resampler = LinearResampler::create(source_rate, SAMPLE_RATE);
    let mut total = 0u64;

    while let Ok(packet) = format.next_packet() {
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(decoded) => decoded,
            // 解码器有时会对个别包报错，跳过即可（末尾的垃圾数据很常见）
            Err(_) => continue,
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
        let resampled = match resampler.as_mut() {
            Some(resampler) => resampler.resample(&mono, false),
            None => mono,
        };
        if !resampled.is_empty() {
            total += resampled.len() as u64;
            on_chunk(&resampled)?;
        }
    }

    // 收尾：把重采样器内部缓冲冲出来
    if let Some(resampler) = resampler.as_mut() {
        let tail = resampler.resample(&[], true);
        if !tail.is_empty() {
            total += tail.len() as u64;
            on_chunk(&tail)?;
        }
    }

    Ok(DecodeInfo {
        sample_rate: SAMPLE_RATE as u32,
        samples: total,
    })
}
