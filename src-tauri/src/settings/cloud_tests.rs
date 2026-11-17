//! 云端连接（配置档案）的写入规则。

use super::*;

/// 造一条连接（测试用）。
fn profile(provider: &str, key: &str, model: &str) -> CloudProfile {
    CloudProfile {
        id: "p-1".into(),
        label: String::new(),
        provider: provider.into(),
        base_url: String::new(),
        api_key: key.into(),
        model: model.into(),
    }
}

/// 写入连接：`None` 不改凭据、空串清空凭据。
///
/// 这条差别是给前端留的活：凭据不回传，输入框留空没法区分
/// 「没改」和「要清空」—— 所以约定由调用方显式表达。
#[test]
fn profile_none_keeps_key_empty_clears() {
    let mut settings = Settings {
        cloud_profiles: vec![profile("aliyun", "old-key", "model-x")],
        cloud_active_profile: "p-1".into(),
        ..Settings::default()
    };

    let id = upsert_cloud_profile(
        &mut settings,
        Some("p-1".into()),
        "阿里".into(),
        "aliyun".into(),
        " https://example.invalid/v1 ".into(),
        None,
        " model-y ".into(),
    );
    let saved = settings
        .cloud_profiles
        .iter()
        .find(|item| item.id == id)
        .expect("连接该在");
    assert_eq!(saved.base_url, "https://example.invalid/v1", "地址该去空白");
    assert_eq!(saved.model, "model-y", "模型该去空白");
    assert_eq!(saved.api_key, "old-key", "没传 key 就不该动它");
    assert_eq!(saved.label, "阿里", "备注该存下");

    // 传空串 = 清除凭据。
    upsert_cloud_profile(
        &mut settings,
        Some(id.clone()),
        "阿里".into(),
        "aliyun".into(),
        String::new(),
        Some(String::new()),
        String::new(),
    );
    let saved = settings
        .cloud_profiles
        .iter()
        .find(|item| item.id == id)
        .expect("连接该在");
    assert!(saved.api_key.is_empty(), "传空串 = 清除凭据");
    assert!(saved.base_url.is_empty());
    assert!(saved.model.is_empty());
}

/// 新增连接：拿到新 id，并自动成为正在使用的那条。
#[test]
fn new_profile_becomes_active_and_gets_an_id() {
    let mut settings = Settings::default();
    let id = upsert_cloud_profile(
        &mut settings,
        None,
        "阿里".into(),
        "aliyun".into(),
        String::new(),
        Some("key-1".into()),
        String::new(),
    );
    assert_eq!(settings.cloud_profiles.len(), 1, "该多出一条连接");
    assert_eq!(settings.cloud_active_profile, id, "新增的自动成为使用中");
    assert_eq!(settings.cloud_profiles[0].api_key, "key-1");
}

/// 非法 provider 不该被接受 → 回退默认。
#[test]
fn illegal_provider_falls_back_to_the_default() {
    let mut settings = Settings::default();
    upsert_cloud_profile(
        &mut settings,
        None,
        String::new(),
        "不存在的服务".into(),
        String::new(),
        Some("k".into()),
        String::new(),
    );
    assert_eq!(settings.cloud_profiles[0].provider, "realtime");
}

/// 删连接：删的是使用中的那条 → 自动落到下一条。
#[test]
fn deleting_the_active_profile_falls_back_to_the_next_one() {
    let mut settings = Settings::default();
    let first = upsert_cloud_profile(
        &mut settings,
        None,
        "第一条".into(),
        "aliyun".into(),
        String::new(),
        Some("k1".into()),
        String::new(),
    );
    let second = upsert_cloud_profile(
        &mut settings,
        None,
        "第二条".into(),
        "volcengine".into(),
        String::new(),
        Some("k2".into()),
        String::new(),
    );
    assert_eq!(settings.cloud_active_profile, first, "第一条是使用中的");
    set_active_cloud_profile(&mut settings, &first);

    delete_cloud_profile(&mut settings, &first);
    assert_eq!(settings.cloud_profiles.len(), 1, "该只剩一条");
    assert_eq!(settings.cloud_active_profile, second, "删掉使用中的 → 落到下一条");
}

