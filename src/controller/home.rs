// 主页数据刷新与轻量状态转换。

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use slint::{ComponentHandle, Timer};
use sysinfo::{Pid, System};

use crate::app::config;
use crate::clash::{api, core};
use crate::constants::RUNTIME_UI_DIR;
use crate::controller::tray;
use crate::{platform, MainWindow};

pub(crate) fn bind_callbacks(window: &MainWindow, root: PathBuf, start: Instant, timer: &Timer) {
    bind_mode_and_proxy(window, start);
    bind_core(window, root.clone(), start);
    bind_online_panel(window, root);
    bind_timer(window, timer, start);
}

fn bind_mode_and_proxy(window: &MainWindow, start: Instant) {
    let weak = window.as_weak();
    window.global::<crate::HomeModel>().on_change_mode({
        let weak = weak.clone();
        move |index| {
            let mode = match index {
                0 => "rule",
                1 => "global",
                2 => "direct",
                _ => return,
            };
            tray::set_mode(mode);
            if let Some(window) = weak.upgrade() {
                refresh(&window, &start);
            }
        }
    });
    window.global::<crate::HomeModel>().on_copy_env(|index| {
        let terminal = match index {
            0 => tray::Terminal::PowerShell,
            1 => tray::Terminal::Cmd,
            2 => tray::Terminal::Bash,
            _ => return,
        };
        tray::copy_proxy_env(terminal);
    });
    window.global::<crate::HomeModel>().on_toggle_system_proxy({
        let weak = weak.clone();
        move || {
            tray::toggle_system_proxy();
            if let Some(window) = weak.upgrade() {
                refresh(&window, &start);
            }
        }
    });
    window.global::<crate::HomeModel>().on_toggle_tun({
        let weak = weak.clone();
        move || {
            tray::toggle_tun();
            if let Some(window) = weak.upgrade() {
                refresh(&window, &start);
            }
        }
    });
}

fn bind_core(window: &MainWindow, root: PathBuf, start: Instant) {
    let weak = window.as_weak();
    window.global::<crate::HomeModel>().on_restart_core({
        let weak = weak.clone();
        let root = root.clone();
        move || {
            if let Err(error) = core::restart_core(&root) {
                eprintln!("重启 clash 核心失败: {error}");
            }
            if let Some(window) = weak.upgrade() {
                refresh(&window, &start);
            }
        }
    });
    window.global::<crate::HomeModel>().on_update_core({
        let weak = weak.clone();
        move || {
            if let Err(error) = core::update_core(&root) {
                eprintln!("更新 clash 核心失败: {error}");
            }
            if let Some(window) = weak.upgrade() {
                refresh(&window, &start);
            }
        }
    });
}

fn bind_online_panel(window: &MainWindow, root: PathBuf) {
    window
        .global::<crate::HomeModel>()
        .on_open_online_panel(move || {
            let task_root = root.clone();
            let task = std::thread::Builder::new()
                .name("online-panel-download".to_string())
                .spawn(move || match prepare_online_panel(&task_root) {
                    Ok(url) => {
                        if let Err(error) = slint::invoke_from_event_loop(move || {
                            if let Err(error) = platform::open_url(&url) {
                                eprintln!("打开在线面板失败：{error}");
                            }
                        }) {
                            eprintln!("投递在线面板打开任务失败：{error}");
                        }
                    }
                    Err(error) => eprintln!("准备在线面板失败：{error}"),
                });
            if let Err(error) = task {
                eprintln!("启动在线面板任务失败：{error}");
            }
        });
}

fn bind_timer(window: &MainWindow, timer: &Timer, start: Instant) {
    let weak = window.as_weak();
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_secs(2),
        move || {
            if let Some(window) = weak.upgrade() {
                if window.global::<crate::AppState>().get_current_page() == 0 {
                    refresh(&window, &start);
                } else {
                    refresh_runtime_state(&window);
                }
            }
        },
    );
}

/// 字节格式化为 MB。
fn fmt_mb(bytes: u64) -> String {
    format!("{} MB", bytes / 1024 / 1024)
}

/// 运行时长格式化为 H:MM:SS。
fn format_uptime(duration: Duration) -> String {
    let total = duration.as_secs();
    let hours = total / 3600;
    let minutes = (total % 3600) / 60;
    let seconds = total % 60;
    format!("{hours}:{minutes:02}:{seconds:02}")
}

/// 当前进程常驻内存（字节）。
fn client_rss() -> u64 {
    let system = System::new_all();
    let pid = Pid::from_u32(std::process::id());
    system
        .process(pid)
        .map(|process| process.memory())
        .unwrap_or(0)
}

/// 刷新主页和托盘的核心运行态，不请求 Clash API。
pub fn refresh_runtime_state(main_window: &MainWindow) {
    main_window
        .global::<crate::HomeModel>()
        .set_core_running(core::get_port().is_some());
    tray::refresh_runtime_state();
}

