//! `lib` 模块的单元测试（从 `lib.rs` 拆出）。

#[test]
fn recognition_config_is_locked_while_recording() {
    assert!(super::ensure_config_unlocked(false).is_ok());
    assert_eq!(
        super::ensure_config_unlocked(true),
        Err("record.configLocked".into())
    );
}
