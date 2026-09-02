// Linux 平台专属能力。

use std::process::Child;

/// Linux 核心进程守卫；进程树由终端信号和直接子进程句柄管理。
pub struct CoreProcessGuard;

impl CoreProcessGuard {
    pub fn attach(_child: &Child) -> Result<Self, crate::platform::PlatformError> {
        Ok(Self)
    }

    pub fn terminate(&self) -> Result<(), crate::platform::PlatformError> {
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

/// Linux 不需要为 TUN 网卡名增加平台前缀。
pub fn normalize_tun_device(value: &str) -> String {
    value.trim().to_string()
}

/// Linux 不支持 UWP。
pub fn supports_uwp() -> bool {
    false
}

pub fn list_uwp_apps() -> Result<Vec<crate::platform::UwpApp>, crate::platform::PlatformError> {
    Err(crate::platform::PlatformError::unsupported(
        "当前平台不支持 UWP 设置",
    ))
}

pub fn set_uwp_loopback(
    _package_family_name: &str,
    _enabled: bool,
) -> Result<(), crate::platform::PlatformError> {
    Err(crate::platform::PlatformError::unsupported(
        "当前平台不支持 UWP 设置",
    ))
}

pub fn set_uwp_loopback_batch(
    _changes: &[(String, bool)],
) -> Result<(), crate::platform::PlatformError> {
    Err(crate::platform::PlatformError::unsupported(
        "当前平台不支持 UWP 设置",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_tun_device_whitespace() {
        assert_eq!(normalize_tun_device("  clash  "), "clash");
    }
}