/// 删空 → 回到「未配置」。
#[test]
fn deleting_the_last_profile_leaves_nothing_configured() {
    let mut settings = Settings::default();
    let id = upsert_cloud_profile(
        &mut settings,
        None,
        "唯一一条".into(),
        "aliyun".into(),
        String::new(),
        Some("k".into()),
        String::new(),
    );
    delete_cloud_profile(&mut settings, &id);
    assert!(settings.cloud_profiles.is_empty(), "该空了");
    assert!(settings.cloud_active_profile.is_empty(), "没有使用中的连接");
    assert!(
        settings.active_cloud_profile().is_none(),
        "没有活动连接 = 没配过云端"
    );
}

/// 切换 provider **不再清空**别人的连接 —— 两条各留各的。
#[test]
fn switching_provider_keeps_the_other_providers_profile() {
    let mut settings = Settings::default();
    upsert_cloud_profile(
        &mut settings,
        None,
        "阿里".into(),
        "aliyun".into(),
        String::new(),
        Some("aliyun-key".into()),
        String::new(),
    );
    let volc = upsert_cloud_profile(
        &mut settings,
        None,
        "火山".into(),
        "volcengine".into(),
        String::new(),
        Some("volc-key".into()),
        String::new(),
    );
    assert_eq!(settings.cloud_profiles.len(), 2, "两条都在");
    set_active_cloud_profile(&mut settings, &volc);
    assert_eq!(settings.cloud_active_profile, volc, "切到火山那条");
    // 阿里那条**一个字段都没丢**。
    let aliyun = settings
        .cloud_profiles
        .iter()
        .find(|item| item.provider == "aliyun")
        .expect("阿里那条还在");
    assert_eq!(aliyun.api_key, "aliyun-key", "切走不能把别人的凭据清掉");
}



/// 从没配过云端 → **不**生成空连接。
#[test]
fn a_fresh_install_has_no_profiles() {
    let fresh = Settings::default().normalize();
    assert!(fresh.cloud_profiles.is_empty(), "没配过就不该有连接");
    assert!(fresh.cloud_active_profile.is_empty());
    assert!(fresh.active_cloud_profile().is_none());
}


/// 已有连接列表 → normalize 不该多出一条连接。
#[test]
fn normalize_keeps_the_existing_profile_list() {
    let settings = Settings {
        cloud_profiles: vec![profile("aliyun", "key-1", "m")],
        cloud_active_profile: "p-1".into(),
        ..Settings::default()
    };
    // 有连接列表 → 直接 normalize，不碰旧字段。
    let normalized = settings.normalize();
    assert_eq!(normalized.cloud_profiles.len(), 1, "不该多出一条");
    assert_eq!(normalized.cloud_profiles[0].api_key, "key-1");
    assert_eq!(normalized.cloud_active_profile, "p-1");
}

/// 掩码：头 4 + 尾 4；短的只露头 2 个；空串是空串。
#[test]
fn mask_secret_hides_the_middle() {
    assert_eq!(mask_secret(""), "");
    assert_eq!(mask_secret("sk-c1234HePU"), "sk-c…HePU");
    // 太短的凭据：只露头 2 个（露头尾就把大半条露出去了）。
    assert_eq!(mask_secret("abc"), "ab…");
    assert_eq!(mask_secret("abcdefgh"), "ab…");
}

/// 掩码不能被原样反推：短凭据不该整条露出。
#[test]
fn mask_secret_never_reveals_the_whole_secret() {
    for secret in ["a", "ab", "abcd", "abcdefgh", "abcdefghi", "sk-c1234HePU"] {
        let masked = mask_secret(secret);
        assert_ne!(masked, secret, "掩码不能等于原文：{secret}");
        assert!(!masked.contains(secret), "掩码里不能含整条原文：{secret}");
    }
}
