// 设置页控制器：负责表单持久化、平台动作和核心配置延迟应用。

use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use slint::{ComponentHandle, ModelRc, VecModel, Weak};

use crate::app::config::{self, LogLevel, ThemeMode, TunStack};
use crate::clash::core;
use crate::controller::home;
use crate::platform;
use crate::{ListEntry, MainWindow, SettingsModel, UwpRow};

#[derive(Debug)]
pub struct SettingsViewState {
    root: PathBuf,
    core_dirty: bool,
    applying_core: bool,
    operation_busy: bool,
    uwp_apps: Vec<platform::UwpApp>,
    uwp_draft: Vec<String>,
    uwp_query: String,
    dialog_list_items: Vec<String>,
    next_token: u64,
    uwp_token: u64,
}

pub type SharedSettingsState = Arc<Mutex<SettingsViewState>>;

pub(crate) fn bind_callbacks(window: &MainWindow, state: SharedSettingsState) {
    bind_core_callbacks(window, state.clone());
    bind_tun_callbacks(window, state.clone());
    bind_list_callbacks(window, state.clone());
    bind_uwp_callbacks(window, state);
}

fn bind_core_callbacks(window: &MainWindow, state: SharedSettingsState) {
    let weak = window.as_weak();
    window.global::<SettingsModel>().on_toggle_auto_start({
        let weak = weak.clone();
        let state = state.clone();
        move |enabled| toggle_auto_start(weak.clone(), state.clone(), enabled)
    });
    window
        .global::<SettingsModel>()
        .on_toggle_silent_start(|enabled| {
            toggle_silent_start(enabled);
        });
    window.global::<SettingsModel>().on_change_log_level({
        let weak = weak.clone();
        let state = state.clone();
        move |index| change_log_level(weak.clone(), state.clone(), index)
    });
    window.global::<SettingsModel>().on_toggle_ipv6({
        let weak = weak.clone();
        let state = state.clone();
        move |enabled| toggle_ipv6(weak.clone(), state.clone(), enabled)
    });
    window.global::<SettingsModel>().on_toggle_allow_lan({
        let weak = weak.clone();
        let state = state.clone();
        move |enabled| toggle_allow_lan(weak.clone(), state.clone(), enabled)
    });
    window.global::<SettingsModel>().on_submit_bind_address({
        let weak = weak.clone();
        let state = state.clone();
        move |value| submit_bind_address(weak.clone(), state.clone(), value.into())
    });
    window.global::<SettingsModel>().on_open_ports({
        let weak = weak.clone();
        move || open_ports(weak.clone())
    });
    window.global::<SettingsModel>().on_submit_ports({
        let weak = weak.clone();
        let state = state.clone();
        move |mixed, http, socks| {
            submit_ports(
                weak.clone(),
                state.clone(),
                mixed.into(),
                http.into(),
                socks.into(),
            )
        }
    });
}

fn bind_tun_callbacks(window: &MainWindow, state: SharedSettingsState) {
    let weak = window.as_weak();
    window.global::<SettingsModel>().on_change_tun_stack({
        let weak = weak.clone();
        let state = state.clone();
        move |index| change_tun_stack(weak.clone(), state.clone(), index)
    });
    window.global::<SettingsModel>().on_submit_tun_device({
        let weak = weak.clone();
        let state = state.clone();
        move |value| submit_tun_device(weak.clone(), state.clone(), value.into())
    });
    window.global::<SettingsModel>().on_submit_tun_mtu({
        let weak = weak.clone();
        let state = state.clone();
        move |value| submit_tun_mtu(weak.clone(), state.clone(), value.into())
    });
    window
        .global::<SettingsModel>()
        .on_toggle_tun_strict_route({
            let weak = weak.clone();
            let state = state.clone();
            move |enabled| toggle_tun_strict_route(weak.clone(), state.clone(), enabled)
        });
    window
        .global::<SettingsModel>()
        .on_toggle_tun_auto_detect_interface({
            let weak = weak.clone();
            let state = state.clone();
            move |enabled| toggle_tun_auto_detect_interface(weak.clone(), state.clone(), enabled)
        });
    window.global::<SettingsModel>().on_toggle_tun_auto_route({
        let weak = weak.clone();
        let state = state.clone();
        move |enabled| toggle_tun_auto_route(weak.clone(), state.clone(), enabled)
    });
}

fn bind_list_callbacks(window: &MainWindow, state: SharedSettingsState) {
    let weak = window.as_weak();
    window.global::<SettingsModel>().on_open_list({
        let weak = weak.clone();
        let state = state.clone();
        move |kind| open_list(weak.clone(), state.clone(), kind)
    });
    window.global::<SettingsModel>().on_edit_list_item({
        let weak = weak.clone();
        let state = state.clone();
        move |index, value| edit_list_item(weak.clone(), state.clone(), index, value.into())
    });
    window.global::<SettingsModel>().on_add_list_item({
        let weak = weak.clone();
        let state = state.clone();
        move |value| add_list_item(weak.clone(), state.clone(), value.into())
    });
    window.global::<SettingsModel>().on_remove_list_item({
        let weak = weak.clone();
        let state = state.clone();
        move |index| remove_list_item(weak.clone(), state.clone(), index)
    });
    window.global::<SettingsModel>().on_save_list({
        let weak = weak.clone();
        let state = state.clone();
        move |kind, value| save_list(weak.clone(), state.clone(), kind, value.into())
    });
    window.global::<SettingsModel>().on_reset_list({
        let weak = weak.clone();
        move |kind| reset_list(weak.clone(), state.clone(), kind)
    });
}

