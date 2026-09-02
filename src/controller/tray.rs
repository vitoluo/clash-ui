use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use slint::ComponentHandle;

use crate::app::config;
use crate::clash::{api, core};
use crate::controller::common::business::{self, Terminal};
use crate::{ClashTray, MainWindow};

#[derive(Debug, Default)]
struct TrayCoreDataState {
    core_state: crate::event::CoreState,
    loaded_generation: Option<u64>,
    loading_generation: Option<u64>,
    request_token: u64,
}

static TRAY_CORE_DATA_STATE: OnceLock<Mutex<TrayCoreDataState>> = OnceLock::new();

fn tray_core_data_state() -> &'static Mutex<TrayCoreDataState> {
    TRAY_CORE_DATA_STATE.get_or_init(|| Mutex::new(TrayCoreDataState::default()))
}

fn next_request_token(state: &mut TrayCoreDataState) -> u64 {
    state.request_token = state.request_token.wrapping_add(1).max(1);
    state.request_token
}

thread_local! {
    static WINDOW: RefCell<Option<slint::Weak<MainWindow>>> = const { RefCell::new(None) };
}

enum TrayUiUpdate {
    OutboundMode(String),
    SystemProxy(bool),
    TunProxy(bool),
    CoreState(crate::event::CoreState),
}

fn apply_ui_update(tray: &ClashTray, update: TrayUiUpdate) {
    match update {
        TrayUiUpdate::OutboundMode(mode) => {
            tray.set_rule_mode_checked(mode == "rule");
            tray.set_global_mode_checked(mode == "global");
            tray.set_direct_mode_checked(mode == "direct");
        }
        TrayUiUpdate::SystemProxy(enabled) => {
            tray.set_system_proxy(enabled);
        }
        TrayUiUpdate::TunProxy(enabled) => {
            tray.set_tun_proxy(enabled);
        }
        TrayUiUpdate::CoreState(state) => {
            refresh(tray.as_weak(), state);
        }
    }
}

fn refresh(weak: slint::Weak<ClashTray>, core_state: crate::event::CoreState) {
    let request = {
        let mut state = tray_core_data_state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.core_state != core_state {
            state.core_state = core_state;
            state.loaded_generation = None;
            state.loading_generation = None;
        }
        if !core_state.running {
            let token = next_request_token(&mut state);
            state.loaded_generation = None;
            state.loading_generation = None;
            Some(token)
        } else if state.loaded_generation == Some(core_state.generation)
            || state.loading_generation == Some(core_state.generation)
        {
            None
        } else {
            let token = next_request_token(&mut state);
            state.loading_generation = Some(core_state.generation);
            Some(token)
        }
    };
    let Some(token) = request else {
        return;
    };
    if !core_state.running {
        if let Err(error) = slint::invoke_from_event_loop(move || {
            if let Some(tray) = weak.upgrade() {
                tray.set_core_running(false);
            }
        }) {
            crate::log::error(format_args!("投递托盘停止状态失败：{error}"));
        }
        return;
    }

    crate::runtime::spawn_task(async move {
        let (outbound_mode, success) = match api::get_configs().await {
            Ok(configs) => (configs.mode, true),
            Err(error) => {
                crate::log::error(format_args!("加载核心配置失败：{error}"));
                (String::new(), false)
            }
        };

        let current = {
            let mut state = tray_core_data_state()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if state.request_token != token
                || state.core_state != core_state
                || state.loading_generation != Some(core_state.generation)
                || *crate::event::subscribe_core_state().borrow() != core_state
                || !core_state.running
            {
                false
            } else {
                state.loading_generation = None;
                state.loaded_generation = success.then_some(core_state.generation);
                true
            }
        };
        if !current {
            return;
        }

        if let Err(e) = slint::invoke_from_event_loop(move || {
            let current = tray_core_data_state()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if current.request_token != token
                || current.core_state != core_state
                || *crate::event::subscribe_core_state().borrow() != core_state
                || !core_state.running
            {
                return;
            }
            drop(current);
            if let Some(tray) = weak.upgrade() {
                tray.set_core_running(true);
                let proxy_status = config::proxy_status();
                tray.set_system_proxy(proxy_status.system);
                tray.set_tun_proxy(proxy_status.tun);

                let update = TrayUiUpdate::OutboundMode(outbound_mode);
                apply_ui_update(&tray, update);
            }
        }) {
            crate::log::error(format_args!("投递托盘刷新任务失败：{e}"));
        }
    });
}

fn listen_ui_updates(tray: slint::Weak<ClashTray>) {
    let mut outbound_mode = crate::event::subscribe_outbound_mode();
    let mut system_proxy = crate::event::subscribe_system_proxy();
    let mut tun_proxy = crate::event::subscribe_tun_proxy();
    let mut core_state = crate::event::subscribe_core_state();
    // watch 接收器订阅后默认已读当前版本；控制器可能晚于发布，因此主动处理当前值。
    core_state.mark_changed();
    crate::runtime::spawn_task(async move {
        loop {
            let update = tokio::select! {
                result = outbound_mode.changed() => {
                    if result.is_err() {
                        return;
                    }
                    TrayUiUpdate::OutboundMode(outbound_mode.borrow_and_update().clone())
                }
                result = system_proxy.changed() => {
                    if result.is_err() {
                        return;
                    }
                    TrayUiUpdate::SystemProxy(*system_proxy.borrow_and_update())
                }
                result = tun_proxy.changed() => {
                    if result.is_err() {
                        return;
                    }
                    TrayUiUpdate::TunProxy(*tun_proxy.borrow_and_update())
                }
                result = core_state.changed() => {
                    if result.is_err() {
                        return;
                    }
                    TrayUiUpdate::CoreState(*core_state.borrow_and_update())
                }
            };
            let weak = tray.clone();
            if let Err(error) = slint::invoke_from_event_loop(move || {
                if let Some(tray) = weak.upgrade() {
                    apply_ui_update(&tray, update);
                }
            }) {
                crate::log::error(format_args!("投递托盘状态更新失败：{error}"));
                return;
            }
        }
    });
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

/// 退出：停止核心并退出事件循环。
pub fn quit() {
    if let Err(error) = core::stop_core() {
        crate::log::error(format_args!("停止 clash 核心失败：{error}"));
    }
    let _ = slint::quit_event_loop();
}

/// 初始化 Slint 系统托盘。托盘不可用时仍保留主界面状态同步能力。
pub fn init(root: PathBuf, window: slint::Weak<MainWindow>, tray: Option<&ClashTray>) {
    WINDOW.with(|value| *value.borrow_mut() = Some(window));

    let Some(tray) = tray else {
        return;
    };

    listen_ui_updates(tray.as_weak());
    tray.on_show_main(show_main);
    tray.on_copy_env(|index| match index {
        0 => business::copy_proxy_env(Terminal::PowerShell),
        1 => business::copy_proxy_env(Terminal::Cmd),
        2 => business::copy_proxy_env(Terminal::Bash),
        _ => crate::log::error(format_args!("未知的终端类型索引：{index}")),
    });
    tray.on_change_mode(|mode| business::set_mode(mode.as_str()));
    tray.on_toggle_system_proxy(business::toggle_system_proxy);
    tray.on_toggle_tun(move || business::toggle_tun(root.clone()));
    tray.on_quit(quit);
}
