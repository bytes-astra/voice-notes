//! 云端配置的写入规则。

use super::*;

/// 写入云端配置：`None` 不改凭据、空串清空凭据。
///
/// 这条差别是给前端留的活：凭据不回传，输入框留空没法区分
/// 「没改」和「要清空」—— 所以约定由调用方显式表达。
#[test]
fn cloud_config_none_keeps_key_empty_clears() {
    let mut settings = Settings {
        cloud_api_key: "old-key".into(),
        ..Settings::default()
    };

    apply_cloud_config(
        &mut settings,
        " https://example.invalid/v1 ".into(),
        None,
        " model-x ".into(),
    );
    assert_eq!(settings.cloud_base_url, "https://example.invalid/v1", "地址该去空白");
    assert_eq!(settings.cloud_model, "model-x", "模型名该去空白");
    assert_eq!(settings.cloud_api_key, "old-key", "没传 key 就不该动它");

    apply_cloud_config(&mut settings, String::new(), Some(String::new()), String::new());
    assert!(settings.cloud_api_key.is_empty(), "传空串 = 清除凭据");
    assert!(settings.cloud_base_url.is_empty());
    assert!(settings.cloud_model.is_empty());

    apply_cloud_config(&mut settings, String::new(), Some(" new-key ".into()), String::new());
    assert_eq!(settings.cloud_api_key, "new-key", "新 key 该去空白");
}