fn bind_uwp_callbacks(window: &MainWindow, state: SharedSettingsState) {
    let weak = window.as_weak();
    window.global::<SettingsModel>().on_open_uwp({
        let weak = weak.clone();
        let state = state.clone();
        move || open_uwp(weak.clone(), state.clone())
    });
    window.global::<SettingsModel>().on_search_uwp({
        let weak = weak.clone();
        let state = state.clone();
        move |value| search_uwp(state.clone(), value.into(), weak.clone())
    });
    window.global::<SettingsModel>().on_toggle_uwp({
        let weak = weak.clone();
        let state = state.clone();
        move |package, enabled| toggle_uwp(weak.clone(), state.clone(), package.into(), enabled)
    });
    window.global::<SettingsModel>().on_save_uwp({
        let weak = weak.clone();
        let state = state.clone();
        move || save_uwp(weak.clone(), state.clone())
    });
    window.global::<SettingsModel>().on_close_dialog({
        let weak = weak.clone();
        let state = state.clone();
        move || close_dialog(weak.clone(), state.clone())
    });
    window.global::<SettingsModel>().on_apply_core({
        let weak = weak.clone();
        move || apply_core(weak.clone(), state.clone())
    });
}

fn lock_state(state: &SharedSettingsState) -> MutexGuard<'_, SettingsViewState> {
    state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub fn new_state(root: PathBuf) -> SharedSettingsState {
    Arc::new(Mutex::new(SettingsViewState {
        root,
        core_dirty: false,
        applying_core: false,
        operation_busy: false,
        uwp_apps: Vec::new(),
        uwp_draft: Vec::new(),
        uwp_query: String::new(),
        dialog_list_items: Vec::new(),
        next_token: 0,
        uwp_token: 0,
    }))
}

fn next_token(state: &mut SettingsViewState) -> u64 {
    state.next_token = state.next_token.wrapping_add(1).max(1);
    state.next_token
}

fn invoke_ui<F>(callback: F)
where
    F: FnOnce() + Send + 'static,
{
    if let Err(error) = slint::invoke_from_event_loop(callback) {
        crate::log::error(format_args!("设置页 UI 回调失败：{error}"));
    }
}

fn theme_index(mode: ThemeMode) -> i32 {
    match mode {
        ThemeMode::System => 0,
        ThemeMode::Light => 1,
        ThemeMode::Dark => 2,
    }
}

fn log_level_index(level: LogLevel) -> i32 {
    match level {
        LogLevel::Silent => 0,
        LogLevel::Error => 1,
        LogLevel::Warning => 2,
        LogLevel::Info => 3,
        LogLevel::Debug => 4,
    }
}

fn log_level_from_index(index: i32) -> Option<LogLevel> {
    Some(match index {
        0 => LogLevel::Silent,
        1 => LogLevel::Error,
        2 => LogLevel::Warning,
        3 => LogLevel::Info,
        4 => LogLevel::Debug,
        _ => return None,
    })
}

fn tun_stack_index(stack: TunStack) -> i32 {
    match stack {
        TunStack::System => 0,
        TunStack::Gvisor => 1,
        TunStack::Mixed => 2,
    }
}

fn tun_stack_from_index(index: i32) -> Option<TunStack> {
    Some(match index {
        0 => TunStack::System,
        1 => TunStack::Gvisor,
        2 => TunStack::Mixed,
        _ => return None,
    })
}

fn list_summary(values: &[String]) -> String {
    if values.is_empty() {
        "暂无".to_string()
    } else {
        values.join("、")
    }
}

fn ports_summary(clash: &config::ClashSettings) -> String {
    match (clash.mixed_port, clash.port, clash.socks_port) {
        (Some(mixed), _, _) => format!("混合端口 {mixed}"),
        (None, Some(http), Some(socks)) => format!("HTTP {http} · SOCKS {socks}"),
        (None, Some(http), None) => format!("HTTP {http}"),
        (None, None, Some(socks)) => format!("SOCKS {socks}"),
        (None, None, None) => "未配置".to_string(),
    }
}

fn sync_ports_summary(window: &MainWindow) {
    let clash = config::get().settings.clash;
    window
        .global::<SettingsModel>()
        .set_ports_summary(ports_summary(&clash).into());
}

fn sync_tun_list_summary(window: &MainWindow, kind: i32) {
    let tun = config::get().settings.clash.tun;
    let model = window.global::<SettingsModel>();
    match kind {
        0 => model.set_tun_exclude_address_summary(list_summary(&tun.route_exclude_address).into()),
        1 => model.set_tun_exclude_interface_summary(list_summary(&tun.exclude_interface).into()),
        _ => {}
    }
}

