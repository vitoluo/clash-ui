// 系统托盘：图标按 (系统代理, TUN) 四态切换，单击弹出三段菜单快捷控制。
//
// 设计要点：
// - 菜单项句柄（CheckMenuItem）内部基于 Rc，非 Send/Sync，不可存入跨线程 static。
//   本应用所有托盘动作均在主线程触发/执行，故用 thread_local 缓存句柄，
//   事件分发闭包仅按 event.id 匹配、不捕获任何句柄（从而满足 Send+Sync 约束）。
// - 动作函数导出供 task 008（主页同逻辑控件）复用。

use std::cell::RefCell;
use std::path::PathBuf;

use tray_icon::menu::{
    CheckMenuItem, CheckMenuItemBuilder, IsMenuItem, Menu, MenuEvent, MenuId, MenuItem,
    MenuItemBuilder, PredefinedMenuItem, Submenu, SubmenuBuilder,
};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

use crate::app::config;
use crate::clash::{api, core};
use crate::constants::TRAY_ICONS;
use crate::platform;
use crate::MainWindow;
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

// ===== 主线程局部缓存（句柄非 Send/Sync，仅主线程访问）=====
thread_local! {
    static TRAY: RefCell<Option<TrayIcon>> = const { RefCell::new(None) };
    static COPY_MENU: RefCell<Option<Submenu>> = const { RefCell::new(None) };
    static COPY_ITEMS: RefCell<Option<[MenuItem; 3]>> = const { RefCell::new(None) };
    static MODE_MENU: RefCell<Option<Submenu>> = const { RefCell::new(None) };
    static MODE_ITEMS: RefCell<Option<[MenuItem; 3]>> = const { RefCell::new(None) };
    static SYS_ITEM: RefCell<Option<CheckMenuItem>> = const { RefCell::new(None) };
    static TUN_ITEM: RefCell<Option<CheckMenuItem>> = const { RefCell::new(None) };
    static ICONS: RefCell<Option<[Vec<u8>; 4]>> = const { RefCell::new(None) };
    static ROOT: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
    static WINDOW: RefCell<Option<slint::Weak<MainWindow>>> = const { RefCell::new(None) };
}

/// 当前图标索引：system 置位 0，tun 置位 1。顺序 [灰, 绿, 蓝, 白]。
fn current_index() -> usize {
    let s = config::get().proxy_status;
    (s.system as usize) | ((s.tun as usize) << 1)
}

/// 将 RGBA 预乘像素还原为直 alpha（resvg/tiny_skia 输出为预乘）。
fn unpremultiply(src: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(src.len());
    for chunk in src.chunks_exact(4) {
        let (r, g, b, a) = (chunk[0], chunk[1], chunk[2], chunk[3]);
        if a == 255 || a == 0 {
            out.extend_from_slice(chunk);
        } else {
            let inv = 255.0 / a as f32;
            out.push((r as f32 * inv) as u8);
            out.push((g as f32 * inv) as u8);
            out.push((b as f32 * inv) as u8);
            out.push(a);
        }
    }
    out
}

/// 将 SVG 文本光栅化为 64×64 的 RGBA 字节（已去预乘）。
fn rasterize(svg: &str) -> Option<Vec<u8>> {
    let tree = resvg::usvg::Tree::from_str(svg, &resvg::usvg::Options::default()).ok()?;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(64, 64)?;
    let scale = 64.0 / 512.0;
    let transform = resvg::tiny_skia::Transform::from_scale(scale, scale);
    let mut pixmap_mut = pixmap.as_mut();
    resvg::render(&tree, transform, &mut pixmap_mut);
    Some(unpremultiply(pixmap.data()))
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

/// 按当前状态刷新托盘图标。
pub fn refresh_icon() {
    let idx = current_index();
    let rgba = ICONS.with(|i| i.borrow().as_ref().map(|a| a[idx].clone()));
    if let Some(rgba) = rgba {
        if let Ok(icon) = Icon::from_rgba(rgba, 64, 64) {
            TRAY.with(|t| {
                if let Some(tray) = t.borrow().as_ref() {
                    let _ = tray.set_icon(Some(icon));
                }
            });
        }
    }
}

fn refresh_home_proxy_status() {
    let status = config::get().proxy_status;
    SYS_ITEM.with(|item| {
        if let Some(item) = item.borrow().as_ref() {
            item.set_checked(status.system);
        }
    });
    TUN_ITEM.with(|item| {
        if let Some(item) = item.borrow().as_ref() {
            item.set_checked(status.tun);
        }
    });

    let window = WINDOW.with(|w| w.borrow().clone().and_then(|weak| weak.upgrade()));
    let Some(window) = window else {
        return;
    };

    let home = window.global::<crate::HomeModel>();
    home.set_system_proxy(status.system);
    home.set_tun_proxy(status.tun);
    home.set_core_running(core::get_port().is_some());
    refresh_runtime_state();
}

/// 根据核心会话状态刷新主页和托盘代理操作的可用性。
pub fn refresh_runtime_state() {
    let enabled = core::get_port().is_some();
    COPY_MENU.with(|menu| {
        if let Some(menu) = menu.borrow().as_ref() {
            menu.set_enabled(enabled);
        }
    });
    COPY_ITEMS.with(|items| {
        if let Some(items) = items.borrow().as_ref() {
            for item in items {
                item.set_enabled(enabled);
            }
        }
    });
    MODE_MENU.with(|menu| {
        if let Some(menu) = menu.borrow().as_ref() {
            menu.set_enabled(enabled);
        }
    });
    MODE_ITEMS.with(|items| {
        if let Some(items) = items.borrow().as_ref() {
            for item in items {
                item.set_enabled(enabled);
            }
        }
    });
    SYS_ITEM.with(|item| {
        if let Some(item) = item.borrow().as_ref() {
            item.set_enabled(enabled);
        }
    });
    TUN_ITEM.with(|item| {
        if let Some(item) = item.borrow().as_ref() {
            item.set_enabled(enabled);
        }
    });
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
        eprintln!("设置系统代理失败：{error}");
        return;
    }
    config::update(|c| c.proxy_status.system = enabled);
    refresh_home_proxy_status();
    refresh_icon();
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
        eprintln!("启动时恢复系统代理失败：{error}");
        return;
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

/// 设置 TUN 代理开关（写配置 + 重启核心注入 tun.enable + 同步勾选 + 刷新图标）。
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
        return;
    }

    config::update(|c| c.proxy_status.tun = enabled);
    let root = ROOT.with(|r| r.borrow().clone());
    if let Some(root) = root {
        if let Err(e) = core::restart_core(&root) {
            eprintln!("应用 TUN 配置失败: {e}");
        }
    }
    refresh_home_proxy_status();
    refresh_icon();
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
        eprintln!("设置出站模式失败：Clash 核心未运行");
        return;
    }
    if let Err(e) = api::put_mode(mode) {
        eprintln!("设置出站模式 {mode} 失败: {e}");
    }
}

