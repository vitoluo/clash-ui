use slint::winit_030::WinitWindowAccessor;
use slint::ComponentHandle;

use super::app_context::{effective_dark, AppContext};
use super::config;
use crate::controller::{
    config as config_page, connections, home, logs, proxy, r#override as override_page, rules,
    settings, tray,
};

pub(crate) fn bind_app_state(context: &AppContext) {
    let weak = context.main_window.as_weak();
    context
        .main_window
        .global::<crate::AppState>()
        .on_change_theme({
            let weak = weak.clone();
            move |mode_index| {
                let mode = match mode_index {
                    0 => config::ThemeMode::System,
                    1 => config::ThemeMode::Light,
                    2 => config::ThemeMode::Dark,
                    _ => config::ThemeMode::System,
                };
                config::update(|cfg| cfg.settings.app.theme = mode);
                if let Some(window) = weak.upgrade() {
                    window
                        .global::<crate::Theme>()
                        .set_dark(effective_dark(mode));
                    window
                        .global::<crate::AppState>()
                        .set_theme_mode(mode_index);
                }
            }
        });

    context
        .main_window
        .global::<crate::AppState>()
        .on_confirm_tun_enable(tray::confirm_tun_enable);
    context
        .main_window
        .global::<crate::AppState>()
        .on_cancel_tun_enable(tray::cancel_tun_enable);

    context
        .main_window
        .global::<crate::AppState>()
        .on_navigate({
            let weak = weak.clone();
            let start = context.start;
            let proxy_state = context.proxy_state.clone();
            let rules_state = context.rules_state.clone();
            let config_state = context.config_state.clone();
            let override_state = context.override_state.clone();
            let connections_state = context.connections_state.clone();
            let logs_state = context.logs_state.clone();
            let settings_state = context.settings_state.clone();
            move |page| {
                let Some(window) = weak.upgrade() else {
                    return;
                };
                window.global::<crate::AppState>().set_current_page(page);
                match page {
                    0 => home::refresh(&window, &start),
                    1 => proxy::refresh_async(window.as_weak(), proxy_state.clone()),
                    2 => rules::refresh_async(window.as_weak(), rules_state.clone()),
                    3 => config_page::refresh_async(window.as_weak(), config_state.clone()),
                    4 => override_page::refresh_async(window.as_weak(), override_state.clone()),
                    5 => connections::sync_ui(&window, &connections_state),
                    6 => logs::sync_ui(&window, &logs_state),
                    7 => settings::refresh(&window, &settings_state),
                    _ => {}
                }
            }
        });
}

pub(crate) fn bind_window_callbacks(context: &AppContext) {
    bind_window_actions(context);
    bind_window_resize(context);
}

fn bind_window_actions(context: &AppContext) {
    let weak = context.main_window.as_weak();
    context.main_window.on_start_drag({
        let weak = weak.clone();
        move || {
            if let Some(window) = weak.upgrade() {
                window.window().with_winit_window(|w| {
                    let _ = w.drag_window();
                });
            }
        }
    });
    context.main_window.on_minimize({
        let weak = weak.clone();
        move || {
            if let Some(window) = weak.upgrade() {
                window.window().with_winit_window(|w| w.set_minimized(true));
            }
        }
    });
    context.main_window.on_toggle_maximize({
        let weak = weak.clone();
        move || {
            if let Some(window) = weak.upgrade() {
                let maximized = window.window().is_maximized();
                window
                    .window()
                    .with_winit_window(|w| w.set_maximized(!maximized));
            }
        }
    });
    context.main_window.on_close_window({
        let weak = weak.clone();
        move || {
            if let Some(window) = weak.upgrade() {
                let _ = window.window().hide();
            }
        }
    });
}

fn bind_window_resize(context: &AppContext) {
    let weak = context.main_window.as_weak();
    context.main_window.on_start_resize(move |direction: i32| {
        use slint::winit_030::winit::window::ResizeDirection;

        let direction = match direction {
            0 => ResizeDirection::North,
            1 => ResizeDirection::South,
            2 => ResizeDirection::West,
            3 => ResizeDirection::East,
            4 => ResizeDirection::NorthEast,
            5 => ResizeDirection::SouthEast,
            6 => ResizeDirection::NorthWest,
            7 => ResizeDirection::SouthWest,
            _ => return,
        };
        if let Some(window) = weak.upgrade() {
            window.window().with_winit_window(|w| {
                let _ = w.drag_resize_window(direction);
            });
        }
    });
    context
        .main_window
        .window()
        .on_close_requested(|| slint::CloseRequestResponse::HideWindow);
}