fn set_toast(window: &MainWindow, message: &str, variant: i32) {
    let model = window.global::<SettingsModel>();
    model.set_toast_message(message.to_string().into());
    model.set_toast_variant(variant);
    model.set_toast_visible(true);
}

fn enabled_uwp_packages(apps: &[platform::UwpApp]) -> Vec<String> {
    apps.iter()
        .filter(|app| app.enabled)
        .map(|app| app.package_family_name.clone())
        .collect()
}

fn render_uwp(window: &MainWindow, state: &SettingsViewState) {
    let query = state.uwp_query.to_ascii_lowercase();
    let rows = state
        .uwp_apps
        .iter()
        .filter(|app| {
            query.is_empty()
                || app.name.to_ascii_lowercase().contains(&query)
                || app
                    .package_family_name
                    .to_ascii_lowercase()
                    .contains(&query)
        })
        .map(|app| UwpRow {
            name: app.name.clone().into(),
            package_family_name: app.package_family_name.clone().into(),
            enabled: state.uwp_draft.contains(&app.package_family_name),
        })
        .collect::<Vec<_>>();
    window
        .global::<SettingsModel>()
        .set_uwp_apps(ModelRc::new(VecModel::from(rows)));
}

fn render_list_editor(window: &MainWindow, state: &SettingsViewState) {
    let rows = state
        .dialog_list_items
        .iter()
        .map(|value| ListEntry {
            value: value.clone().into(),
        })
        .collect::<Vec<_>>();
    let model = window.global::<SettingsModel>();
    model.set_dialog_list_items(ModelRc::new(VecModel::from(rows)));
    model.set_dialog_list_draft(state.dialog_list_items.join("\n").into());
}

fn set_model(window: &MainWindow, state: &SettingsViewState, cfg: &config::AppConfig) {
    let model = window.global::<SettingsModel>();
    let clash = &cfg.settings.clash;
    let tun = &clash.tun;
    model.set_auto_start(cfg.settings.app.auto_start);
    model.set_silent_start(cfg.settings.app.silent_start);
    model.set_theme_index(theme_index(cfg.settings.app.theme));
    model.set_log_level_index(log_level_index(clash.log_level));
    model.set_ipv6(clash.ipv6);
    model.set_allow_lan(clash.allow_lan);
    model.set_bind_address(clash.bind_address.clone().into());
    model.set_ports_summary(ports_summary(clash).into());
    model.set_tun_stack_index(tun_stack_index(tun.stack));
    model.set_tun_device(tun.device.clone().into());
    model.set_tun_mtu(tun.mtu.to_string().into());
    model.set_tun_strict_route(tun.strict_route);
    model.set_tun_auto_detect_interface(tun.auto_detect_interface);
    model.set_tun_auto_route(tun.auto_route);
    model.set_tun_exclude_address_summary(list_summary(&tun.route_exclude_address).into());
    model.set_tun_exclude_interface_summary(list_summary(&tun.exclude_interface).into());
    model.set_uwp_supported(platform::supports_uwp());
    model.set_bypass_list_summary(list_summary(&cfg.settings.proxy.bypass_list).into());
    model.set_core_dirty(state.core_dirty);
    model.set_applying_core(state.applying_core);
    model.set_operation_busy(state.operation_busy);
    render_uwp(window, state);
}

/// 进入设置页时刷新展示值。
pub fn refresh(window: &MainWindow, state: &SharedSettingsState) {
    let cfg = config::get();
    let should_load_uwp = platform::supports_uwp() && lock_state(state).uwp_apps.is_empty();
    {
        let mut view = lock_state(state);
        if !view.operation_busy {
            view.uwp_draft = enabled_uwp_packages(&view.uwp_apps);
        }
        set_model(window, &view, &cfg);
    }
    if should_load_uwp {
        load_uwp_async(window.as_weak(), state.clone());
    }
}

fn core_change_allowed(state: &SharedSettingsState) -> bool {
    !lock_state(state).applying_core
}

fn mark_core_dirty(weak: &Weak<MainWindow>, state: &SharedSettingsState) {
    let mut view = lock_state(state);
    view.core_dirty = true;
    if let Some(window) = weak.upgrade() {
        let model = window.global::<SettingsModel>();
        model.set_core_dirty(true);
    }
}

fn update_core<F>(weak: Weak<MainWindow>, state: SharedSettingsState, update: F)
where
    F: FnOnce(&mut config::ClashSettings),
{
    if !core_change_allowed(&state) {
        return;
    }
    config::update(|cfg| update(&mut cfg.settings.clash));
    mark_core_dirty(&weak, &state);
}