/// 格式化主页代理地址；无可用端口时显示无可用值。
fn proxy_address(endpoint: &tray::ProxyEndpoint) -> String {
    tray::proxy_address(endpoint).unwrap_or_else(|| "—".to_string())
}

/// 刷新主页展示数据。
pub fn refresh(main_window: &MainWindow, start: &Instant) {
    let home = main_window.global::<crate::HomeModel>();
    refresh_runtime_state(main_window);
    home.set_platform_name(platform::platform_name().into());

    let configs = api::get_configs();
    let endpoint = configs
        .as_ref()
        .ok()
        .and_then(|configs| tray::proxy_endpoint_from_configs(configs).ok());
    let proxy_address = endpoint
        .as_ref()
        .map(proxy_address)
        .unwrap_or_else(|| "—".to_string());
    home.set_proxy_address(proxy_address.into());

    if let Ok(configs) = configs {
        let mode = match configs.mode.as_str() {
            "rule" => "规则模式",
            "global" => "全局模式",
            "direct" => "直连模式",
            _ => "—",
        };
        home.set_outbound_mode(mode.into());
    }

    let proxy_status = config::get().proxy_status;
    home.set_system_proxy(proxy_status.system);
    home.set_tun_proxy(proxy_status.tun);
    home.set_core_running(core::get_port().is_some());
    home.set_core_version(
        api::get_version()
            .map(|version| version.version)
            .unwrap_or_else(|_| "—".to_string())
            .into(),
    );
    home.set_client_mem(fmt_mb(client_rss()).into());
    home.set_core_mem(
        api::latest_memory()
            .map(|snapshot| fmt_mb(snapshot.inuse))
            .unwrap_or_else(|| "—".to_string())
            .into(),
    );
    home.set_uptime(format_uptime(start.elapsed()).into());
    home.set_client_version(env!("CARGO_PKG_VERSION").into());

    let panel_url = core::get_controller_snapshot()
        .map(|snapshot| zashboard_url(&snapshot))
        .unwrap_or_default();
    home.set_zashboard_url(panel_url.into());
}

/// 根据当前核心控制会话构造完整的 zashboard URL。
fn zashboard_url(snapshot: &core::ControllerSnapshot) -> String {
    format!(
        "http://127.0.0.1:{}/ui/#/setup?hostname=127.0.0.1&port={}&secret={}",
        snapshot.port, snapshot.port, snapshot.secret
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PanelDirectoryState {
    Empty,
    Ready,
}

/// 判断在线面板目录是否为空；目录读取失败时保留文件系统上下文。
fn panel_directory_state(path: &Path) -> Result<PanelDirectoryState, String> {
    let mut entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(PanelDirectoryState::Empty)
        }
        Err(error) => return Err(format!("读取在线面板目录 {} 失败：{error}", path.display())),
    };

    match entries.next() {
        None => Ok(PanelDirectoryState::Empty),
        Some(Ok(_)) => Ok(PanelDirectoryState::Ready),
        Some(Err(error)) => Err(format!("读取在线面板目录 {} 失败：{error}", path.display())),
    }
}

/// 编排在线面板准备流程，允许测试注入更新闭包和核心会话快照。
fn prepare_online_panel_with<S, U, E>(
    root: &Path,
    initial_snapshot: Option<core::ControllerSnapshot>,
    current_snapshot: S,
    upgrade: U,
) -> Result<String, String>
where
    S: FnOnce() -> Option<core::ControllerSnapshot>,
    U: FnOnce() -> Result<(), E>,
    E: std::fmt::Display,
{
    let _initial_snapshot = initial_snapshot.ok_or_else(|| "核心未运行".to_string())?;
    let panel_dir = root.join(RUNTIME_UI_DIR);
    if panel_directory_state(&panel_dir)? == PanelDirectoryState::Empty {
        upgrade().map_err(|error| format!("下载在线面板失败：{error}"))?;
        if panel_directory_state(&panel_dir)? == PanelDirectoryState::Empty {
            return Err("在线面板下载完成但目录仍为空".to_string());
        }
    }

    let snapshot = current_snapshot().ok_or_else(|| "核心未运行".to_string())?;
    Ok(zashboard_url(&snapshot))
}

