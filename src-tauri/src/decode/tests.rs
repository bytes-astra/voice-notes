//! `decode` 模块的单元测试（从 `decode.rs` 拆出）。

use super::*;

fn fixture(name: &str) -> Option<std::path::PathBuf> {
    let path = std::path::Path::new("tests/fixtures").join(name);
    if path.exists() {
        Some(path)
    } else {
        eprintln!("跳过：缺少 {name}");
        None
    }
}

/// 干净的文件不该有警告（否则每次转写都会弹一条没用的提示）。
#[test]
fn clean_file_has_no_warning() {
    let Some(path) = fixture("zh.wav") else { return };
    let mut total = 0usize;
    let result = decode_to_16k(&path, |chunk| {
        total += chunk.len();
        Ok(())
    });
    let Ok(info) = result else {
        panic!("解码 wav 应该成功");
    };
    assert_eq!(info.samples as usize, total);
    assert!(total > 16_000, "至少应该解出 1 秒");
    assert!(info.warning.is_none(), "不该有警告: {:?}", info.warning);
}

/// 一个采样都没解出来 → 报错，不能当成「文件是空的」糊弄过去。
#[test]
fn empty_result_is_an_error() {
    let Some(path) = fixture("broken.mp4") else { return };
    let Err(err) = decode_to_16k(&path, |_| Ok(())) else {
        panic!("解不出内容应该报错");
    };
    assert!(err.starts_with("decode."), "错误应该是字典 key: {err}");
}

/// 文件被截断 → 解出一部分 + 警告，不能静默截断。
#[test]
fn truncated_file_warns_but_returns_partial() {
    let Some(path) = fixture("zh.wav") else { return };
    let Ok(bytes) = std::fs::read(&path) else {
        eprintln!("跳过：读不到 fixture");
        return;
    };
    if bytes.len() < 60_000 {
        eprintln!("跳过：fixture 太小");
        return;
    }
    let cut = std::env::temp_dir().join(format!("vn-trunc-{}.wav", std::process::id()));
    if std::fs::write(&cut, &bytes[..40_000]).is_err() {
        eprintln!("跳过：写不了临时文件");
        return;
    }
    let result = decode_to_16k(&cut, |_| Ok(()));
    let _ = std::fs::remove_file(&cut);
    let Ok(info) = result else {
        panic!("截断文件应该解出一部分");
    };
    assert!(info.samples > 0, "应该解出一些内容");
    assert!(
        info.warning.is_some(),
        "截断文件必须带警告，不能静默截断"
    );
}