/// 平台自启动作成功后才持久化开关状态。
pub fn toggle_auto_start(weak: Weak<MainWindow>, state: SharedSettingsState, enabled: bool) {
    let previous = config::get().settings.app.auto_start;
    let token = {
        let mut view = lock_state(&state);
        if view.operation_busy {
            return;
        }
        view.operation_busy = true;
        next_token(&mut view)
    };
    if let Some(window) = weak.upgrade() {
        window.global::<SettingsModel>().set_operation_busy(true);
    }
    std::thread::spawn(move || {
        let result = platform::set_auto_start(enabled);
        invoke_ui(move || {
            let Some(window) = weak.upgrade() else {
                return;
            };
            let mut view = lock_state(&state);
            if view.next_token != token {
                return;
            }
            view.operation_busy = false;
            window.global::<SettingsModel>().set_operation_busy(false);
            match result {
                Ok(()) => {
                    config::update(|cfg| cfg.settings.app.auto_start = enabled);
                    window.global::<SettingsModel>().set_auto_start(enabled);
                    set_toast(&window, "开机自启设置已更新", 1);
                }
                Err(error) => {
                    window.global::<SettingsModel>().set_auto_start(previous);
                    set_toast(&window, &format!("开机自启设置失败：{error}"), 2);
                }
            }
        });
    });
}

/// 立即保存静默启动开关，不改变平台自启动设置。
pub fn toggle_silent_start(enabled: bool) {
    if config::get().settings.app.silent_start == enabled {
        return;
    }
    config::update(|cfg| cfg.settings.app.silent_start = enabled);
}

pub fn change_log_level(weak: Weak<MainWindow>, state: SharedSettingsState, index: i32) {
    let Some(level) = log_level_from_index(index) else {
        return;
    };
    if config::get().settings.clash.log_level == level {
        return;
    }
    update_core(weak, state, move |clash| clash.log_level = level);
}

pub fn toggle_ipv6(weak: Weak<MainWindow>, state: SharedSettingsState, enabled: bool) {
    if config::get().settings.clash.ipv6 == enabled {
        return;
    }
    update_core(weak, state, move |clash| clash.ipv6 = enabled);
}

pub fn toggle_allow_lan(weak: Weak<MainWindow>, state: SharedSettingsState, enabled: bool) {
    if config::get().settings.clash.allow_lan == enabled {
        return;
    }
    update_core(weak, state, move |clash| clash.allow_lan = enabled);
}

pub fn submit_bind_address(weak: Weak<MainWindow>, state: SharedSettingsState, value: String) {
    let value = value.trim().to_string();
    if value.is_empty() {
        if let Some(window) = weak.upgrade() {
            set_toast(&window, "监听地址不能为空", 2);
            refresh(&window, &state);
        }
        return;
    }
    if config::get().settings.clash.bind_address == value {
        return;
    }
    update_core(weak, state, move |clash| clash.bind_address = value);
}

pub fn open_ports(weak: Weak<MainWindow>) {
    let Some(window) = weak.upgrade() else {
        return;
    };
    let clash = config::get().settings.clash;
    let model = window.global::<SettingsModel>();
    model.set_port_mixed_draft(optional_port_text(clash.mixed_port).into());
    model.set_port_http_draft(optional_port_text(clash.port).into());
    model.set_port_socks_draft(optional_port_text(clash.socks_port).into());
    model.set_dialog_kind(1);
}

fn optional_port_text(value: Option<u16>) -> String {
    value.map(|value| value.to_string()).unwrap_or_default()
}

fn parse_optional_port(value: &str) -> Result<Option<u16>, String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    let number = value
        .parse::<u32>()
        .map_err(|_| "端口必须是 1 到 65535 的整数".to_string())?;
    if !(1..=u16::MAX as u32).contains(&number) {
        return Err("端口必须是 1 到 65535 的整数".to_string());
    }
    Ok(Some(number as u16))
}

pub fn submit_ports(
    weak: Weak<MainWindow>,
    state: SharedSettingsState,
    mixed: String,
    http: String,
    socks: String,
) {
    let result = (|| {
        let mixed = parse_optional_port(&mixed)?;
        let http = parse_optional_port(&http)?;
        let socks = parse_optional_port(&socks)?;
        if mixed.is_none() && http.is_none() && socks.is_none() {
            return Err("三个端口不可全部为空".to_string());
        }
        if mixed.is_none() && http.is_some() && http == socks {
            return Err("HTTP 与 SOCKS 端口不可相同".to_string());
        }
        Ok((mixed, http, socks))
    })();
    let Ok((mixed, http, socks)) = result else {
        if let Err(error) = result {
            if let Some(window) = weak.upgrade() {
                set_toast(&window, &error, 2);
            }
        }
        return;
    };
    let current = config::get().settings.clash;
    let unchanged = match mixed {
        Some(port) => current.mixed_port == Some(port),
        None => current.mixed_port.is_none() && current.port == http && current.socks_port == socks,
    };
    if unchanged {
        if let Some(window) = weak.upgrade() {
            sync_ports_summary(&window);
            window.global::<SettingsModel>().set_dialog_kind(0);
        }
        return;
    }
    update_core(weak.clone(), state, move |clash| {
        if let Some(port) = mixed {
            clash.mixed_port = Some(port);
            clash.port = None;
            clash.socks_port = None;
        } else {
            clash.mixed_port = None;
            clash.port = http;
            clash.socks_port = socks;
        }
    });
    if let Some(window) = weak.upgrade() {
        sync_ports_summary(&window);
        window.global::<SettingsModel>().set_dialog_kind(0);
    }
}

