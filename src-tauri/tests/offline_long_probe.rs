//! 探针：**长段连续语音**跑完整的离线会话，看有多少音频没变成文字。
//!
//! 背景：用户反馈「一长段转写后，后面的录音会有丢失」。日志显示 162 秒的录音
//! 全部喂进了识别会话（`recorded_samples` 对得上），只出 20 句 308 字，
//! 且**没有**「识别跟不上」的告警 —— 所以不是队列丢的。
//! 另一个探针（vad_long_probe）证明 VAD 也不丢（105 秒只被吞 0.3 秒）。
//!
//! 这个探针量三件事：
//!   1. 喂进会话的采样数
//!   2. 切出来的句子覆盖了哪些时间段（段落的 start/end）
//!   3. 没被任何段落覆盖的时间 = 音频到不了文字的地方
//!
//! 运行：
//!   VOICE_NOTES_MODEL_DIR=... cargo test --test offline_long_probe -- --nocapture

use voice_notes_lib::asr::{OfflineEvent, OfflineSession};
use voice_notes_lib::engine::Engine;

mod common;
use common::{model_paths, trim_silence};

#[test]
fn probe_long_recording_loss() {
    let Some(paths) = model_paths() else {
        eprintln!("跳过：找不到 SenseVoice 模型");
        return;
    };
    let engine = std::sync::Arc::new(Engine::new(&paths, "zh", true).expect("加载模型"));

    // VAD 配置直接问引擎要 —— 和应用跑的是同一份参数
    let vad = engine.vad_config().expect("需要 VAD 模型");

    let wave = sherpa_onnx::Wave::read("tests/fixtures/zh.wav").expect("读取音频");
    let speech = trim_silence(wave.samples(), 0.02);

    for (label, gap) in [
        ("连续语音", 0usize),
        ("说话+2秒静音+说话", 16_000 * 2),
        ("说话+10秒静音+说话", 16_000 * 10),
    ] {
        let repeat = 3usize;
        let mut long = Vec::new();
        for i in 0..repeat {
            long.extend_from_slice(&speech);
            if gap > 0 && i + 1 < repeat {
                long.extend(std::iter::repeat(0.0f32).take(gap));
            }
        }
        let seconds = long.len() as f32 / 16000.0;

        let mut session = OfflineSession::new(engine.clone(), vad.clone());
        let mut segments: Vec<(f32, f32, String)> = Vec::new();
        // 和应用一样按 512 一块喂
        for chunk in long.chunks(512) {
            for event in session.push(chunk) {
                let OfflineEvent::Segment(segment) = event;
                segments.push((segment.start, segment.end, segment.text.clone()));
            }
        }
        for event in session.finish() {
            let OfflineEvent::Segment(segment) = event;
            segments.push((segment.start, segment.end, segment.text.clone()));
        }

        let fed = session.recorded_samples() as f32 / 16000.0;
        let covered: f32 = segments.iter().map(|(_, end, _)| *end).sum::<f32>()
            - segments.iter().map(|(start, _, _)| *start).sum::<f32>();
        let chars: usize = segments.iter().map(|(_, _, t)| t.chars().count()).sum();

        println!("\n════ {label}：{seconds:.1}s ════");
        println!(
            "  喂进 {fed:.1}s → 切出 {} 段，覆盖 {covered:.1}s，**没覆盖 {:.1}s**",
            segments.len(),
            fed - covered
        );
        println!("  文字共 {chars} 字");
        for (start, end, text) in segments.iter().take(12) {
            println!(
                "    [{start:>7.1}s → {end:>7.1}s] {}",
                if text.chars().count() > 40 {
                    format!("{}…", text.chars().take(40).collect::<String>())
                } else {
                    text.clone()
                }
            );
        }
        if segments.len() > 12 {
            println!("    …（还有 {} 段）", segments.len() - 12);
        }
    }
}