/// 复制指定终端的代理环境变量命令到剪贴板。
pub fn copy_proxy_env(terminal: Terminal) {
    if core::get_port().is_none() {
        eprintln!("获取代理环境变量失败：Clash 核心未运行");
        return;
    }
    let endpoint = match proxy_endpoint() {
        Ok(endpoint) => endpoint,
        Err(error) => {
            eprintln!("获取代理环境变量失败：{error}");
            return;
        }
    };
    if endpoint.ports.http.is_none() && endpoint.ports.socks.is_none() {
        eprintln!("没有可用的代理端口，无法复制代理环境变量");
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
            eprintln!("退出时清除系统代理失败：{error}");
        }
    }
    core::stop_core();
    let _ = slint::quit_event_loop();
}

/// 初始化托盘：光栅化图标、构建菜单、注册事件分发。须在 Slint 主线程调用。
pub fn init(root: PathBuf, window: slint::Weak<MainWindow>) {
    let Some(init_icon) = load_icons() else {
        return;
    };
    let (menu, handles) = build_menu(config::get().proxy_status);
    let Some(tray) = create_tray(menu, init_icon) else {
        return;
    };
    cache_handles(root, window, tray, handles);
    register_menu_events();
    refresh_runtime_state();
}

fn load_icons() -> Option<Icon> {
    let mut rgba_icons: [Vec<u8>; 4] = [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
    for (i, svg) in TRAY_ICONS.iter().enumerate() {
        match rasterize(svg) {
            Some(rgba) => rgba_icons[i] = rgba,
            None => eprintln!("光栅化托盘图标 {i} 失败"),
        }
    }
    ICONS.with(|i| *i.borrow_mut() = Some(rgba_icons.clone()));

    // 2. 初始图标（光栅化失败时回退到灰图标）。
    let idx = current_index();
    let init_rgba = if rgba_icons[idx].len() == 64 * 64 * 4 {
        rgba_icons[idx].clone()
    } else {
        rgba_icons[0].clone()
    };
    let init_icon = match Icon::from_rgba(init_rgba, 64, 64) {
        Ok(icon) => icon,
        Err(e) => {
            eprintln!("构造初始托盘图标失败: {e}");
            return None;
        }
    };
    Some(init_icon)
}

fn build_copy_menu() -> (Submenu, [MenuItem; 3]) {
    let copy_ps = MenuItemBuilder::new()
        .id(MenuId::new("copy_ps"))
        .text("PowerShell")
        .enabled(true)
        .build();
    let copy_cmd = MenuItemBuilder::new()
        .id(MenuId::new("copy_cmd"))
        .text("CMD")
        .enabled(true)
        .build();
    let copy_bash = MenuItemBuilder::new()
        .id(MenuId::new("copy_bash"))
        .text("Bash")
        .enabled(true)
        .build();
    let items = [copy_ps, copy_cmd, copy_bash];
    let item_refs: Vec<&dyn IsMenuItem> =
        items.iter().map(|item| item as &dyn IsMenuItem).collect();
    let submenu = SubmenuBuilder::new()
        .id(MenuId::new("copy_env"))
        .text("复制环境变量")
        .enabled(true)
        .items(&item_refs)
        .build()
        .expect("构建复制环境变量子菜单失败");
    (submenu, items)
}

fn build_mode_menu() -> (Submenu, [MenuItem; 3]) {
    let mode_rule = MenuItemBuilder::new()
        .id(MenuId::new("mode_rule"))
        .text("规则模式")
        .enabled(true)
        .build();
    let mode_global = MenuItemBuilder::new()
        .id(MenuId::new("mode_global"))
        .text("全局模式")
        .enabled(true)
        .build();
    let mode_direct = MenuItemBuilder::new()
        .id(MenuId::new("mode_direct"))
        .text("直连模式")
        .enabled(true)
        .build();
    let items = [mode_rule, mode_global, mode_direct];
    let item_refs: Vec<&dyn IsMenuItem> =
        items.iter().map(|item| item as &dyn IsMenuItem).collect();
    let submenu = SubmenuBuilder::new()
        .id(MenuId::new("mode"))
        .text("出站模式")
        .enabled(true)
        .items(&item_refs)
        .build()
        .expect("构建出站模式子菜单失败");
    (submenu, items)
}

struct MenuHandles {
    copy_menu: Submenu,
    copy_items: [MenuItem; 3],
    mode_menu: Submenu,
    mode_items: [MenuItem; 3],
    sys: CheckMenuItem,
    tun: CheckMenuItem,
}

fn build_menu(status: config::ProxyStatus) -> (Menu, MenuHandles) {
    let runtime_enabled = core::get_port().is_some();
    let show = MenuItemBuilder::new()
        .id(MenuId::new("show_main"))
        .text("显示主界面")
        .enabled(true)
        .build();

    let sep1 = PredefinedMenuItem::separator();
    let (copy_env, copy_items) = build_copy_menu();
    let (mode, mode_items) = build_mode_menu();

    let sys = CheckMenuItemBuilder::new()
        .id(MenuId::new("system_proxy"))
        .text("系统代理")
        .enabled(runtime_enabled)
        .checked(status.system)
        .build();
    let tun = CheckMenuItemBuilder::new()
        .id(MenuId::new("tun_proxy"))
        .text("TUN 代理")
        .enabled(runtime_enabled)
        .checked(status.tun)
        .build();

    let sep2 = PredefinedMenuItem::separator();

    let quit_item = MenuItemBuilder::new()
        .id(MenuId::new("quit"))
        .text("退出")
        .enabled(true)
        .build();

    let menu = Menu::new();
    let items: Vec<&dyn IsMenuItem> = vec![
        &show, &sep1, &copy_env, &mode, &sys, &tun, &sep2, &quit_item,
    ];
    menu.append_items(&items).expect("追加托盘菜单项失败");
    (
        menu,
        MenuHandles {
            copy_menu: copy_env,
            copy_items,
            mode_menu: mode,
            mode_items,
            sys,
            tun,
        },
    )
}

fn create_tray(menu: Menu, icon: Icon) -> Option<TrayIcon> {
    match TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_icon(icon)
        .with_tooltip("Clash UI")
        .with_menu_on_left_click(true)
        .build()
    {
        Ok(tray) => Some(tray),
        Err(e) => {
            eprintln!("创建托盘失败: {e}");
            None
        }
    }
}

fn cache_handles(
    root: PathBuf,
    window: slint::Weak<MainWindow>,
    tray: TrayIcon,
    handles: MenuHandles,
) {
    let MenuHandles {
        copy_menu,
        copy_items,
        mode_menu,
        mode_items,
        sys,
        tun,
    } = handles;
    TRAY.with(|t| *t.borrow_mut() = Some(tray));
    COPY_MENU.with(|m| *m.borrow_mut() = Some(copy_menu));
    COPY_ITEMS.with(|i| *i.borrow_mut() = Some(copy_items));
    MODE_MENU.with(|m| *m.borrow_mut() = Some(mode_menu));
    MODE_ITEMS.with(|i| *i.borrow_mut() = Some(mode_items));
    SYS_ITEM.with(|s| *s.borrow_mut() = Some(sys));
    TUN_ITEM.with(|t| *t.borrow_mut() = Some(tun));
    ROOT.with(|r| *r.borrow_mut() = Some(root));
    WINDOW.with(|w| *w.borrow_mut() = Some(window));
}

fn register_menu_events() {
    MenuEvent::set_event_handler(Some(|event: MenuEvent| match event.id.as_ref() {
        "show_main" => show_main(),
        "copy_ps" => copy_proxy_env(Terminal::PowerShell),
        "copy_cmd" => copy_proxy_env(Terminal::Cmd),
        "copy_bash" => copy_proxy_env(Terminal::Bash),
        "mode_rule" => set_mode("rule"),
        "mode_global" => set_mode("global"),
        "mode_direct" => set_mode("direct"),
        "system_proxy" => {
            let _ = slint::invoke_from_event_loop(toggle_system_proxy);
        }
        "tun_proxy" => {
            let _ = slint::invoke_from_event_loop(toggle_tun);
        }
        "quit" => quit(),
        _ => {}
    }));
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
