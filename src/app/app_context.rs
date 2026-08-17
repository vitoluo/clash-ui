use std::path::PathBuf;
use std::time::Instant;

use slint::{ComponentHandle, LogicalPosition, LogicalSize};

use super::{app_bindings, config};
use crate::clash::{api, core};
use crate::controller::{
    config as config_page, connections, home, logs, proxy, r#override as override_page, rules,
    settings, speed_stats, tray,
};
use crate::{platform, MainWindow};

/// 应用运行期间共享的生命周期资源。
pub(crate) struct AppContext {
    pub(crate) root: PathBuf,
    pub(crate) start: Instant,
    pub(crate) main_window: MainWindow,
    pub(crate) proxy_state: proxy::SharedProxyState,
    pub(crate) rules_state: rules::SharedRulesState,
    pub(crate) config_state: config_page::SharedConfigState,
    pub(crate) override_state: override_page::SharedOverrideState,
    pub(crate) connections_state: connections::SharedConnectionsState,
    pub(crate) logs_state: logs::SharedLogsState,
    pub(crate) settings_state: settings::SharedSettingsState,
    pub(crate) _connections_recorder: connections::ConnectionsRecorder,
    pub(crate) _logs_recorder: logs::LogsRecorder,
    pub(crate) home_timer: slint::Timer,
}

impl AppContext {
    pub(crate) fn new(root: PathBuf, start: Instant) -> Result<Self, Box<dyn std::error::Error>> {
        let connections_recorder =
            connections::start_recorder(api::conns_rx().expect("连接广播发送端初始化失败"));
        let logs_recorder = logs::start_recorder(api::logs_rx().expect("日志广播发送端初始化失败"));

        let main_window = MainWindow::new()?;
        connections::attach_ui(&connections_recorder, main_window.as_weak());
        logs::attach_ui(&logs_recorder, main_window.as_weak());
        configure_window(&main_window);

        let proxy_state = proxy::new_state();
        let rules_state = rules::new_state();
        let config_state = config_page::new_state(root.clone());
        let override_state = override_page::new_state(root.clone());
        let connections_state = connections_recorder.state();
        let logs_state = logs_recorder.state();
        let settings_state = settings::new_state(root.clone());
        settings::refresh(&main_window, &settings_state);
        register_core_stop_cleanup(
            &main_window,
            proxy_state.clone(),
            rules_state.clone(),
            connections_state.clone(),
            logs_state.clone(),
        );
        if let Err(error) = core::on_config_changed(&root) {
            crate::log::error(format_args!("启动 clash 核心失败: {error}"));
        }
        configure_theme(&main_window, &start);

        Ok(Self {
            root,
            start,
            main_window,
            proxy_state,
            rules_state,
            config_state,
            override_state,
            connections_state,
            logs_state,
            settings_state,
            _connections_recorder: connections_recorder,
            _logs_recorder: logs_recorder,
            home_timer: slint::Timer::default(),
        })
    }

    pub(crate) fn bind_callbacks(&self) {
        app_bindings::bind_app_state(self);
        settings::bind_callbacks(&self.main_window, self.settings_state.clone());
        logs::bind_callbacks(&self.main_window, self.logs_state.clone());
        connections::bind_callbacks(&self.main_window, self.connections_state.clone());
        proxy::bind_callbacks(&self.main_window, self.proxy_state.clone());
        config_page::bind_callbacks(&self.main_window, self.config_state.clone());
        override_page::bind_callbacks(&self.main_window, self.override_state.clone());
        home::bind_callbacks(
            &self.main_window,
            self.root.clone(),
            self.start,
            &self.home_timer,
        );
        app_bindings::bind_window_callbacks(self);
    }

    pub(crate) fn start_services(&self) {
        speed_stats::start(&self.main_window);
        tray::init(self.root.clone(), self.main_window.as_weak());
        tray::restore_system_proxy();
    }

    pub(crate) fn show_and_run(&self) -> Result<(), Box<dyn std::error::Error>> {
        if !config::get().settings.app.silent_start {
            self.main_window.show()?;
        }
        slint::run_event_loop_until_quit()?;
        Ok(())
    }
}

fn register_core_stop_cleanup(
    window: &MainWindow,
    proxy_state: proxy::SharedProxyState,
    rules_state: rules::SharedRulesState,
    connections_state: connections::SharedConnectionsState,
    logs_state: logs::SharedLogsState,
) {
    let weak = window.as_weak();
    core::set_stop_handler(move || {
        proxy::clear_runtime(&proxy_state);
        rules::clear_runtime(&rules_state);
        connections::clear_runtime(&connections_state);
        logs::clear_runtime(&logs_state);

        let weak = weak.clone();
        let proxy_state = proxy_state.clone();
        let rules_state = rules_state.clone();
        let connections_state = connections_state.clone();
        let logs_state = logs_state.clone();
        if let Err(error) = slint::invoke_from_event_loop(move || {
            let Some(window) = weak.upgrade() else {
                return;
            };
            proxy::sync_ui(&window, &proxy_state);
            rules::sync_ui(&window, &rules_state);
            connections::sync_ui(&window, &connections_state);
            logs::sync_ui(&window, &logs_state);
        }) {
            crate::log::error(format_args!("核心停止后清理页面数据失败：{error}"));
        }
    });
}

fn configure_window(main_window: &MainWindow) {
    let (sw, sh) = platform::get_primary_screen_size();
    let width = (sw / 2.0).max(900.0);
    let height = (sh / 2.0).max(600.0);
    let window = main_window.window();
    window.set_size(LogicalSize::new(width, height));
    window.set_position(LogicalPosition::new(
        (sw - width) / 2.0,
        (sh - height) / 2.0,
    ));
}

fn configure_theme(main_window: &MainWindow, start: &Instant) {
    let theme_mode = config::get().settings.app.theme;
    main_window
        .global::<crate::Theme>()
        .set_dark(effective_dark(theme_mode));
    main_window
        .global::<crate::AppState>()
        .set_theme_mode(theme_index(theme_mode));
    home::refresh(main_window, start);
}

fn theme_index(mode: config::ThemeMode) -> i32 {
    match mode {
        config::ThemeMode::System => 0,
        config::ThemeMode::Light => 1,
        config::ThemeMode::Dark => 2,
    }
}

pub(crate) fn effective_dark(mode: config::ThemeMode) -> bool {
    match mode {
        config::ThemeMode::System => platform::is_dark_mode(),
        config::ThemeMode::Light => false,
        config::ThemeMode::Dark => true,
    }
}
