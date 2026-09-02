use std::path::PathBuf;

use crate::app::config;
use crate::clash::{api, core};
use crate::event::CoreState;
use crate::platform;

/// 目标终端类型（决定复制的环境变量命令格式）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Terminal {
    PowerShell,
    Cmd,
    Bash,
}

/// 当前 Clash 可用于系统代理的可选端口。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProxyPorts {
    pub mixed: Option<u16>,
    pub http: Option<u16>,
    pub socks: Option<u16>,
}

/// Clash API 返回的代理端点。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProxyEndpoint {
    pub(crate) host: String,
    pub(crate) ports: ProxyPorts,
}

/// 生成指定终端设置代理环境变量的命令。
pub fn proxy_env_command(
    terminal: Terminal,
    host: &str,
    http: Option<u16>,
    socks: Option<u16>,
) -> String {
    let mut lines = Vec::new();
    match terminal {
        Terminal::PowerShell => {
            if let Some(port) = http {
                lines.push(format!("$env:HTTP_PROXY=\"http://{host}:{port}\""));
                lines.push(format!("$env:HTTPS_PROXY=\"http://{host}:{port}\""));
            }
            if let Some(port) = socks {
                lines.push(format!("$env:ALL_PROXY=\"socks5://{host}:{port}\""));
            }
        }
        Terminal::Cmd => {
            if let Some(port) = http {
                lines.push(format!("set HTTP_PROXY=http://{host}:{port}"));
                lines.push(format!("set HTTPS_PROXY=http://{host}:{port}"));
            }
            if let Some(port) = socks {
                lines.push(format!("set ALL_PROXY=socks5://{host}:{port}"));
            }
        }
        Terminal::Bash => {
            if let Some(port) = http {
                lines.push(format!("export HTTP_PROXY=http://{host}:{port}"));
                lines.push(format!("export HTTPS_PROXY=http://{host}:{port}"));
            }
            if let Some(port) = socks {
                lines.push(format!("export ALL_PROXY=socks5://{host}:{port}"));
            }
        }
    }
    lines.join("\n")
}

fn map_ports(mixed_port: u16, http_port: u16, socks_port: u16) -> ProxyPorts {
    if mixed_port != 0 {
        return ProxyPorts {
            mixed: Some(mixed_port),
            http: Some(mixed_port),
            socks: Some(mixed_port),
        };
    }
    ProxyPorts {
        mixed: None,
        http: (http_port != 0).then_some(http_port),
        socks: (socks_port != 0).then_some(socks_port),
    }
}

/// 按主页展示优先级生成代理地址。
pub(crate) fn proxy_address(endpoint: &ProxyEndpoint) -> Option<String> {
    if let Some(port) = endpoint.ports.mixed {
        return Some(format!("http://{}:{port}", endpoint.host));
    }
    if let Some(port) = endpoint.ports.socks {
        return Some(format!("socks5://{}:{port}", endpoint.host));
    }
    endpoint
        .ports
        .http
        .map(|port| format!("http://{}:{port}", endpoint.host))
}

/// 从 Clash API 配置响应提取代理端点。
pub(crate) fn proxy_endpoint_from_configs(configs: &api::Configs) -> Result<ProxyEndpoint, String> {
    let host = configs.bind_address.trim();
    if host.is_empty() {
        return Err("Clash API 未返回代理地址".to_string());
    }
    Ok(ProxyEndpoint {
        host: host.to_string(),
        ports: map_ports(configs.mixed_port, configs.port, configs.socks_port),
    })
}

/// 从 Clash API 获取代理端点；失败时不使用旧配置或固定地址。
async fn proxy_endpoint(core_state: CoreState) -> Result<ProxyEndpoint, String> {
    if !is_current_core_state(core_state) {
        return Err("Clash 核心会话已变化".to_string());
    }
    let configs = api::get_configs()
        .await
        .map_err(|error| format!("获取 Clash 代理配置失败：{error}"))?;
    if !is_current_core_state(core_state) {
        return Err("Clash 核心会话已变化".to_string());
    }
    proxy_endpoint_from_configs(&configs)
}

fn current_core_state() -> CoreState {
    *crate::event::subscribe_core_state().borrow()
}