pub fn change_tun_stack(weak: Weak<MainWindow>, state: SharedSettingsState, index: i32) {
    let Some(stack) = tun_stack_from_index(index) else {
        return;
    };
    if config::get().settings.clash.tun.stack == stack {
        return;
    }
    update_core(weak, state, move |clash| clash.tun.stack = stack);
}

pub fn submit_tun_device(weak: Weak<MainWindow>, state: SharedSettingsState, value: String) {
    let value = platform::normalize_tun_device(&value);
    if value.is_empty() || value == "utun-" {
        if let Some(window) = weak.upgrade() {
            set_toast(&window, "网卡名称不能为空", 2);
            refresh(&window, &state);
        }
        return;
    }
    if config::get().settings.clash.tun.device == value {
        return;
    }
    update_core(weak, state, move |clash| clash.tun.device = value);
}

pub fn submit_tun_mtu(weak: Weak<MainWindow>, state: SharedSettingsState, value: String) {
    let mtu = match value.trim().parse::<u32>() {
        Ok(value) if value > 0 => value,
        _ => {
            if let Some(window) = weak.upgrade() {
                set_toast(&window, "MTU 必须是正整数", 2);
                refresh(&window, &state);
            }
            return;
        }
    };
    if config::get().settings.clash.tun.mtu == mtu {
        return;
    }
    update_core(weak, state, move |clash| clash.tun.mtu = mtu);
}

pub fn toggle_tun_strict_route(weak: Weak<MainWindow>, state: SharedSettingsState, enabled: bool) {
    if config::get().settings.clash.tun.strict_route == enabled {
        return;
    }
    update_core(weak, state, move |clash| clash.tun.strict_route = enabled);
}

pub fn toggle_tun_auto_detect_interface(
    weak: Weak<MainWindow>,
    state: SharedSettingsState,
    enabled: bool,
) {
    if config::get().settings.clash.tun.auto_detect_interface == enabled {
        return;
    }
    update_core(weak, state, move |clash| {
        clash.tun.auto_detect_interface = enabled
    });
}

pub fn toggle_tun_auto_route(weak: Weak<MainWindow>, state: SharedSettingsState, enabled: bool) {
    if config::get().settings.clash.tun.auto_route == enabled {
        return;
    }
    update_core(weak, state, move |clash| clash.tun.auto_route = enabled);
}

pub fn open_list(weak: Weak<MainWindow>, state: SharedSettingsState, kind: i32) {
    let Some(window) = weak.upgrade() else {
        return;
    };
    let cfg = config::get();
    let values = match kind {
        0 => cfg.settings.clash.tun.route_exclude_address,
        1 => cfg.settings.clash.tun.exclude_interface,
        2 => cfg.settings.proxy.bypass_list,
        _ => return,
    };
    let model = window.global::<SettingsModel>();
    {
        let mut view = lock_state(&state);
        view.dialog_list_items = values.clone();
        render_list_editor(&window, &view);
    }
    model.set_dialog_list_kind(kind);
    model.set_dialog_list_new_draft("".into());
    model.set_dialog_kind(2);
}

pub fn edit_list_item(
    weak: Weak<MainWindow>,
    state: SharedSettingsState,
    index: i32,
    value: String,
) {
    let mut view = lock_state(&state);
    let Some(item) = view.dialog_list_items.get_mut(index as usize) else {
        return;
    };
    *item = value;
    if let Some(window) = weak.upgrade() {
        window
            .global::<SettingsModel>()
            .set_dialog_list_draft(view.dialog_list_items.join("\n").into());
    }
}

pub fn add_list_item(weak: Weak<MainWindow>, state: SharedSettingsState, value: String) {
    let value = value.trim();
    if value.is_empty() {
        return;
    }
    let mut view = lock_state(&state);
    view.dialog_list_items.push(value.to_string());
    if let Some(window) = weak.upgrade() {
        render_list_editor(&window, &view);
        window
            .global::<SettingsModel>()
            .set_dialog_list_new_draft("".into());
    }
}

pub fn remove_list_item(weak: Weak<MainWindow>, state: SharedSettingsState, index: i32) {
    let mut view = lock_state(&state);
    let index = index as usize;
    if index >= view.dialog_list_items.len() {
        return;
    }
    view.dialog_list_items.remove(index);
    if let Some(window) = weak.upgrade() {
        render_list_editor(&window, &view);
    }
}

fn normalize_list(value: &str) -> Vec<String> {
    let mut values = Vec::new();
    for item in value.split(|character| matches!(character, ',' | ';' | '\n' | '\r')) {
        let item = item.trim();
        if !item.is_empty() && !values.iter().any(|current| current == item) {
            values.push(item.to_string());
        }
    }
    values
}

fn is_valid_ip_cidr(value: &str) -> bool {
    let mut parts = value.split('/');
    let Some(ip) = parts.next() else {
        return false;
    };
    let Some(prefix) = parts.next() else {
        return false;
    };
    if parts.next().is_some() || ip.is_empty() || prefix.is_empty() {
        return false;
    }
    let Ok(ip) = ip.parse::<IpAddr>() else {
        return false;
    };
    let Ok(prefix) = prefix.parse::<u8>() else {
        return false;
    };
    match ip {
        IpAddr::V4(_) => prefix <= 32,
        IpAddr::V6(_) => prefix <= 128,
    }
}

