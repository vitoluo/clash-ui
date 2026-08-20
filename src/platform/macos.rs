// macOS 平台专属能力。

use std::process::Child;

/// macOS 核心进程守卫；进程树由终端信号和直接子进程句柄管理。
pub struct CoreProcessGuard;

impl CoreProcessGuard {
    pub fn attach(_child: &Child) -> Result<Self, String> {
        Ok(Self)
    }

    pub fn terminate(&self) -> Result<(), String> {
        Ok(())
    }
}

pub(super) fn proxy_bypass_string(bypass: &[String]) -> String {
    bypass
        .iter()
        .map(|item| item.trim())
        .filter(|item| !item.is_empty())
        .collect::<Vec<_>>()
        .join(",")
}

/// macOS 要求 TUN 网卡名称以 utun- 开头。
pub fn normalize_tun_device(value: &str) -> String {
    let value = value.trim();
    if value.starts_with("utun-") {
        value.to_string()
    } else {
        format!("utun-{value}")
    }
}

/// macOS 不支持 UWP。
pub fn supports_uwp() -> bool {
    false
}

pub fn list_uwp_apps() -> Result<Vec<crate::platform::UwpApp>, String> {
    Err("当前平台不支持 UWP 设置".to_string())
}

pub fn set_uwp_loopback(_package_family_name: &str, _enabled: bool) -> Result<(), String> {
    Err("当前平台不支持 UWP 设置".to_string())
}

pub fn set_uwp_loopback_batch(_changes: &[(String, bool)]) -> Result<(), String> {
    Err("当前平台不支持 UWP 设置".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_utun_prefix_to_tun_device() {
        assert_eq!(normalize_tun_device("clash"), "utun-clash");
        assert_eq!(normalize_tun_device("utun-clash"), "utun-clash");
    }
}
