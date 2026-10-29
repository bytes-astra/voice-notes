//! `audio` 模块的单元测试（从 `audio.rs` 拆出）。

use super::*;

#[test]
fn event_and_device_fields_serialize_camel_case() {
    let level = serde_json::to_value(LevelPayload {
        session_id: Some(7),
        level: 0.5,
        elapsed: 2.0,
    })
    .unwrap();
    assert_eq!(level["sessionId"], serde_json::json!(7));
    assert!(level.get("session_id").is_none());

    let device = serde_json::to_value(InputDevice {
        name: "Mic".into(),
        is_default: true,
    })
    .unwrap();
    assert_eq!(device["isDefault"], serde_json::json!(true));
    assert!(device.get("is_default").is_none());
}

/// 没有名字的设备不能列出来：按名字选不中它（`pick_device_name` 只做精确匹配），
/// 列出来只会让界面显示一条选不中的项。
#[test]
fn unnamed_devices_are_not_listed() {
    assert!(device_entry(String::new(), None).is_none());
    assert!(device_entry("   ".into(), None).is_none());
    let device = device_entry("Mic".into(), Some("Mic")).expect("有名字的设备要列出");
    assert!(device.is_default);
    assert!(!device_entry("Mic".into(), Some("Other")).unwrap().is_default);
}

#[test]
fn take_new_consumes_samples() {
    let q = SampleQueue::new(48_000);
    q.push(&[1.0, 2.0, 3.0]);
    let got = q.take_new();
    assert_eq!(got.samples, vec![1.0, 2.0, 3.0]);
    assert_eq!(got.dropped, 0);
    assert!(q.take_new().samples.is_empty()); // 取过就没有了
}

#[test]
fn take_new_drops_oldest_when_over_limit() {
    let q = SampleQueue::new(48_000);
    let limit = MAX_PENDING_SECONDS as usize * 48_000;
    let zeros = vec![0.0; limit + 100];
    q.push(&zeros);
    let got = q.take_new();
    assert_eq!(got.dropped, 100);
    assert_eq!(got.samples.len(), limit);
}

#[test]
fn pending_limit_follows_sample_rate() {
    // 16 kHz 的设备上限应该按 16k 算，不能拿 48k 的样本数去卡
    let q = SampleQueue::new(16_000);
    let limit = MAX_PENDING_SECONDS as usize * 16_000;
    let zeros = vec![0.0; limit];
    q.push(&zeros);
    assert_eq!(q.take_new().dropped, 0);
    q.push(&[0.0; 1]);
    assert_eq!(q.take_new().dropped, 0);
}

/// 缺口标记是语言中立的占位符：前端渲染时才翻成界面语言。
#[test]
fn gap_marker_is_a_placeholder() {
    assert_eq!(gap_marker(48_000, 48_000), "[[gap:1]]"); // 48000 @48k = 1 秒
    assert_eq!(gap_marker(96_000, 48_000), "[[gap:2]]"); // 2 秒
    assert_eq!(gap_marker(1, 48_000), "[[gap:1]]"); // 不足 1 秒也算 1 秒
}

#[test]
fn default_input_is_usable() {
    // CI runner 的虚拟声卡可能列得出来、读配置却失败，别让宿主环境卡住构建。
    if std::env::var("CI").is_ok() {
        eprintln!("跳过：CI 环境不依赖宿主音频设备");
        return;
    }
    let host = cpal::default_host();
    let Some(device) = host.default_input_device() else {
        eprintln!("跳过：没有默认输入设备");
        return;
    };
    let Ok(config) = device.default_input_config() else {
        eprintln!("跳过：读取默认输入配置失败");
        return;
    };
    assert!(config.sample_rate().0 >= 8_000);
    assert!(config.channels() >= 1);
    println!(
        "默认麦克风：{} Hz / {} 声道 / {:?}",
        config.sample_rate().0,
        config.channels(),
        config.sample_format()
    );
}

