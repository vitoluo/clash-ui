// Windows UWP 应用枚举和回环配置。

use std::ffi::c_void;
use std::ptr;
use std::slice;

use super::uwp_api::{
    self, FirewallApi, SidAndAttributes, ERROR_SUCCESS, NETISO_FLAG_FORCE_COMPUTE_BINARIES,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct LoopbackSid {
    pub(super) bytes: Vec<u8>,
    pub(super) attributes: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct AppContainer {
    pub(super) name: String,
    pub(super) package_family_name: String,
    pub(super) sid: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct UwpSnapshot {
    containers: Vec<AppContainer>,
    loopback_sids: Vec<LoopbackSid>,
}

fn deduplicate_loopback_sids(entries: Vec<LoopbackSid>) -> Vec<LoopbackSid> {
    let mut unique = Vec::with_capacity(entries.len());
    for entry in entries {
        if unique
            .iter()
            .all(|current: &LoopbackSid| current.bytes != entry.bytes)
        {
            unique.push(entry);
        }
    }
    unique
}

fn usable_uwp_name(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    (!value.is_empty() && !value.starts_with("@{") && !value.starts_with("ms-resource:"))
        .then(|| value.to_string())
}

pub(super) fn resolve_uwp_display_name_with<F>(
    display_name: Option<&str>,
    app_container_name: &str,
    package_full_name: &str,
    resolve_indirect: F,
) -> String
where
    F: Fn(&str) -> Option<String>,
{
    if let Some(name) = usable_uwp_name(display_name) {
        return name;
    }
    if let Some(display_name) = display_name.map(str::trim) {
        let resource = if display_name.starts_with("@{") {
            Some(display_name.to_string())
        } else if display_name.starts_with("ms-resource:") && !package_full_name.is_empty() {
            Some(format!("@{{{package_full_name}?{display_name}}}"))
        } else {
            None
        };
        if let Some(resource) = resource.and_then(|value| resolve_indirect(&value)) {
            if let Some(name) = usable_uwp_name(Some(&resource)) {
                return name;
            }
        }
    }
    usable_uwp_name(Some(app_container_name))
        .or_else(|| usable_uwp_name(Some(package_full_name)))
        .unwrap_or_else(|| package_full_name.to_string())
}

pub(super) fn resolve_uwp_display_name(
    display_name: Option<&str>,
    app_container_name: &str,
    package_full_name: &str,
) -> String {
    resolve_uwp_display_name_with(
        display_name,
        app_container_name,
        package_full_name,
        uwp_api::load_indirect_string,
    )
}

fn enumerate_app_containers(api: &FirewallApi) -> Result<Vec<AppContainer>, String> {
    let mut last_error = None;
    for flags in [NETISO_FLAG_FORCE_COMPUTE_BINARIES, 0] {
        let mut count = 0;
        let mut containers = ptr::null_mut();
        let result = api.enum_app_containers(flags, &mut count, &mut containers);
        if result == ERROR_SUCCESS {
            if count > 0 && containers.is_null() {
                return Err("NetworkIsolationEnumAppContainers 返回空容器数组".to_string());
            }
            let items = if containers.is_null() {
                Vec::new()
            } else {
                unsafe {
                    // count 来自同一次成功的 Windows API 调用，指向连续结构数组。
                    slice::from_raw_parts(containers, count as usize)
                        .iter()
                        .filter_map(|container| {
                            let app_container_name =
                                uwp_api::read_utf16(container.app_container_name)?;
                            let package_full_name =
                                uwp_api::read_utf16(container.package_full_name)
                                    .unwrap_or_default();
                            let package_family_name = app_container_name.clone();
                            let name = resolve_uwp_display_name(
                                uwp_api::read_utf16(container.display_name).as_deref(),
                                &app_container_name,
                                &package_full_name,
                            );
                            let sid = uwp_api::copy_sid(container.app_container_sid)?;
                            Some(AppContainer {
                                name,
                                package_family_name,
                                sid,
                            })
                        })
                        .collect()
                }
            };
            if !containers.is_null() {
                // 释放枚举数组及其嵌套字段。
                let _ = api.free_app_containers(containers);
            }
            let mut unique = Vec::with_capacity(items.len());
            for item in items {
                if unique.iter().all(|current: &AppContainer| {
                    current.package_family_name != item.package_family_name
                }) {
                    unique.push(item);
                }
            }
            return Ok(unique);
        }
        last_error = Some(uwp_api::format_network_isolation_error(
            "NetworkIsolationEnumAppContainers",
            result,
        ));
        if flags != 0 && uwp_api::is_access_denied(result) {
            continue;
        }
        break;
    }
    Err(last_error.unwrap_or_else(|| "枚举 UWP 应用容器失败".to_string()))
}

fn read_current_loopback_sids(api: &FirewallApi) -> Result<Vec<LoopbackSid>, String> {
    let mut count = 0;
    let mut sids = ptr::null_mut();
    let result = api.get_app_container_config(&mut count, &mut sids);
    if result != ERROR_SUCCESS {
        return Err(uwp_api::format_network_isolation_error(
            "NetworkIsolationGetAppContainerConfig",
            result,
        ));
    }
    if count > 0 && sids.is_null() {
        return Err("NetworkIsolationGetAppContainerConfig 返回空 SID 数组".to_string());
    }
    let entries = if sids.is_null() {
        Vec::new()
    } else {
        unsafe {
            // count 与 sids 由同一次成功调用返回，逐项复制后再释放原始缓冲区。
            slice::from_raw_parts(sids, count as usize)
                .iter()
                .filter_map(|entry| {
                    Some(LoopbackSid {
                        bytes: uwp_api::copy_sid(entry.sid)?,
                        attributes: entry.attributes,
                    })
                })
                .collect()
        }
    };
    if !sids.is_null() {
        unsafe {
            uwp_api::free_loopback_sid_config(count, sids);
        }
    }
    Ok(deduplicate_loopback_sids(entries))
}

fn load_uwp_snapshot(api: &FirewallApi) -> Result<UwpSnapshot, String> {
    Ok(UwpSnapshot {
        containers: enumerate_app_containers(api)?,
        loopback_sids: read_current_loopback_sids(api)?,
    })
}

pub(super) fn apply_uwp_changes(
    current: &[LoopbackSid],
    containers: &[AppContainer],
    changes: &[(String, bool)],
) -> Result<Vec<LoopbackSid>, String> {
    let mut result = deduplicate_loopback_sids(current.to_vec());
    for (package_family_name, enabled) in changes {
        let package_family_name = package_family_name.trim();
        if package_family_name.is_empty() {
            return Err("UWP 包族名称不能为空".to_string());
        }
        let container = containers
            .iter()
            .find(|container| container.package_family_name == package_family_name)
            .ok_or_else(|| format!("未找到 UWP 包族名：{package_family_name}"))?;
        let attributes = result
            .iter()
            .find(|entry| entry.bytes == container.sid)
            .map(|entry| entry.attributes)
            .unwrap_or(0);
        result.retain(|entry| entry.bytes != container.sid);
        if *enabled {
            result.push(LoopbackSid {
                bytes: container.sid.clone(),
                attributes,
            });
        }
    }
    Ok(deduplicate_loopback_sids(result))
}

fn set_loopback_sid_config(api: &FirewallApi, entries: &[LoopbackSid]) -> Result<(), String> {
    let count = u32::try_from(entries.len())
        .map_err(|_| "UWP 回环 SID 数量超过 Windows API 上限".to_string())?;
    let sid_storage = entries
        .iter()
        .map(|entry| entry.bytes.clone())
        .collect::<Vec<_>>();
    let sid_attributes = sid_storage
        .iter()
        .zip(entries)
        .map(|(sid, entry)| SidAndAttributes {
            sid: sid.as_ptr() as *mut c_void,
            attributes: entry.attributes,
        })
        .collect::<Vec<_>>();
    let pointer = if sid_attributes.is_empty() {
        ptr::null()
    } else {
        sid_attributes.as_ptr()
    };
    let result = api.set_app_container_config(count, pointer);
    if result == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(uwp_api::format_network_isolation_error(
            "NetworkIsolationSetAppContainerConfig",
            result,
        ))
    }
}

pub(super) fn uwp_apps_from_containers(
    containers: Vec<AppContainer>,
    loopback_sids: &[LoopbackSid],
) -> Vec<crate::platform::UwpApp> {
    let mut apps = containers
        .into_iter()
        .map(|container| crate::platform::UwpApp {
            name: container.name,
            package_family_name: container.package_family_name,
            enabled: loopback_sids
                .iter()
                .any(|entry| entry.bytes == container.sid),
        })
        .collect::<Vec<_>>();
    apps.sort_by(|left, right| {
        left.name
            .to_ascii_lowercase()
            .cmp(&right.name.to_ascii_lowercase())
            .then_with(|| left.name.cmp(&right.name))
            .then_with(|| left.package_family_name.cmp(&right.package_family_name))
    });
    apps
}

pub(super) fn set_uwp_loopback_batch_impl(changes: &[(String, bool)]) -> Result<(), String> {
    if changes.is_empty() {
        return Ok(());
    }
    let api = FirewallApi::load()?;
    let snapshot = load_uwp_snapshot(&api)?;
    let next_sids = apply_uwp_changes(&snapshot.loopback_sids, &snapshot.containers, changes)?;
    set_loopback_sid_config(&api, &next_sids)
}

/// Windows 支持 UWP 回环代理设置。
pub fn supports_uwp() -> bool {
    true
}

/// 枚举当前用户安装的 UWP 应用。
pub fn list_uwp_apps() -> Result<Vec<crate::platform::UwpApp>, String> {
    let api = FirewallApi::load()?;
    let snapshot = load_uwp_snapshot(&api)?;
    Ok(uwp_apps_from_containers(
        snapshot.containers,
        &snapshot.loopback_sids,
    ))
}

/// 修改单个 UWP 应用的回环豁免状态。
#[allow(dead_code)]
pub fn set_uwp_loopback(package_family_name: &str, enabled: bool) -> Result<(), String> {
    set_uwp_loopback_batch(&[(package_family_name.to_string(), enabled)])
}

/// 在一次提权流程中批量修改 UWP 应用的回环豁免状态。
pub fn set_uwp_loopback_batch(changes: &[(String, bool)]) -> Result<(), String> {
    if changes.is_empty() {
        return Ok(());
    }
    if crate::platform::is_admin() {
        set_uwp_loopback_batch_impl(changes)
    } else {
        super::uwp_elevation::set_uwp_loopback_elevated(changes)
    }
}
