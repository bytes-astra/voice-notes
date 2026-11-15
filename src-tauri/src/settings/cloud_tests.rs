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
        None,
        None,
    );
    assert_eq!(settings.cloud_base_url, "https://example.invalid/v1", "地址该去空白");
    assert_eq!(settings.cloud_model, "model-x", "模型名该去空白");
    assert_eq!(settings.cloud_api_key, "old-key", "没传 key 就不该动它");

    apply_cloud_config(&mut settings, String::new(), Some(String::new()), String::new(), None, None);
    assert!(settings.cloud_api_key.is_empty(), "传空串 = 清除凭据");
    assert!(settings.cloud_base_url.is_empty());
    assert!(settings.cloud_model.is_empty());

    apply_cloud_config(&mut settings, String::new(), Some(" new-key ".into()), String::new(), None, None);
    assert_eq!(settings.cloud_api_key, "new-key", "新 key 该去空白");
}

/// 云端服务与资源标识可随配置一起保存；非法 provider 不改。
#[test]
fn 云端配置保存服务与资源标识() {
    let mut settings = Settings::default();
    apply_cloud_config(
        &mut settings,
        String::new(),
        None,
        String::new(),
        Some("volcengine".into()),
        Some(" volc.seedasr.sauc.duration ".into()),
    );
    assert_eq!(settings.cloud_provider, "volcengine");
    assert_eq!(settings.cloud_resource_id, "volc.seedasr.sauc.duration", "资源标识该去空白");
    // 非法 provider 不该写进去（normalize 会回退默认）。
    apply_cloud_config(
        &mut settings,
        String::new(),
        None,
        String::new(),
        Some("不存在的服务".into()),
        None,
    );
    assert_eq!(settings.cloud_provider, "volcengine", "非法 provider 不该被接受");
}
