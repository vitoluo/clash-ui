// 系统托盘：状态和菜单由 Slint SystemTrayIcon 管理，动作复用主页业务逻辑。

use std::cell::RefCell;
use std::path::PathBuf;

use crate::app::config;
use crate::clash::{api, core};
use crate::platform;
use crate::{ClashTray, MainWindow};
use slint::ComponentHandle;

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

// ===== 主线程局部缓存 =====
thread_local! {
    static TRAY: RefCell<Option<slint::Weak<ClashTray>>> = const { RefCell::new(None) };
    static ROOT: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
    static WINDOW: RefCell<Option<slint::Weak<MainWindow>>> = const { RefCell::new(None) };
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
pub(crate) fn proxy_endpoint() -> Result<ProxyEndpoint, String> {
    let configs =
        api::get_configs().map_err(|error| format!("获取 Clash 代理配置失败：{error}"))?;
    proxy_endpoint_from_configs(&configs)
}

fn tray() -> Option<ClashTray> {
    TRAY.with(|tray| tray.borrow().as_ref().and_then(|weak| weak.upgrade()))
}

fn refresh_home_proxy_status() {
    let status = config::get().proxy_status;
    if let Some(tray) = tray() {
        tray.set_system_proxy(status.system);
        tray.set_tun_proxy(status.tun);
    }

    let window = WINDOW.with(|w| w.borrow().clone().and_then(|weak| weak.upgrade()));
    if let Some(window) = window {
        let home = window.global::<crate::HomeModel>();
        home.set_system_proxy(status.system);
        home.set_tun_proxy(status.tun);
        home.set_core_running(core::get_port().is_some());
    }
    refresh_runtime_state();
}

/// 根据核心会话状态刷新主页和托盘代理操作的可用性。
pub fn refresh_runtime_state() {
    let enabled = core::get_port().is_some();
    if let Some(tray) = tray() {
        tray.set_core_running(enabled);
    }
}

/// 同步托盘当前出站模式。
pub fn set_outbound_mode(mode: &str) {
    if let Some(tray) = tray() {
        tray.set_rule_mode_checked(mode == "rule");
        tray.set_global_mode_checked(mode == "global");
        tray.set_direct_mode_checked(mode == "direct");
    }
}

/// 设置系统代理开关（平台动作成功后再写配置）。
pub fn set_system_proxy(enabled: bool) {
    let cfg = config::get();
    let result = if enabled {
        proxy_endpoint().and_then(|endpoint| {
            platform::set_system_proxy(
                &endpoint.host,
                true,
                endpoint.ports.http,
                endpoint.ports.socks,
                &cfg.settings.proxy.bypass_list,
            )
        })
    } else {
        clear_system_proxy()
    };
    if let Err(error) = result {
        crate::log::error(format_args!("设置系统代理失败：{error}"));
        refresh_home_proxy_status();
        return;
    }
    config::update(|c| c.proxy_status.system = enabled);
    refresh_home_proxy_status();
}

/// 按当前 Clash 配置恢复持久化的系统代理状态。
pub fn restore_system_proxy() {
    let cfg = config::get();
    if !cfg.proxy_status.system {
        return;
    }
    let result = proxy_endpoint().and_then(|endpoint| {
        platform::set_system_proxy(
            &endpoint.host,
            true,
            endpoint.ports.http,
            endpoint.ports.socks,
            &cfg.settings.proxy.bypass_list,
        )
    });
    if let Err(error) = result {
        crate::log::error(format_args!("启动时恢复系统代理失败：{error}"));
        config::update(|c| c.proxy_status.system = false);
    }
    refresh_home_proxy_status();
}

/// 清除平台系统代理，不读取 Clash API，也不修改持久化意图。
pub fn clear_system_proxy() -> Result<(), String> {
    platform::set_system_proxy("", false, None, None, &[])
}

/// 切换系统代理（供托盘与主页复用）。
pub fn toggle_system_proxy() {
    set_system_proxy(!config::get().proxy_status.system);
}

/// 设置 TUN 代理开关（写配置并重启核心注入 tun.enable）。
pub fn set_tun(enabled: bool) {
    if enabled && !platform::is_admin() {
        WINDOW.with(|w| {
            if let Some(window) = w.borrow().as_ref().and_then(|weak| weak.upgrade()) {
                let _ = window.show();
                window
                    .global::<crate::AppState>()
                    .set_tun_confirm_open(true);
            }
        });
        refresh_home_proxy_status();
        return;
    }

    config::update(|c| c.proxy_status.tun = enabled);
    let root = ROOT.with(|r| r.borrow().clone());
    if let Some(root) = root {
        if let Err(e) = core::restart_core(&root) {
            crate::log::error(format_args!("应用 TUN 配置失败: {e}"));
        }
    }
    refresh_home_proxy_status();
}

/// 确认非管理员开启 TUN，失败时回滚配置并恢复系统代理。
pub fn confirm_tun_enable() {
    config::update(|c| c.proxy_status.tun = true);
    platform::request_elevation();
    quit();
}

/// 取消非管理员 TUN 确认，不修改配置或触发提权。
pub fn cancel_tun_enable() {
    WINDOW.with(|w| {
        if let Some(window) = w.borrow().as_ref().and_then(|weak| weak.upgrade()) {
            window
                .global::<crate::AppState>()
                .set_tun_confirm_open(false);
        }
    });
    refresh_home_proxy_status();
}

/// 切换 TUN 代理（供托盘与主页复用）。
pub fn toggle_tun() {
    set_tun(!config::get().proxy_status.tun);
}

/// 设置 clash 出站模式（失败仅打印日志）。
pub fn set_mode(mode: &str) {
    if core::get_port().is_none() {
        crate::log::error(format_args!("设置出站模式失败：Clash 核心未运行"));
        return;
    }
    if let Err(e) = api::put_mode(mode) {
        crate::log::error(format_args!("设置出站模式 {mode} 失败: {e}"));
        return;
    }
    set_outbound_mode(mode);
}

/// 复制指定终端的代理环境变量命令到剪贴板。
pub fn copy_proxy_env(terminal: Terminal) {
    if core::get_port().is_none() {
        crate::log::error(format_args!("获取代理环境变量失败：Clash 核心未运行"));
        return;
    }
    let endpoint = match proxy_endpoint() {
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
    platform::set_clipboard_text(&proxy_env_command(
        terminal,
        &endpoint.host,
        endpoint.ports.http,
        endpoint.ports.socks,
    ));
}

/// 显示主界面（经事件循环线程操作窗口）。
pub fn show_main() {
    let _ = slint::invoke_from_event_loop(|| {
        WINDOW.with(|w| {
            if let Some(weak) = w.borrow().as_ref() {
                if let Some(win) = weak.upgrade() {
                    let _ = win.show();
                }
            }
        });
    });
}

/// 退出：清除系统代理、停止核心并退出事件循环。
pub fn quit() {
    if config::get().proxy_status.system {
        if let Err(error) = clear_system_proxy() {
            crate::log::error(format_args!("退出时清除系统代理失败：{error}"));
        }
    }
    core::stop_core();
    let _ = slint::quit_event_loop();
}

/// 初始化 Slint 系统托盘。托盘不可用时仍保留 root/window 状态，确保主界面业务可用。
pub fn init(root: PathBuf, window: slint::Weak<MainWindow>, tray: Option<&ClashTray>) {
    ROOT.with(|value| *value.borrow_mut() = Some(root));
    WINDOW.with(|value| *value.borrow_mut() = Some(window));

    let Some(tray) = tray else {
        refresh_home_proxy_status();
        return;
    };

    TRAY.with(|value| *value.borrow_mut() = Some(tray.as_weak()));
    tray.on_show_main(show_main);
    tray.on_copy_env(|index| match index {
        0 => copy_proxy_env(Terminal::PowerShell),
        1 => copy_proxy_env(Terminal::Cmd),
        2 => copy_proxy_env(Terminal::Bash),
        _ => crate::log::error(format_args!("未知的终端类型索引：{index}")),
    });
    tray.on_change_mode(|mode| set_mode(mode.as_str()));
    tray.on_toggle_system_proxy(toggle_system_proxy);
    tray.on_toggle_tun(toggle_tun);
    tray.on_quit(quit);
    refresh_home_proxy_status();
    if let Ok(configs) = api::get_configs() {
        set_outbound_mode(&configs.mode);
    }
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
