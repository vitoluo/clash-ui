use std::path::PathBuf;
use std::time::Instant;

use slint::{ComponentHandle, LogicalPosition, LogicalSize};

use super::{app_bindings, config};
use crate::clash::{core, stream};
use crate::controller::{
    common::business, config as config_page, connections, home, logs, proxy,
    r#override as override_page, rules, settings, speed_stats, tray,
};
use crate::{platform, ClashTray, MainWindow};

/// 应用运行期间共享的生命周期资源。
pub(crate) struct AppContext {
    pub(crate) root: PathBuf,
    pub(crate) start: Instant,
    pub(crate) main_window: MainWindow,
    pub(crate) tray: Option<ClashTray>,
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
    pub(crate) fn new(root: PathBuf, start: Instant) -> Result<Self, anyhow::Error> {
        let connections_recorder =
            connections::start_recorder(stream::conns_rx()?, crate::event::subscribe_core_state());
        let logs_recorder =
            logs::start_recorder(stream::logs_rx()?, crate::event::subscribe_core_state());

        let main_window = MainWindow::new()?;
        connections::attach_ui(&connections_recorder, main_window.as_weak());
        logs::attach_ui(&logs_recorder, main_window.as_weak());
        configure_window(&main_window);
        // live-preview 解释器无法正确显示 SystemTrayIcon。
        let tray = if std::env::var_os("SLINT_LIVE_PREVIEW").is_some() {
            None
        } else {
            match ClashTray::new() {
                Ok(tray) => match tray.show() {
                    Ok(()) => Some(tray),
                    Err(error) => {
                        crate::log::error(format_args!(
                            "显示系统托盘失败，继续运行主界面：{error}"
                        ));
                        None
                    }
                },
                Err(error) => {
                    crate::log::error(format_args!("创建系统托盘失败，继续运行主界面：{error}"));
                    None
                }
            }
        };

        let proxy_state = proxy::new_state();
        let rules_state = rules::new_state();
        let config_state = config_page::new_state(root.clone());
        let override_state = override_page::new_state(root.clone());
        let connections_state = connections_recorder.state();
        let logs_state = logs_recorder.state();
        let settings_state = settings::new_state(root.clone());
        register_system_proxy_lifecycle_listener();
        if let Err(error) = core::start_core(&root) {
            crate::log::error(format_args!("启动 clash 核心失败: {error}"));
        }
        configure_theme(&main_window);

        Ok(Self {
            root,
            start,
            main_window,
            tray,
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
        rules::listen_core_state(&self.main_window, self.rules_state.clone());
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
        tray::init(
            self.root.clone(),
            self.main_window.as_weak(),
            self.tray.as_ref(),
        );
    }

    pub(crate) fn show_and_run(&self) -> Result<(), slint::PlatformError> {
        if !config::settings().app.silent_start {
            self.main_window.show()?;
        }
        slint::run_event_loop_until_quit()?;
        Ok(())
    }
}

fn register_system_proxy_lifecycle_listener() {
    let mut core_state = crate::event::subscribe_core_state();
    crate::runtime::spawn_task(async move {
        loop {
            if core_state.changed().await.is_err() {
                return;
            }
            let state = *core_state.borrow_and_update();
            if state.running {
                business::restore_system_proxy(state);
                continue;
            }

            if config::get().proxy_status.system {
                match crate::runtime::spawn_blocking(business::clear_system_proxy).await {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => {
                        crate::log::error(format_args!("核心停止时清除系统代理失败：{error}"));
                    }
                    Err(error) => {
                        crate::log::error(format_args!("核心停止时清除系统代理任务失败：{error}"));
                    }
                }
            }
        }
    });
}

fn configure_window(main_window: &MainWindow) {
    let (sw, sh) = match platform::get_primary_screen_size() {
        Ok(size) => size,
        Err(error) => {
            crate::log::error(format_args!("读取主显示器尺寸失败：{error}"));
            (1800.0, 1200.0)
        }
    };
    let (width, height) = (sw / 2.0, sh / 2.0);
    let (width, height) = if width < 900.0 || height < 600.0 {
        (900.0, 600.0)
    } else {
        (width, height)
    };
    let window = main_window.window();
    window.set_size(LogicalSize::new(width, height));
    window.set_position(LogicalPosition::new(
        (sw - width) / 2.0,
        (sh - height) / 2.0,
    ));
}

fn configure_theme(main_window: &MainWindow) {
    let theme_mode = config::get().settings.app.theme;
    main_window
        .global::<crate::Theme>()
        .set_dark(effective_dark(theme_mode));
    main_window
        .global::<crate::AppState>()
        .set_theme_mode(theme_index(theme_mode));
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
        config::ThemeMode::System => match platform::is_dark_mode() {
            Ok(is_dark) => is_dark,
            Err(error) => {
                crate::log::error(format_args!("检测系统主题失败：{error}"));
                false
            }
        },
        config::ThemeMode::Light => false,
        config::ThemeMode::Dark => true,
    }
}