fn is_current_core_state(expected: CoreState) -> bool {
    expected.running && current_core_state() == expected
}

/// 设置系统代理开关（平台动作成功后再写配置）。
async fn set_system_proxy(enabled: bool, core_state: CoreState) -> Result<(), anyhow::Error> {
    if !is_current_core_state(core_state) {
        return Err(anyhow::anyhow!("Clash 核心会话已变化"));
    }
    let bypass_list = config::get().settings.proxy.bypass_list;
    apply_system_proxy(enabled, bypass_list, core_state)
        .await
        .map_err(|e| anyhow::anyhow!(e))?;
    if !is_current_core_state(core_state) {
        return Err(anyhow::anyhow!("Clash 核心会话已变化"));
    }
    config::update(|cfg| cfg.proxy_status.system = enabled);
    crate::event::publish_system_proxy(enabled);
    if !is_current_core_state(core_state) {
        return Err(anyhow::anyhow!("Clash 核心会话已变化"));
    }
    api::close_all_connections().await?;
    Ok(())
}

async fn apply_system_proxy(
    enabled: bool,
    bypass_list: Vec<String>,
    core_state: CoreState,
) -> Result<(), String> {
    if enabled {
        let endpoint = proxy_endpoint(core_state).await?;
        if !is_current_core_state(core_state) {
            return Err("Clash 核心会话已变化".to_string());
        }
        platform::set_system_proxy(
            &endpoint.host,
            true,
            endpoint.ports.http,
            endpoint.ports.socks,
            &bypass_list,
        )
        .map_err(|error| format!("设置系统代理任务失败：{error}"))?;
    } else {
        if !is_current_core_state(core_state) {
            return Err("Clash 核心会话已变化".to_string());
        }
        clear_system_proxy().map_err(|error| format!("清除系统代理任务失败：{error}"))?
    }
    Ok(())
}

/// 按当前 Clash 配置恢复持久化的系统代理状态。
pub fn restore_system_proxy(core_state: CoreState) {
    if !is_current_core_state(core_state) {
        return;
    }
    let cfg = config::get();
    match core::is_ready() {
        Ok(true) => {}
        Ok(false) => return,
        Err(error) => {
            crate::log::error(format_args!("恢复系统代理时读取核心状态失败：{error}"));
            return;
        }
    }
    if !cfg.proxy_status.system {
        return;
    }
    let bypass_list = cfg.settings.proxy.bypass_list;
    crate::runtime::spawn_task(async move {
        match restore_system_proxy_async(bypass_list, core_state).await {
            Ok(()) => {}
            Err(error) => {
                crate::log::error(format_args!("核心启动时恢复系统代理失败：{error}"));
            }
        }
        if is_current_core_state(core_state) {
            crate::event::publish_system_proxy(config::proxy_status().system);
        }
    });
}