fn is_valid_bypass_pattern(value: &str) -> bool {
    if value.is_empty() || value.len() > 253 || value.starts_with('.') || value.ends_with('.') {
        return false;
    }
    value.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || "-_*".contains(character))
    })
}

fn is_valid_bypass_entry(value: &str, is_windows: bool) -> bool {
    if value.is_empty()
        || value
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
    {
        return false;
    }
    if matches!(value, "localhost" | "<local>" | "localdomain") {
        return true;
    }
    if value.contains('/') {
        return !is_windows && is_valid_ip_cidr(value);
    }
    if value.parse::<IpAddr>().is_ok() {
        return true;
    }
    is_valid_bypass_pattern(value)
}

fn validate_list(kind: i32, values: &[String]) -> Result<(), String> {
    match kind {
        0 => values
            .iter()
            .enumerate()
            .find(|(_, value)| !is_valid_ip_cidr(value))
            .map_or(Ok(()), |(index, value)| {
                Err(format!(
                    "排除网段第 {} 项不是有效的 IP CIDR：{}",
                    index + 1,
                    value
                ))
            }),
        2 => {
            let is_windows = cfg!(target_os = "windows");
            let platform = if is_windows { "Windows" } else { "Linux/macOS" };
            values
                .iter()
                .enumerate()
                .find(|(_, value)| !is_valid_bypass_entry(value, is_windows))
                .map_or(Ok(()), |(index, value)| {
                    Err(format!(
                        "跳过代理第 {} 项不是有效的 {} bypass 格式：{}",
                        index + 1,
                        platform,
                        value
                    ))
                })
        }
        _ => Ok(()),
    }
}

fn default_list(kind: i32) -> Option<Vec<String>> {
    match kind {
        0 => Some(config::default_route_exclude_address()),
        2 => Some(config::default_proxy_bypass_list()),
        _ => None,
    }
}

pub fn reset_list(weak: Weak<MainWindow>, state: SharedSettingsState, kind: i32) {
    let Some(values) = default_list(kind) else {
        return;
    };
    let Some(window) = weak.upgrade() else {
        return;
    };
    let mut view = lock_state(&state);
    view.dialog_list_items = values;
    render_list_editor(&window, &view);
    window
        .global::<SettingsModel>()
        .set_dialog_list_new_draft("".into());
}

pub fn save_list(weak: Weak<MainWindow>, state: SharedSettingsState, kind: i32, value: String) {
    let values = normalize_list(&value);
    if let Err(error) = validate_list(kind, &values) {
        if let Some(window) = weak.upgrade() {
            set_toast(&window, &error, 2);
        }
        return;
    }
    if kind == 0 || kind == 1 {
        let current = config::get().settings.clash.tun;
        let unchanged = if kind == 0 {
            current.route_exclude_address == values
        } else {
            current.exclude_interface == values
        };
        if unchanged {
            if let Some(window) = weak.upgrade() {
                sync_tun_list_summary(&window, kind);
                window.global::<SettingsModel>().set_dialog_kind(0);
            }
            return;
        }
        update_core(weak.clone(), state, move |clash| {
            if kind == 0 {
                clash.tun.route_exclude_address = values.clone();
            } else {
                clash.tun.exclude_interface = values.clone();
            }
        });
        if let Some(window) = weak.upgrade() {
            sync_tun_list_summary(&window, kind);
            window.global::<SettingsModel>().set_dialog_kind(0);
        }
        return;
    }
    if kind != 2 {
        return;
    }
    let old = config::get().settings.proxy.bypass_list;
    if old == values {
        if let Some(window) = weak.upgrade() {
            window.global::<SettingsModel>().set_dialog_kind(0);
        }
        return;
    }
    config::update(|cfg| cfg.settings.proxy.bypass_list = values.clone());
    if let Some(window) = weak.upgrade() {
        window
            .global::<SettingsModel>()
            .set_bypass_list_summary(list_summary(&values).into());
        window.global::<SettingsModel>().set_dialog_kind(0);
    }
    let token = {
        let mut view = lock_state(&state);
        view.operation_busy = true;
        next_token(&mut view)
    };
    if let Some(window) = weak.upgrade() {
        window.global::<SettingsModel>().set_operation_busy(true);
    }
    std::thread::spawn(move || {
        let result = platform::set_proxy_bypass(&values);
        invoke_ui(move || {
            let Some(window) = weak.upgrade() else {
                return;
            };
            let mut view = lock_state(&state);
            if view.next_token != token {
                return;
            }
            view.operation_busy = false;
            window.global::<SettingsModel>().set_operation_busy(false);
            if let Err(error) = result {
                set_toast(
                    &window,
                    &format!("跳过代理已保存，但平台刷新失败：{error}"),
                    2,
                );
            } else {
                set_toast(&window, "跳过代理已保存", 1);
            }
        });
    });
}

pub fn open_uwp(weak: Weak<MainWindow>, state: SharedSettingsState) {
    let Some(window) = weak.upgrade() else {
        return;
    };
    {
        let mut view = lock_state(&state);
        view.uwp_draft = enabled_uwp_packages(&view.uwp_apps);
        render_uwp(&window, &view);
    }
    window.global::<SettingsModel>().set_dialog_kind(3);
    load_uwp_async(weak, state);
}