/// 准备在线面板并返回当前核心会话对应的完整 URL。
pub fn prepare_online_panel(root: &Path) -> Result<String, String> {
    prepare_online_panel_with(
        root,
        core::get_controller_snapshot(),
        core::get_controller_snapshot,
        || api::upgrade_ui().map_err(|error| error.to_string()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::path::PathBuf;

    fn tmp_root(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("clash_ui_home_test_{name}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("创建主页测试目录失败");
        root
    }

    #[test]
    fn homepage_proxy_address_uses_api_bind_address() {
        let endpoint = tray::ProxyEndpoint {
            host: "192.168.1.20".to_string(),
            ports: tray::ProxyPorts {
                mixed: None,
                http: Some(7890),
                socks: Some(7891),
            },
        };
        assert_eq!(
            tray::proxy_address(&endpoint),
            Some("socks5://192.168.1.20:7891".to_string())
        );
    }

    #[test]
    fn zashboard_url_uses_controller_port_and_preserves_query_order() {
        let snapshot = core::ControllerSnapshot {
            port: 20001,
            secret: "s3cr3t".to_string(),
        };
        assert_eq!(
            zashboard_url(&snapshot),
            "http://127.0.0.1:20001/ui/#/setup?hostname=127.0.0.1&port=20001&secret=s3cr3t"
        );
    }

    #[test]
    fn covers_panel_directory_states() {
        let root = tmp_root("directory_state");
        let missing = root.join("missing");
        assert_eq!(
            panel_directory_state(&missing),
            Ok(PanelDirectoryState::Empty)
        );

        let empty = root.join("empty");
        fs::create_dir_all(&empty).unwrap();
        assert_eq!(
            panel_directory_state(&empty),
            Ok(PanelDirectoryState::Empty)
        );

        let ready = root.join("ready");
        fs::create_dir_all(&ready).unwrap();
        fs::write(ready.join("index.html"), "ok").unwrap();
        assert_eq!(
            panel_directory_state(&ready),
            Ok(PanelDirectoryState::Ready)
        );

        let file = root.join("file");
        fs::write(&file, "not a directory").unwrap();
        let error = panel_directory_state(&file).unwrap_err();
        assert!(error.contains("读取在线面板目录"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn updates_missing_panel_once_and_rebuilds_current_session_url() {
        let root = tmp_root("upgrade");
        let initial = core::ControllerSnapshot {
            port: 20001,
            secret: "old".to_string(),
        };
        let current = core::ControllerSnapshot {
            port: 20002,
            secret: "new".to_string(),
        };
        let calls = Cell::new(0);
        let url = prepare_online_panel_with(
            &root,
            Some(initial),
            || Some(current.clone()),
            || {
                calls.set(calls.get() + 1);
                fs::create_dir_all(root.join(RUNTIME_UI_DIR)).unwrap();
                fs::write(root.join(RUNTIME_UI_DIR).join("index.html"), "ok").unwrap();
                Ok::<(), &str>(())
            },
        )
        .unwrap();

        assert_eq!(calls.get(), 1);
        assert_eq!(
            url,
            "http://127.0.0.1:20002/ui/#/setup?hostname=127.0.0.1&port=20002&secret=new"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn does_not_update_existing_panel() {
        let root = tmp_root("ready");
        let panel_dir = root.join(RUNTIME_UI_DIR);
        fs::create_dir_all(&panel_dir).unwrap();
        fs::write(panel_dir.join("index.html"), "ok").unwrap();
        let snapshot = core::ControllerSnapshot {
            port: 20003,
            secret: "ready".to_string(),
        };
        let calls = Cell::new(0);
        let url = prepare_online_panel_with(
            &root,
            Some(snapshot.clone()),
            || Some(snapshot.clone()),
            || {
                calls.set(calls.get() + 1);
                Ok::<(), &str>(())
            },
        )
        .unwrap();

        assert_eq!(calls.get(), 0);
        assert!(url.contains("port=20003&secret=ready"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn blocks_open_when_update_fails_or_directory_empty() {
        let root = tmp_root("failure");
        let snapshot = core::ControllerSnapshot {
            port: 20004,
            secret: "failure".to_string(),
        };
        let error = prepare_online_panel_with(
            &root,
            Some(snapshot.clone()),
            || Some(snapshot.clone()),
            || Err::<(), _>("网络失败"),
        )
        .unwrap_err();
        assert!(error.contains("下载在线面板失败：网络失败"));

        let empty = root.join("empty");
        fs::create_dir_all(&empty).unwrap();
        let error = prepare_online_panel_with(
            &empty,
            Some(snapshot.clone()),
            || Some(snapshot),
            || Ok::<(), &str>(()),
        )
        .unwrap_err();
        assert!(error.contains("目录仍为空"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn does_not_update_when_core_is_not_running() {
        let root = tmp_root("no_session");
        let calls = Cell::new(0);
        let error = prepare_online_panel_with(
            &root,
            None,
            || None,
            || {
                calls.set(calls.get() + 1);
                Ok::<(), &str>(())
            },
        )
        .unwrap_err();
        assert_eq!(error, "核心未运行");
        assert_eq!(calls.get(), 0);
        let _ = fs::remove_dir_all(root);
    }
}
