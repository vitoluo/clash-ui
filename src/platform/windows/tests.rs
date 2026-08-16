use super::network::{normalize_tun_device, proxy_bypass_string};
use super::uwp::{
    apply_uwp_changes, resolve_uwp_display_name, resolve_uwp_display_name_with,
    set_uwp_loopback_batch, uwp_apps_from_containers, AppContainer, LoopbackSid,
};
use super::uwp_api::{format_network_isolation_error, ERROR_ACCESS_DENIED};
use super::uwp_elevation::{
    cleanup_uwp_result, deserialize_uwp_changes, elevated_uwp_launch_error, new_uwp_result_path,
    parse_uwp_result, resolve_uwp_helper_path, serialize_uwp_changes, wait_for_uwp_result,
    write_uwp_result,
};

fn test_app_container(name: &str, package_family_name: &str) -> AppContainer {
    AppContainer {
        name: name.to_string(),
        package_family_name: package_family_name.to_string(),
        sid: Vec::new(),
    }
}

#[test]
fn normalizes_tun_device_whitespace() {
    assert_eq!(normalize_tun_device("  clash  "), "clash");
}

#[test]
fn sorts_native_uwp_apps_and_resolves_resource_name() {
    assert_eq!(
        resolve_uwp_display_name(
            Some("@{Package.ResourceName}"),
            "Fallback_abc",
            "Package_full",
        ),
        "Fallback_abc"
    );
    assert_eq!(
        resolve_uwp_display_name(Some("  Calculator  "), "Fallback_abc", "Package_full"),
        "Calculator"
    );
    let apps = uwp_apps_from_containers(
        vec![
            test_app_container("zeta", "Zeta_abc"),
            test_app_container("Alpha", "Alpha_abc"),
            test_app_container("alpha", "Alpha_def"),
        ],
        &[],
    );
    assert_eq!(apps.len(), 3);
    assert_eq!(
        apps.iter()
            .map(|app| app.package_family_name.as_str())
            .collect::<Vec<_>>(),
        vec!["Alpha_abc", "Alpha_def", "Zeta_abc"]
    );
}

#[test]
fn maps_current_loopback_sids_to_enabled_apps() {
    let enabled_sid = vec![1, 2, 3, 4, 5, 6, 7, 8];
    let mut enabled = test_app_container("Enabled", "Enabled_abc");
    enabled.sid = enabled_sid.clone();
    let mut disabled = test_app_container("Disabled", "Disabled_abc");
    disabled.sid = vec![8, 7, 6, 5, 4, 3, 2, 1];

    let apps = uwp_apps_from_containers(
        vec![enabled, disabled],
        &[LoopbackSid {
            bytes: enabled_sid,
            attributes: 0,
        }],
    );
    assert_eq!(
        apps.iter()
            .map(|app| (app.package_family_name.as_str(), app.enabled))
            .collect::<Vec<_>>(),
        vec![("Disabled_abc", false), ("Enabled_abc", true)]
    );
}

#[test]
fn resolves_localized_uwp_resource_before_fallback() {
    assert_eq!(
        resolve_uwp_display_name_with(
            Some("@{Package.ResourceName}"),
            "Fallback_abc",
            "Package_full",
            |resource| {
                assert_eq!(resource, "@{Package.ResourceName}");
                Some("计算器".to_string())
            },
        ),
        "计算器"
    );
    assert_eq!(
        resolve_uwp_display_name_with(
            Some("@{Package.ResourceName}"),
            "Fallback_abc",
            "Package_full",
            |_| None,
        ),
        "Fallback_abc"
    );
    assert_eq!(
        resolve_uwp_display_name_with(
            Some("ms-resource:///Resources/AppName"),
            "Fallback_abc",
            "Package_full",
            |resource| {
                assert_eq!(resource, "@{Package_full?ms-resource:///Resources/AppName}");
                Some("本地化应用".to_string())
            },
        ),
        "本地化应用"
    );
}

#[test]
fn serializes_uwp_changes_and_uses_real_elevated_result() {
    let changes = vec![
        ("Alpha_abc".to_string(), true),
        ("Beta_def".to_string(), false),
    ];
    let payload = serialize_uwp_changes(&changes).unwrap();
    assert_eq!(deserialize_uwp_changes(&payload).unwrap(), changes);

    assert_eq!(parse_uwp_result("ok\n"), Ok(()));
    assert_eq!(
        parse_uwp_result("error\n拒绝访问"),
        Err("拒绝访问".to_string())
    );
    assert!(parse_uwp_result("42").is_err());
    assert_eq!(elevated_uwp_launch_error(Some(42)), None);
    assert!(elevated_uwp_launch_error(Some(5))
        .unwrap()
        .contains("启动返回码 5"));

    let result_path = new_uwp_result_path().unwrap();
    write_uwp_result(&result_path, &Ok(())).unwrap();
    assert_eq!(wait_for_uwp_result(&result_path), Ok(()));
    cleanup_uwp_result(&result_path);

    assert_eq!(
        resolve_uwp_helper_path(std::path::Path::new(r"C:\Clash\clash-ui.exe")).unwrap(),
        std::path::PathBuf::from(r"C:\Clash\clash-ui-uwp-helper.exe")
    );
}

#[test]
fn converts_ipv4_cidr_bypass_to_wildcards() {
    assert_eq!(
        proxy_bypass_string(&["192.168.1.0/24".to_string(), "localhost".to_string()]),
        "192.168.1.*;localhost"
    );
}

#[test]
fn applies_uwp_changes_and_preserves_unknown_sids() {
    let target_sid = vec![1, 2, 3, 4, 5, 6, 7, 8];
    let unknown_sid = LoopbackSid {
        bytes: vec![9, 8, 7, 6, 5, 4, 3, 2],
        attributes: 23,
    };
    let target = LoopbackSid {
        bytes: target_sid.clone(),
        attributes: 11,
    };
    let mut container = test_app_container("Known", "Known_abc");
    container.sid = target_sid;
    let containers = vec![container];

    let enabled = apply_uwp_changes(
        &[unknown_sid.clone(), target.clone(), unknown_sid.clone()],
        &containers,
        &[("Known_abc".to_string(), true)],
    )
    .unwrap();
    assert_eq!(enabled, vec![unknown_sid.clone(), target.clone()]);

    let disabled =
        apply_uwp_changes(&enabled, &containers, &[("Known_abc".to_string(), false)]).unwrap();
    assert_eq!(disabled, vec![unknown_sid]);
}

#[test]
fn rejects_unknown_or_empty_uwp_package() {
    let mut container = test_app_container("Known", "Known_abc");
    container.sid = vec![1, 2, 3, 4, 5, 6, 7, 8];
    let containers = [container];
    assert!(
        apply_uwp_changes(&[], &containers, &[("Missing_abc".to_string(), true)])
            .unwrap_err()
            .contains("Missing_abc")
    );
    assert!(
        apply_uwp_changes(&[], &containers, &[("  ".to_string(), true)])
            .unwrap_err()
            .contains("不能为空")
    );
}

#[test]
fn formats_network_isolation_error_with_win32_code() {
    let error = format_network_isolation_error(
        "NetworkIsolationSetAppContainerConfig",
        ERROR_ACCESS_DENIED,
    );
    assert!(error.contains("0x00000005"));
    assert!(error.contains("权限不足"));
}

#[test]
fn empty_uwp_changes_do_not_require_system_snapshot() {
    assert!(set_uwp_loopback_batch(&[]).is_ok());
}