fn load_uwp_async(weak: Weak<MainWindow>, state: SharedSettingsState) {
    if !platform::supports_uwp() {
        return;
    }
    let token = {
        let mut view = lock_state(&state);
        let token = next_token(&mut view);
        view.uwp_token = token;
        token
    };
    std::thread::spawn(move || {
        let result = platform::list_uwp_apps();
        invoke_ui(move || {
            let Some(window) = weak.upgrade() else {
                return;
            };
            let mut view = lock_state(&state);
            if view.uwp_token != token {
                return;
            }
            match result {
                Ok(apps) => {
                    view.uwp_draft = enabled_uwp_packages(&apps);
                    view.uwp_apps = apps;
                    render_uwp(&window, &view);
                }
                Err(error) => set_toast(&window, &format!("加载 UWP 列表失败：{error}"), 2),
            }
        });
    });
}

pub fn search_uwp(state: SharedSettingsState, value: String, weak: Weak<MainWindow>) {
    let mut view = lock_state(&state);
    view.uwp_query = value;
    if let Some(window) = weak.upgrade() {
        render_uwp(&window, &view);
    }
}

pub fn toggle_uwp(
    weak: Weak<MainWindow>,
    state: SharedSettingsState,
    package_family_name: String,
    enabled: bool,
) {
    if package_family_name.trim().is_empty() {
        return;
    }
    let Some(window) = weak.upgrade() else {
        return;
    };
    let mut view = lock_state(&state);
    if view.operation_busy
        || !view
            .uwp_apps
            .iter()
            .any(|app| app.package_family_name == package_family_name)
    {
        return;
    }
    if enabled {
        if !view.uwp_draft.contains(&package_family_name) {
            view.uwp_draft.push(package_family_name);
        }
    } else {
        view.uwp_draft.retain(|value| value != &package_family_name);
    }
    render_uwp(&window, &view);
}

fn uwp_changes(current: &[String], desired: &[String]) -> Vec<(String, bool)> {
    let mut changes = current
        .iter()
        .filter(|value| !desired.contains(value))
        .cloned()
        .map(|value| (value, false))
        .collect::<Vec<_>>();
    changes.extend(
        desired
            .iter()
            .filter(|value| !current.contains(value))
            .cloned()
            .map(|value| (value, true)),
    );
    changes
}

pub fn save_uwp(weak: Weak<MainWindow>, state: SharedSettingsState) {
    let (current, desired, changes, token) = {
        let mut view = lock_state(&state);
        if view.operation_busy {
            return;
        }
        let current = enabled_uwp_packages(&view.uwp_apps);
        let desired = view.uwp_draft.clone();
        let changes = uwp_changes(&current, &desired);
        if changes.is_empty() {
            if let Some(window) = weak.upgrade() {
                window.global::<SettingsModel>().set_dialog_kind(0);
            }
            return;
        }
        view.operation_busy = true;
        let token = next_token(&mut view);
        (current, desired, changes, token)
    };
    if let Some(window) = weak.upgrade() {
        window.global::<SettingsModel>().set_operation_busy(true);
    }
    std::thread::spawn(move || {
        let result = platform::set_uwp_loopback_batch(&changes);
        invoke_ui(move || {
            let Some(window) = weak.upgrade() else {
                return;
            };
            let mut view = lock_state(&state);
            if view.next_token != token {
                return;
            }
            view.operation_busy = false;
            window.global::<SettingsModel>().set_operation_busy(false);
            match result {
                Ok(()) => {
                    view.uwp_draft = desired;
                    window.global::<SettingsModel>().set_dialog_kind(0);
                    set_toast(&window, "UWP 回环设置已更新", 1);
                }
                Err(error) => {
                    view.uwp_draft = current;
                    render_uwp(&window, &view);
                    set_toast(&window, &format!("UWP 回环设置失败：{error}"), 2);
                }
            }
        });
    });
}

pub fn close_dialog(weak: Weak<MainWindow>, state: SharedSettingsState) {
    if let Some(window) = weak.upgrade() {
        let mut view = lock_state(&state);
        view.uwp_draft = enabled_uwp_packages(&view.uwp_apps);
        window.global::<SettingsModel>().set_dialog_kind(0);
    }
}