/// 真录音测试（会触发系统麦克风授权），默认跳过：
///   VOICE_NOTES_RECORD_TEST=1 cargo test --lib records_microphone -- --nocapture
#[test]
fn records_microphone() {
    if std::env::var("VOICE_NOTES_RECORD_TEST").is_err() {
        eprintln!("跳过：未设置 VOICE_NOTES_RECORD_TEST");
        return;
    }
    let recorder = start_with(None, None, None).expect("打开麦克风");
    let rate = recorder.sample_rate();
    let queue = recorder.queue();
    std::thread::sleep(std::time::Duration::from_secs(2));
    recorder.stop().expect("停止录音");
    // 停止后采样仍留在队列里，由消费者取走（stop 不再代取）。
    let samples = queue.take_new().samples;
    let peak = samples.iter().fold(0f32, |acc, s| acc.max(s.abs()));
    println!("采到 {} 个采样 @ {rate} Hz，峰值 {peak:.4}", samples.len());
    assert!(samples.len() > rate as usize, "录音样本太少");
    assert!(peak <= 1.0, "样本超出 [-1, 1]");
}

// ──────────────────── AGC ────────────────────

/// 造一段「语音样」的信号：正弦 + 噪声，峰值和 RMS 可控。
fn tone(rms: f32, len: usize) -> Vec<f32> {
    (0..len)
        .map(|i| {
            let t = i as f32 / 16_000.0;
            rms * (2.0 * std::f32::consts::PI * 220.0 * t).sin() * 1.4
        })
        .collect()
}

/// 电平够的录音一个样本都不能动 —— AGC 只救偏小的，不碰正常的。
#[test]
fn agc_leaves_normal_level_untouched() {
    let mut agc = Agc::new();
    let mut samples = tone(0.0225, 16_000);
    let before = samples.clone();
    agc.process(&mut samples);
    assert_eq!(samples, before, "正常电平不该被改动");
    assert_eq!(agc.gain(), 1.0);
}

/// 电平偏低时要把 RMS 提到目标附近 —— 这是「整段没转写」的解药。
#[test]
fn agc_raises_quiet_signal_to_target() {
    let mut agc = Agc::new();
    let mut samples = tone(0.0013, 16_000); // -25 dB 那档
    agc.process(&mut samples);
    let rms = (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt();
    assert!(rms > 0.015, "RMS 应该被提上来，实际 {rms:.5}");
}

/// 增益有上限：纯静音不能被放大成一片嘶嘶（否则噪声会被 VAD 当成语音）。
#[test]
fn agc_respects_max_gain() {
    let mut agc = Agc::new();
    let mut samples = vec![0.0001f32; 16_000];
    agc.process(&mut samples);
    assert!(agc.gain() <= AGC_MAX_GAIN + 1e-3, "增益超上限");
}

/// 输出不能越界 —— 模型看到的是合法波形，削波会毁掉识别率。
#[test]
fn agc_output_stays_in_range() {
    let mut agc = Agc::new();
    let mut samples = vec![0.9f32; 16_000];
    agc.process(&mut samples);
    let peak = samples.iter().fold(0.0f32, |acc, s| acc.max(s.abs()));
    assert!(peak <= AGC_LIMIT, "峰值越界：{peak}");
}

/// 全零输入不能炸（除零 → NaN 会污染整条链路）。
#[test]
fn agc_handles_silence() {
    let mut agc = Agc::new();
    let mut samples = vec![0.0f32; 16_000];
    agc.process(&mut samples);
    assert!(samples.iter().all(|s| s.is_finite()));
}

/// 增益要跟着电平慢慢变，不能一块一块地跳（pumping 会切碎字头）。
#[test]
fn agc_gain_changes_smoothly() {
    let mut agc = Agc::new();
    let mut previous = agc.gain();
    for _ in 0..20 {
        // 每轮都要用**没被处理过**的采样：生产里每块都是新来的，
        // 拿上一轮改写过的缓冲再喂一遍等于把增益算在增益上。
        let mut samples = tone(0.001, 512);
        agc.process(&mut samples);
        let gain = agc.gain();
        assert!(gain >= previous, "增益不该往回掉");
        assert!(gain - previous < 2.0, "增益跳得太快：{previous} → {gain}");
        previous = gain;
    }
}