async fn restore_system_proxy_async(
    bypass_list: Vec<String>,
    core_state: CoreState,
) -> Result<(), String> {
    let endpoint = proxy_endpoint(core_state).await?;
    if !is_current_core_state(core_state) {
        return Err("Clash 核心会话已变化".to_string());
    }
    crate::runtime::spawn_blocking(move || {
        platform::set_system_proxy(
            &endpoint.host,
            true,
            endpoint.ports.http,
            endpoint.ports.socks,
            &bypass_list,
        )
        .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| format!("恢复系统代理任务失败：{error}"))?
}

/// 清除平台系统代理，不读取 Clash API，也不修改持久化意图。
pub fn clear_system_proxy() -> Result<(), String> {
    platform::set_system_proxy("", false, None, None, &[]).map_err(|error| error.to_string())
}

/// 切换系统代理（供托盘与主页复用）。
pub fn toggle_system_proxy() {
    let core_state = current_core_state();
    crate::runtime::block(async {
        if let Err(e) = set_system_proxy(!config::get().proxy_status.system, core_state).await {
            crate::log::error(format_args!("切换系统代理失败: {e}"));
        }
    })
}

/// 设置 TUN 代理开关（写配置并重启核心注入 tun.enable）。
pub fn set_tun(root: PathBuf, enabled: bool) {
    crate::runtime::spawn_blocking(move || {
        if enabled && !platform::is_admin() {
            crate::event::request_tun_confirmation();
            return;
        }

        config::update(|c| c.proxy_status.tun = enabled);
        if let Err(error) = core::restart_core(&root) {
            crate::log::error(format_args!("应用 TUN 配置失败: {error}"));
        }
        crate::event::publish_tun_proxy(enabled);
    });
}

/// 切换 TUN 代理（供托盘与主页复用）。
pub fn toggle_tun(root: PathBuf) {
    set_tun(root, !config::get().proxy_status.tun);
}

/// 设置 Clash 出站模式（失败仅打印日志）。
pub fn set_mode(mode: &str) {
    let core_state = current_core_state();
    if !is_current_core_state(core_state) {
        return;
    }
    match core::get_port() {
        Ok(Some(_)) => {}
        Ok(None) => {
            crate::log::error(format_args!("设置出站模式失败：Clash 核心未运行"));
            return;
        }
        Err(error) => {
            crate::log::error(format_args!("设置出站模式失败：读取核心端口失败：{error}"));
            return;
        }
    }
    let mode = mode.to_string();
    crate::runtime::spawn_task(async move {
        if !is_current_core_state(core_state) {
            return;
        }
        match api::put_mode(&mode).await {
            Ok(()) => {
                if !is_current_core_state(core_state) {
                    return;
                }
                crate::event::publish_outbound_mode(&mode);
                close_all_connections_after_change("出站模式", core_state).await;
            }
            Err(error) => {
                crate::log::error(format_args!("设置出站模式 {mode} 失败: {error}"));
            }
        }
    });
}

async fn close_all_connections_after_change(change: &str, core_state: CoreState) {
    if !is_current_core_state(core_state) {
        return;
    }
    match core::is_ready() {
        Ok(true) => {}
        Ok(false) => return,
        Err(error) => {
            crate::log::error(format_args!("{change}变更后读取核心状态失败：{error}"));
            return;
        }
    }
    if !is_current_core_state(core_state) {
        return;
    }
    if let Err(error) = api::close_all_connections().await {
        crate::log::error(format_args!("{change}变更后关闭全部连接失败：{error}"));
    }
}

/// 复制指定终端的代理环境变量命令到剪贴板。
pub fn copy_proxy_env(terminal: Terminal) {
    let core_state = current_core_state();
    if !is_current_core_state(core_state) {
        return;
    }
    match core::get_port() {
        Ok(Some(_)) => {}
        Ok(None) => {
            crate::log::error(format_args!("获取代理环境变量失败：Clash 核心未运行"));
            return;
        }
        Err(error) => {
            crate::log::error(format_args!(
                "获取代理环境变量失败：读取核心端口失败：{error}"
            ));
            return;
        }
    }
    crate::runtime::spawn_task(async move {
        let endpoint = match proxy_endpoint(core_state).await {
            Ok(endpoint) => endpoint,
            Err(error) => {
                crate::log::error(format_args!("获取代理环境变量失败：{error}"));
                return;
            }
        };
        if endpoint.ports.http.is_none() && endpoint.ports.socks.is_none() {
            crate::log::error(format_args!("没有可用的代理端口，无法复制代理环境变量"));
            return;
        }
        let command = proxy_env_command(
            terminal,
            &endpoint.host,
            endpoint.ports.http,
            endpoint.ports.socks,
        );
        if !is_current_core_state(core_state) {
            return;
        }
        match crate::runtime::spawn_blocking(move || platform::set_clipboard_text(&command)).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                crate::log::error(format_args!("复制代理环境变量失败：{error}"));
            }
            Err(error) => {
                crate::log::error(format_args!("复制代理环境变量任务失败：{error}"));
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixed_port_overrides_separate_ports() {
        assert_eq!(
            map_ports(7890, 7891, 7892),
            ProxyPorts {
                mixed: Some(7890),
                http: Some(7890),
                socks: Some(7890)
            }
        );
    }

    #[test]
    fn preserves_empty_separate_ports() {
        assert_eq!(
            map_ports(0, 7891, 0),
            ProxyPorts {
                mixed: None,
                http: Some(7891),
                socks: None
            }
        );
    }

    #[test]
    fn does_not_fill_default_ports_when_empty() {
        assert_eq!(
            map_ports(0, 0, 0),
            ProxyPorts {
                mixed: None,
                http: None,
                socks: None
            }
        );
    }

    fn api_configs(bind_address: &str) -> api::Configs {
        serde_json::from_value(serde_json::json!({
            "bind-address": bind_address,
            "port": 7890,
            "mixed-port": 0,
            "socks-port": 7891
        }))
        .unwrap()
    }

    #[test]
    fn proxy_endpoint_uses_api_bind_address() {
        let endpoint = proxy_endpoint_from_configs(&api_configs("192.168.1.20")).unwrap();
        assert_eq!(endpoint.host, "192.168.1.20");
        assert_eq!(endpoint.ports.mixed, None);
        assert_eq!(endpoint.ports.http, Some(7890));
        assert_eq!(endpoint.ports.socks, Some(7891));
    }

    #[test]
    fn api_missing_proxy_address_returns_error() {
        assert!(proxy_endpoint_from_configs(&api_configs(" ")).is_err());
    }

    #[test]
    fn selects_home_address_by_port_source_priority() {
        let endpoint = |ports| ProxyEndpoint {
            host: "192.168.1.20".to_string(),
            ports,
        };
        assert_eq!(
            proxy_address(&endpoint(map_ports(7890, 7891, 7892))),
            Some("http://192.168.1.20:7890".to_string())
        );
        assert_eq!(
            proxy_address(&endpoint(map_ports(0, 7891, 7892))),
            Some("socks5://192.168.1.20:7892".to_string())
        );
        assert_eq!(
            proxy_address(&endpoint(map_ports(0, 7891, 0))),
            Some("http://192.168.1.20:7891".to_string())
        );
        assert_eq!(proxy_address(&endpoint(map_ports(0, 0, 0))), None);
    }

    #[test]
    fn formats_powershell_proxy_command() {
        let s = proxy_env_command(Terminal::PowerShell, "192.168.1.20", Some(7890), Some(7891));
        assert!(s.contains("$env:HTTP_PROXY=\"http://192.168.1.20:7890\""));
        assert!(s.contains("$env:HTTPS_PROXY=\"http://192.168.1.20:7890\""));
        assert!(s.contains("$env:ALL_PROXY=\"socks5://192.168.1.20:7891\""));
    }

    #[test]
    fn formats_cmd_proxy_command() {
        let s = proxy_env_command(Terminal::Cmd, "192.168.1.20", Some(7890), Some(7891));
        assert!(s.contains("set HTTP_PROXY=http://192.168.1.20:7890"));
        assert!(s.contains("set HTTPS_PROXY=http://192.168.1.20:7890"));
        assert!(s.contains("set ALL_PROXY=socks5://192.168.1.20:7891"));
    }

    #[test]
    fn formats_bash_proxy_command() {
        let s = proxy_env_command(Terminal::Bash, "192.168.1.20", Some(7890), Some(7891));
        assert!(s.contains("export HTTP_PROXY=http://192.168.1.20:7890"));
        assert!(s.contains("export HTTPS_PROXY=http://192.168.1.20:7890"));
        assert!(s.contains("export ALL_PROXY=socks5://192.168.1.20:7891"));
    }

    #[test]
    fn formats_single_protocol_commands_without_missing_variables() {
        for terminal in [Terminal::PowerShell, Terminal::Cmd, Terminal::Bash] {
            let http = proxy_env_command(terminal, "host", Some(7890), None);
            assert!(http.contains("HTTP_PROXY"));
            assert!(http.contains("HTTPS_PROXY"));
            assert!(!http.contains("ALL_PROXY"));

            let socks = proxy_env_command(terminal, "host", None, Some(7891));
            assert!(!socks.contains("HTTP_PROXY"));
            assert!(!socks.contains("HTTPS_PROXY"));
            assert!(socks.contains("ALL_PROXY"));
        }
        assert!(proxy_env_command(Terminal::Bash, "host", None, None).is_empty());
    }
}