/// 点击浮动按钮后才合并配置并重启核心。
pub fn apply_core(weak: Weak<MainWindow>, state: SharedSettingsState) {
    let root = {
        let mut view = lock_state(&state);
        if view.applying_core || !view.core_dirty {
            return;
        }
        view.applying_core = true;
        view.root.clone()
    };
    if let Some(window) = weak.upgrade() {
        window.global::<SettingsModel>().set_applying_core(true);
    }
    std::thread::spawn(move || {
        let result = core::on_config_changed(&root).map_err(|error| error.to_string());
        invoke_ui(move || {
            let Some(window) = weak.upgrade() else {
                return;
            };
            let mut view = lock_state(&state);
            view.applying_core = false;
            window.global::<SettingsModel>().set_applying_core(false);
            match result {
                Ok(()) => {
                    view.core_dirty = false;
                    window.global::<SettingsModel>().set_core_dirty(false);
                    set_toast(&window, "配置已应用", 1);
                }
                Err(error) => {
                    window.global::<SettingsModel>().set_core_dirty(true);
                    set_toast(&window, &format!("应用配置失败：{error}"), 2);
                }
            }
            home::refresh_runtime_state(&window);
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_port_boundaries() {
        assert_eq!(parse_optional_port(""), Ok(None));
        assert_eq!(parse_optional_port("1"), Ok(Some(1)));
        assert_eq!(parse_optional_port("65535"), Ok(Some(65535)));
        assert!(parse_optional_port("0").is_err());
        assert!(parse_optional_port("65536").is_err());
        assert!(parse_optional_port("abc").is_err());
    }

    #[test]
    fn renders_port_summaries() {
        let mut clash = config::ClashSettings::default();
        clash.mixed_port = Some(7890);
        assert_eq!(ports_summary(&clash), "混合端口 7890");

        clash.mixed_port = None;
        clash.port = Some(7891);
        clash.socks_port = Some(7892);
        assert_eq!(ports_summary(&clash), "HTTP 7891 · SOCKS 7892");

        clash.socks_port = None;
        assert_eq!(ports_summary(&clash), "HTTP 7891");
    }

    #[test]
    fn renders_list_summaries() {
        assert_eq!(list_summary(&[]), "暂无");
        assert_eq!(
            list_summary(&["10.0.0.0/8".to_string(), "192.168.0.0/16".to_string()]),
            "10.0.0.0/8、192.168.0.0/16"
        );
    }

    #[test]
    fn normalizes_lists_by_trim_dedup_and_order() {
        assert_eq!(
            normalize_list(" a\n\nb, a; b "),
            vec!["a".to_string(), "b".to_string()]
        );
    }

    #[test]
    fn validates_ip_cidr_boundaries() {
        assert!(is_valid_ip_cidr("10.0.0.0/8"));
        assert!(is_valid_ip_cidr("2001:db8::/32"));
        assert!(is_valid_ip_cidr("0.0.0.0/0"));
        assert!(is_valid_ip_cidr("::/0"));
        assert!(!is_valid_ip_cidr("10.0.0.1"));
        assert!(!is_valid_ip_cidr("10.0.0.0/33"));
        assert!(!is_valid_ip_cidr("2001:db8::/129"));
        assert!(!is_valid_ip_cidr("not-an-ip/24"));
    }

    #[test]
    fn validates_platform_bypass_entries() {
        assert!(is_valid_bypass_entry("localhost", true));
        assert!(is_valid_bypass_entry("127.*", true));
        assert!(is_valid_bypass_entry("2001:db8::1", true));
        assert!(!is_valid_bypass_entry("192.168.0.0/16", true));

        assert!(is_valid_bypass_entry("127.0.0.1/8", false));
        assert!(is_valid_bypass_entry("2001:db8::/32", false));
        assert!(is_valid_bypass_entry("*.example.com", false));
        assert!(is_valid_bypass_entry("<local>", false));
        assert!(!is_valid_bypass_entry("bad;entry", false));
        assert!(!is_valid_bypass_entry("bad entry", false));
    }

    #[test]
    fn validates_only_supported_list_kinds() {
        assert!(validate_list(0, &["10.0.0.0/8".to_string()]).is_ok());
        assert!(validate_list(0, &["10.0.0.1".to_string()]).is_err());
        assert!(validate_list(1, &["任意接口名".to_string()]).is_ok());
    }

    #[test]
    fn default_lists_are_available_for_reset() {
        assert_eq!(
            default_list(0),
            Some(config::default_route_exclude_address())
        );
        assert_eq!(default_list(2), Some(config::default_proxy_bypass_list()));
        assert_eq!(default_list(1), None);
    }

    #[test]
    fn computes_only_changed_uwp_entries() {
        let current = vec!["old-a".to_string(), "keep".to_string()];
        let desired = vec!["keep".to_string(), "new-b".to_string()];
        assert_eq!(
            uwp_changes(&current, &desired),
            vec![("old-a".to_string(), false), ("new-b".to_string(), true)]
        );
        assert!(uwp_changes(&current, &current).is_empty());
    }

    #[test]
    fn derives_uwp_current_state_from_system_flags() {
        let apps = vec![
            platform::UwpApp {
                name: "关闭".to_string(),
                package_family_name: "disabled_abc".to_string(),
                enabled: false,
            },
            platform::UwpApp {
                name: "启用".to_string(),
                package_family_name: "enabled_abc".to_string(),
                enabled: true,
            },
        ];
        assert_eq!(enabled_uwp_packages(&apps), vec!["enabled_abc".to_string()]);
    }

    #[test]
    fn maps_enum_indices() {
        assert_eq!(
            log_level_from_index(log_level_index(LogLevel::Debug)),
            Some(LogLevel::Debug)
        );
        assert_eq!(
            tun_stack_from_index(tun_stack_index(TunStack::Gvisor)),
            Some(TunStack::Gvisor)
        );
        assert!(log_level_from_index(-1).is_none());
    }
}
