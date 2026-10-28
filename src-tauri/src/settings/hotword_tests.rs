//! `settings` 模块的单元测试（从 `settings.rs` 拆出）。

use super::*;

#[test]
fn cleans_hotwords() {
    let cleaned = clean_hotwords(vec![
        " 产品评审会 ".into(),
        "".into(),
        "   ".into(),
        "产品评审会".into(),
        "玄戒芯片".into(),
        "x".repeat(MAX_HOTWORD_CHARS + 1),
    ]);
    assert_eq!(cleaned, vec!["产品评审会", "玄戒芯片"]);
}

#[test]
fn caps_hotword_count() {
    let many: Vec<String> = (0..MAX_HOTWORDS + 20).map(|i| format!("词{i}")).collect();
    assert_eq!(clean_hotwords(many).len(), MAX_HOTWORDS);
}

#[test]
fn settings_round_trip() {
    let raw = r#"{"language":"zh","itn":false,"output_dir":"/tmp/out","model_dir":null}"#;
    let settings: Settings = serde_json::from_str(raw).unwrap();
    assert_eq!(settings.language, "zh");
    assert!(!settings.itn);
    let normalized = Settings {
        language: "klingon".into(),
        ..settings
    }
    .normalize();
    assert_eq!(normalized.language, "auto");
}

/// 输出目录必须先校验再落设置：建不出来的目录不能写进设置里。
#[test]
fn rejects_an_unusable_output_dir() {
    // 在文件底下建目录 → 必然失败
    let file = std::env::temp_dir().join(format!("vn-out-file-{}", std::process::id()));
    std::fs::write(&file, b"x").unwrap();
    let bad = file.join("sub");
    assert!(
        prepare_output_dir(Some(bad.display().to_string())).is_err(),
        "建不出来的目录不能通过校验"
    );

    // 正常目录：能建出来才算数
    let good = std::env::temp_dir().join(format!("vn-out-ok-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&good);
    let prepared =
        prepare_output_dir(Some(good.display().to_string())).expect("应该能建出目录");
    assert_eq!(prepared, Some(good.display().to_string()));

    // 空目录 = 用默认目录
    assert_eq!(prepare_output_dir(None).unwrap(), None);
    assert_eq!(prepare_output_dir(Some("   ".into())).unwrap(), None);

    let _ = std::fs::remove_dir_all(&good);
    let _ = std::fs::remove_file(&file);
}
