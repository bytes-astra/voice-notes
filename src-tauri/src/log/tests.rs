//! `log` 模块的单元测试（从 `log.rs` 拆出）。

use super::*;

/// 日志文件真的写出来了、超限会轮转 —— 不依赖 AppHandle，临时目录里跑。
#[test]
fn writes_lines_and_rotates() {
    let dir = std::env::temp_dir().join(format!("voice-notes-log-test-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("建临时目录");

    write_line(&dir, "2026-01-01 00:00:00.000 [INFO] 第一行");
    write_line(&dir, "2026-01-01 00:00:01.000 [INFO] 第二行");
    let body = fs::read_to_string(dir.join(FILE_NAME)).expect("读日志");
    assert!(body.contains("第一行"), "第一行没写进去：{body}");
    assert!(body.contains("第二行"), "第二行没写进去：{body}");

    // 写爆上限 → 轮转：旧内容进 .1，新内容进当前文件。
    fs::write(dir.join(FILE_NAME), vec![b'x'; MAX_BYTES as usize + 16]).expect("写大文件");
    write_line(&dir, "2026-01-01 00:00:02.000 [INFO] 轮转之后");
    assert!(dir.join("voice-notes.log.1").exists(), "没有生成轮转文件");
    let current = fs::read_to_string(dir.join(FILE_NAME)).expect("读当前日志");
    assert!(current.contains("轮转之后"), "轮转后新行没写进去：{current}");
    assert!(!current.contains('x'), "轮转后当前文件还是旧内容");

    let _ = fs::remove_dir_all(&dir);
}
