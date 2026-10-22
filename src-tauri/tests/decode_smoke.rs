//! 后端解码冒烟测试：symphonia 从磁盘解码 → 16 kHz 单声道。
//!
//! fixture 都在 `tests/fixtures/`：
//!   sample.m4a  — afconvert 造的 AAC（mp4 容器）
//!   broken.mp4  — sample.m4a 的前 1KB（损坏文件）

use std::path::Path;

use voice_notes_lib::decode::decode_to_16k;

fn fixture(name: &str) -> Option<std::path::PathBuf> {
    let path = Path::new("tests/fixtures").join(name);
    if path.exists() {
        Some(path)
    } else {
        eprintln!("跳过：缺少 {name}");
        None
    }
}

#[test]
fn decodes_wav_to_16k() {
    let Some(path) = fixture("zh.wav") else { return };
    let mut total = 0usize;
    let info = decode_to_16k(&path, |chunk| {
        total += chunk.len();
        Ok(())
    })
    .expect("解码 wav");
    println!("wav → {total} 个采样 @ {} Hz", info.sample_rate);
    assert_eq!(info.sample_rate, 16_000);
    assert!(total > 16_000, "至少应该解出 1 秒");
    assert_eq!(info.samples as usize, total);
}

#[test]
fn decodes_m4a_aac() {
    let Some(path) = fixture("sample.m4a") else { return };
    let mut total = 0usize;
    let info = decode_to_16k(&path, |chunk| {
        total += chunk.len();
        Ok(())
    })
    .expect("解码 m4a");
    println!("m4a → {total} 个采样 @ {} Hz", info.sample_rate);
    assert_eq!(info.sample_rate, 16_000);
    assert!(total > 16_000, "至少应该解出 1 秒");
}

#[test]
fn corrupted_file_returns_error() {
    let Some(path) = fixture("broken.mp4") else { return };
    let err = decode_to_16k(&path, |_| Ok(())).expect_err("损坏文件应该报错");
    println!("损坏文件报错：{err}");
    assert!(!err.is_empty());
}

#[test]
fn missing_file_returns_error() {
    let err = decode_to_16k(Path::new("tests/fixtures/does-not-exist.wav"), |_| Ok(()))
        .expect_err("不存在的文件应该报错");
    assert!(!err.is_empty());
}
