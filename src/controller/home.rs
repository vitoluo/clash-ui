// 主页数据刷新与轻量状态转换。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::app::config;
use crate::clash::{api, core, stream};
use crate::consts::RUNTIME_UI_DIR;
use crate::controller::common::business;
use crate::{platform, MainWindow};
use serde::Deserialize;
use slint::{ComponentHandle, Timer};
use sysinfo::{Pid, ProcessesToUpdate, System};

const HOME_REFRESH_INTERVAL: Duration = Duration::from_secs(1);
const GITHUB_LATEST_RELEASE_API: &str =
    "https://api.github.com/repos/vitoluo/clash-ui/releases/latest";
const GITHUB_RELEASE_URL_PREFIX: &str = "https://github.com/vitoluo/clash-ui/releases/";
const VERSION_CHECK_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    html_url: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct ReleaseVersion {
    major: u64,
    minor: u64,
    patch: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct VersionUpdate {
    new_version: String,
    release_url: String,
}

fn parse_release_version(value: &str) -> Option<ReleaseVersion> {
    let value = value.trim();
    let value = value
        .strip_prefix('v')
        .or_else(|| value.strip_prefix('V'))
        .unwrap_or(value);
    let mut components = value.split('.');
    let major = parse_version_component(components.next()?)?;
    let minor = parse_version_component(components.next()?)?;
    let patch = parse_version_component(components.next()?)?;
    if components.next().is_some() {
        return None;
    }
    Some(ReleaseVersion {
        major,
        minor,
        patch,
    })
}

fn parse_version_component(value: &str) -> Option<u64> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value.parse().ok()
}

fn is_valid_release_url(value: &str) -> bool {
    value.starts_with(GITHUB_RELEASE_URL_PREFIX)
        && value.len() > GITHUB_RELEASE_URL_PREFIX.len()
        && !value.bytes().any(|byte| byte.is_ascii_whitespace())
}

fn version_update_from_release(
    release: GithubRelease,
    current_version: &str,
) -> Option<VersionUpdate> {
    let new_version = release.tag_name.trim().to_string();
    let release_url = release.html_url.trim().to_string();
    let current_version = parse_release_version(current_version)?;
    let latest_version = parse_release_version(&new_version)?;
    if latest_version <= current_version || !is_valid_release_url(&release_url) {
        return None;
    }
    Some(VersionUpdate {
        new_version,
        release_url,
    })
}

async fn check_for_update() -> Option<VersionUpdate> {
    let release: GithubRelease = crate::network::http::get_json(
        GITHUB_LATEST_RELEASE_API,
        None,
        None,
        VERSION_CHECK_TIMEOUT,
    )
    .await
    .ok()?;
    version_update_from_release(release, env!("CARGO_PKG_VERSION"))
}

#[derive(Debug, Default)]
struct HomeStaticState {
    core_state: crate::event::CoreState,
    loaded_generation: Option<u64>,
    loading_generation: Option<u64>,
    request_token: u64,
}

static HOME_STATIC_STATE: OnceLock<Mutex<HomeStaticState>> = OnceLock::new();

fn home_static_state() -> &'static Mutex<HomeStaticState> {
    HOME_STATIC_STATE.get_or_init(|| Mutex::new(HomeStaticState::default()))
}

fn next_static_request_token(state: &mut HomeStaticState) -> u64 {
    state.request_token = state.request_token.wrapping_add(1).max(1);
    state.request_token
}

struct CoreUpgradeGuard {
    active: Arc<AtomicBool>,
}

impl CoreUpgradeGuard {
    fn try_acquire(active: Arc<AtomicBool>) -> Option<Self> {
        active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| Self { active })
    }
}

impl Drop for CoreUpgradeGuard {
    fn drop(&mut self) {
        self.active.store(false, Ordering::Release);
    }
}

struct HomeMetricsSampler {
    system: Mutex<System>,
    pending: AtomicBool,
    sampled: AtomicBool,
    latest_rss: AtomicU64,
}

impl HomeMetricsSampler {
    fn new() -> Self {
        Self {
            system: Mutex::new(System::new()),
            pending: AtomicBool::new(false),
            sampled: AtomicBool::new(false),
            latest_rss: AtomicU64::new(0),
        }
    }

    fn request(self: &Arc<Self>, weak: slint::Weak<MainWindow>) {
        if !self.try_begin() {
            return;
        }
        let sampler = self.clone();
        crate::runtime::spawn_blocking(move || {
            let pid = Pid::from_u32(std::process::id());
            let rss = {
                let mut system = sampler
                    .system
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                system.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
                system
                    .process(pid)
                    .map(|process| process.memory())
                    .unwrap_or(0)
            };
            sampler.latest_rss.store(rss, Ordering::Release);
            sampler.sampled.store(true, Ordering::Release);
            sampler.finish();
            if let Err(error) = slint::invoke_from_event_loop(move || {
                let Some(window) = weak.upgrade() else { return };
                if home_is_visible(&window) {
                    window
                        .global::<crate::HomeModel>()
                        .set_client_mem(fmt_mb(rss).into());
                }
            }) {
                crate::log::error(format_args!("投递客户端内存刷新任务失败：{error}"));
            }
        });
    }

    fn try_begin(&self) -> bool {
        !self.pending.swap(true, Ordering::AcqRel)
    }

    fn finish(&self) {
        self.pending.store(false, Ordering::Release);
    }
}

static METRICS_SAMPLER: OnceLock<Arc<HomeMetricsSampler>> = OnceLock::new();

fn metrics_sampler() -> &'static Arc<HomeMetricsSampler> {
    METRICS_SAMPLER.get_or_init(|| Arc::new(HomeMetricsSampler::new()))
}

fn home_page_is_active(page: i32) -> bool {
    page == 0
}

fn home_is_visible(window: &MainWindow) -> bool {
    home_page_is_active(window.global::<crate::AppState>().get_current_page())
        && window.window().is_visible()
}

fn set_toast(window: &MainWindow, message: &str, variant: i32) {
    let model = window.global::<crate::HomeModel>();
    model.set_toast_message(message.to_string().into());
    model.set_toast_variant(variant);
    model.set_toast_visible(true);
}

pub(crate) fn bind_callbacks(window: &MainWindow, root: PathBuf, start: Instant, timer: &Timer) {
    let home = window.global::<crate::HomeModel>();
    home.set_platform_name(platform::platform_name().into());
    home.set_client_version(env!("CARGO_PKG_VERSION").into());
    bind_ui_updates(window);
    bind_mode_and_proxy(window, root.clone());
    bind_core(window, root.clone(), start);
    bind_online_panel(window, root);
    bind_version_update(window);
    bind_timer(window, timer, start);
}

fn bind_ui_updates(window: &MainWindow) {
    let weak = window.as_weak();
    let mut outbound_mode = crate::event::subscribe_outbound_mode();
    let mut system_proxy = crate::event::subscribe_system_proxy();
    let mut tun_proxy = crate::event::subscribe_tun_proxy();
    let mut core_state = crate::event::subscribe_core_state();
    let mut tun_confirmation = crate::event::subscribe_tun_confirmation_requests();
    // watch 接收器订阅后默认已读当前版本；控制器可能晚于发布，因此主动处理当前值。
    core_state.mark_changed();
    crate::runtime::spawn_task(async move {
        loop {
            let update = tokio::select! {
                result = outbound_mode.changed() => {
                    if result.is_err() {
                        return;
                    }
                    HomeUiUpdate::OutboundMode(outbound_mode.borrow_and_update().clone())
                }
                result = system_proxy.changed() => {
                    if result.is_err() {
                        return;
                    }
                    HomeUiUpdate::SystemProxy(*system_proxy.borrow_and_update())
                }
                result = tun_proxy.changed() => {
                    if result.is_err() {
                        return;
                    }
                    HomeUiUpdate::TunProxy(*tun_proxy.borrow_and_update())
                }
                result = core_state.changed() => {
                    if result.is_err() {
                        return;
                    }
                    HomeUiUpdate::CoreState(*core_state.borrow_and_update())
                }
                result = tun_confirmation.changed() => {
                    if result.is_err() {
                        return;
                    }
                    let _ = tun_confirmation.borrow_and_update();
                    HomeUiUpdate::TunConfirmationRequested
                }
            };
            let weak = weak.clone();
            if let Err(error) = slint::invoke_from_event_loop(move || {
                if let Some(window) = weak.upgrade() {
                    apply_ui_update(&window, update);
                }
            }) {
                crate::log::error(format_args!("投递主页状态更新失败：{error}"));
                return;
            }
        }
    });
}

enum HomeUiUpdate {
    OutboundMode(String),
    SystemProxy(bool),
    TunProxy(bool),
    CoreState(crate::event::CoreState),
    TunConfirmationRequested,
}

fn apply_ui_update(window: &MainWindow, update: HomeUiUpdate) {
    let home = window.global::<crate::HomeModel>();
    match update {
        HomeUiUpdate::OutboundMode(mode) => {
            home.set_outbound_mode(outbound_mode_label(&mode).into());
        }
        HomeUiUpdate::SystemProxy(enabled) => {
            home.set_system_proxy(enabled);
        }
        HomeUiUpdate::TunProxy(enabled) => {
            home.set_tun_proxy(enabled);
        }
        HomeUiUpdate::CoreState(state) => {
            refresh_static(window.as_weak(), state, false);
        }
        HomeUiUpdate::TunConfirmationRequested => {
            let _ = window.show();
            window
                .global::<crate::AppState>()
                .set_tun_confirm_open(true);
        }
    }
}

fn outbound_mode_label(mode: &str) -> &'static str {
    match mode {
        "rule" => "规则模式",
        "global" => "全局模式",
        "direct" => "直连模式",
        _ => "—",
    }
}

fn bind_mode_and_proxy(window: &MainWindow, root: PathBuf) {
    window.global::<crate::HomeModel>().on_change_mode(|index| {
        let mode = match index {
            0 => "rule",
            1 => "global",
            2 => "direct",
            _ => return,
        };
        business::set_mode(mode);
    });
    window.global::<crate::HomeModel>().on_copy_env(|index| {
        let terminal = match index {
            0 => business::Terminal::PowerShell,
            1 => business::Terminal::Cmd,
            2 => business::Terminal::Bash,
            _ => return,
        };
        business::copy_proxy_env(terminal);
    });
    window
        .global::<crate::HomeModel>()
        .on_toggle_system_proxy(business::toggle_system_proxy);
    window
        .global::<crate::HomeModel>()
        .on_toggle_tun(move || business::toggle_tun(root.clone()));
}

fn bind_core(window: &MainWindow, root: PathBuf, start: Instant) {
    let weak = window.as_weak();
    let upgrade_active = Arc::new(AtomicBool::new(false));
    window.global::<crate::HomeModel>().on_restart_core({
        let root = root.clone();
        move || {
            let root = root.clone();
            crate::runtime::spawn_blocking(move || {
                if let Err(error) = core::restart_core(&root) {
                    crate::log::error(format_args!("重启 clash 核心失败: {error}"));
                }
            });
        }
    });
    window.global::<crate::HomeModel>().on_update_core({
        let weak = weak.clone();
        let upgrade_active = upgrade_active.clone();
        move || {
            let Some(guard) = CoreUpgradeGuard::try_acquire(upgrade_active.clone()) else {
                return;
            };
            let request_core_state = *crate::event::subscribe_core_state().borrow();
            if let Some(window) = weak.upgrade() {
                window.global::<crate::HomeModel>().set_core_updating(true);
            }
            let weak = weak.clone();
            let task_start = start;
            crate::runtime::spawn_task(async move {
                let _guard = guard;
                let update_error = api::upgrade().await.err().map(|error| {
                    crate::log::error(format_args!("更新 clash 核心失败: {error}"));
                    format!("更新核心失败：{error}")
                });
                if let Err(error) = slint::invoke_from_event_loop(move || {
                    if let Some(window) = weak.upgrade() {
                        window.global::<crate::HomeModel>().set_core_updating(false);
                        if *crate::event::subscribe_core_state().borrow() != request_core_state {
                            return;
                        }
                        if let Some(error) = update_error {
                            set_toast(&window, &error, 2);
                        }
                        refresh_explicit(&window, &task_start);
                    }
                }) {
                    crate::log::error(format_args!("投递核心更新刷新任务失败：{error}"));
                }
            });
        }
    });
}

fn bind_online_panel(window: &MainWindow, root: PathBuf) {
    window
        .global::<crate::HomeModel>()
        .on_open_online_panel(move || {
            let task_root = root.clone();
            crate::runtime::spawn_task(async move {
                match prepare_online_panel(&task_root).await {
                    Ok(url) => {
                        let result =
                            crate::runtime::spawn_blocking(move || platform::open_url(&url)).await;
                        match result {
                            Ok(Ok(())) => {}
                            Ok(Err(error)) => {
                                crate::log::error(format_args!("打开在线面板失败：{error}"));
                            }
                            Err(error) => {
                                crate::log::error(format_args!("打开在线面板任务失败：{error}"));
                            }
                        }
                    }
                    Err(error) => crate::log::error(format_args!("准备在线面板失败：{error}")),
                }
            });
        });
}

fn bind_version_update(window: &MainWindow) {
    let weak = window.as_weak();
    window.global::<crate::HomeModel>().on_open_client_update({
        let weak = weak.clone();
        move || {
            let Some(window) = weak.upgrade() else {
                return;
            };
            let release_url = window
                .global::<crate::HomeModel>()
                .get_client_update_url()
                .to_string();
            if !is_valid_release_url(&release_url) {
                return;
            }
            crate::runtime::spawn_task(async move {
                let result =
                    crate::runtime::spawn_blocking(move || platform::open_url(&release_url)).await;
                match result {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => {
                        crate::log::error(format_args!("打开版本更新页面失败：{error}"));
                    }
                    Err(error) => {
                        crate::log::error(format_args!("打开版本更新任务失败：{error}"));
                    }
                }
            });
        }
    });

    crate::runtime::spawn_task(async move {
        let Some(update) = check_for_update().await else {
            return;
        };
        if let Err(error) = slint::invoke_from_event_loop(move || {
            let Some(window) = weak.upgrade() else {
                return;
            };
            let home = window.global::<crate::HomeModel>();
            home.set_client_update_tooltip(format!("可更新：{}", update.new_version).into());
            home.set_client_update_url(update.release_url.into());
            home.set_client_update_visible(true);
        }) {
            crate::log::error(format_args!("投递版本更新状态失败：{error}"));
        }
    });
}

fn bind_timer(window: &MainWindow, timer: &Timer, start: Instant) {
    let weak = window.as_weak();
    timer.start(
        slint::TimerMode::Repeated,
        HOME_REFRESH_INTERVAL,
        move || {
            if let Some(window) = weak.upgrade() {
                if home_is_visible(&window) {
                    refresh_dynamic(&window, &start);
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

/// 刷新可见首页每秒变化的指标，不执行 HTTP 或全系统扫描。
pub fn refresh_dynamic(main_window: &MainWindow, start: &Instant) {
    if !home_is_visible(main_window) {
        return;
    }
    let home = main_window.global::<crate::HomeModel>();
    let sampler = metrics_sampler();
    if sampler.sampled.load(Ordering::Acquire) {
        home.set_client_mem(fmt_mb(sampler.latest_rss.load(Ordering::Acquire)).into());
    }
    let core_memory = match stream::latest_memory() {
        Ok(Some(snapshot)) => fmt_mb(snapshot.inuse),
        Ok(None) => "—".to_string(),
        Err(error) => {
            crate::log::error(format_args!("读取核心内存快照失败：{error}"));
            "—".to_string()
        }
    };
    home.set_core_mem(core_memory.into());
    home.set_uptime(format_uptime(start.elapsed()).into());
    sampler.request(main_window.as_weak());
}

/// 异步刷新核心版本、模式和代理地址，过期响应不会覆盖新会话。
fn refresh_static(weak: slint::Weak<MainWindow>, core_state: crate::event::CoreState, force: bool) {
    let request = {
        let mut state = home_static_state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.core_state != core_state {
            state.core_state = core_state;
            state.loaded_generation = None;
            state.loading_generation = None;
        }
        if !core_state.running {
            let token = next_static_request_token(&mut state);
            state.loaded_generation = None;
            state.loading_generation = None;
            Some(token)
        } else if !force
            && (state.loaded_generation == Some(core_state.generation)
                || state.loading_generation == Some(core_state.generation))
        {
            None
        } else {
            let token = next_static_request_token(&mut state);
            state.loading_generation = Some(core_state.generation);
            Some(token)
        }
    };

    let Some(token) = request else {
        return;
    };
    if !core_state.running {
        apply_static_ui(
            weak,
            core_state,
            token,
            "-".to_string(),
            "-".to_string(),
            "-".to_string(),
        );
        return;
    }

    crate::runtime::spawn_task(async move {
        let (config_result, version_result) = tokio::join!(api::get_configs(), api::get_version());
        let mut success = true;
        let mut outbound_mode = "-".to_string();
        let mut proxy_address = "-".to_string();
        let mut core_version = "-".to_string();

        match version_result {
            Ok(version) => core_version = version.version,
            Err(error) => {
                success = false;
                crate::log::error(format_args!("获取核心版本失败：{error}"));
            }
        }

        match config_result {
            Err(error) => {
                success = false;
                crate::log::error(format_args!("获取核心配置失败：{error}"));
            }
            Ok(configs) => {
                outbound_mode = outbound_mode_label(&configs.mode).to_string();
                match business::proxy_endpoint_from_configs(&configs) {
                    Err(error) => {
                        success = false;
                        crate::log::error(format_args!("解析首页代理地址失败：{error}"));
                    }
                    Ok(endpoint) => {
                        proxy_address =
                            business::proxy_address(&endpoint).unwrap_or("-".to_string())
                    }
                }
            }
        }

        let current = {
            let mut state = home_static_state()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if state.request_token != token
                || state.core_state != core_state
                || state.loading_generation != Some(core_state.generation)
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
        apply_static_ui(
            weak,
            core_state,
            token,
            core_version,
            outbound_mode,
            proxy_address,
        );
    });
}

fn apply_static_ui(
    weak: slint::Weak<MainWindow>,
    core_state: crate::event::CoreState,
    token: u64,
    core_version: String,
    outbound_mode: String,
    proxy_address: String,
) {
    if let Err(error) = slint::invoke_from_event_loop(move || {
        let current = home_static_state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if current.request_token != token || current.core_state != core_state {
            return;
        }
        drop(current);
        let Some(window) = weak.upgrade() else {
            return;
        };
        let home = window.global::<crate::HomeModel>();
        home.set_core_version(core_version.into());
        home.set_outbound_mode(outbound_mode.into());
        home.set_proxy_address(proxy_address.into());
        home.set_core_running(core_state.running);

        let proxy_status = config::proxy_status();
        home.set_system_proxy(proxy_status.system);
        home.set_tun_proxy(proxy_status.tun);

        let panel_url = if core_state.running {
            match core::get_controller_snapshot() {
                Ok(Some(snapshot)) => zashboard_url(&snapshot),
                Ok(None) => String::new(),
                Err(error) => {
                    crate::log::error(format_args!("读取核心控制端点失败：{error}"));
                    String::new()
                }
            }
        } else {
            String::new()
        };
        home.set_zashboard_url(panel_url.into());
    }) {
        crate::log::error(format_args!("投递首页静态刷新任务失败：{error}"));
    }
}

/// 页面进入时只刷新动态指标，核心静态数据由核心状态事件加载。
pub fn refresh(main_window: &MainWindow, start: &Instant) {
    refresh_dynamic(main_window, start);
}

/// 用户明确要求刷新首页核心数据时执行强制刷新。
fn refresh_explicit(main_window: &MainWindow, start: &Instant) {
    refresh_dynamic(main_window, start);
    let core_state = *crate::event::subscribe_core_state().borrow();
    refresh_static(main_window.as_weak(), core_state, true);
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
#[cfg(test)]
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
pub async fn prepare_online_panel(root: &Path) -> Result<String, String> {
    let core_state = *crate::event::subscribe_core_state().borrow();
    if !core_state.running {
        return Err("核心未运行".to_string());
    }
    let _initial_snapshot = core::get_controller_snapshot()
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "核心未运行".to_string())?;
    let panel_dir = root.join(RUNTIME_UI_DIR);
    let panel_state = crate::runtime::spawn_blocking({
        let panel_dir = panel_dir.clone();
        move || panel_directory_state(&panel_dir)
    })
    .await
    .map_err(|error| format!("检查在线面板目录任务失败：{error}"))??;
    if panel_state == PanelDirectoryState::Empty {
        api::upgrade_ui()
            .await
            .map_err(|error| format!("下载在线面板失败：{error}"))?;
        if *crate::event::subscribe_core_state().borrow() != core_state {
            return Err("核心会话已变化".to_string());
        }
        let panel_state = crate::runtime::spawn_blocking(move || panel_directory_state(&panel_dir))
            .await
            .map_err(|error| format!("检查在线面板目录任务失败：{error}"))??;
        if panel_state == PanelDirectoryState::Empty {
            return Err("在线面板下载完成但目录仍为空".to_string());
        }
    }

    if *crate::event::subscribe_core_state().borrow() != core_state {
        return Err("核心会话已变化".to_string());
    }
    let snapshot = core::get_controller_snapshot()
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "核心未运行".to_string())?;
    if *crate::event::subscribe_core_state().borrow() != core_state {
        return Err("核心会话已变化".to_string());
    }
    Ok(zashboard_url(&snapshot))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::path::PathBuf;

    fn github_release(tag_name: &str, html_url: &str) -> GithubRelease {
        GithubRelease {
            tag_name: tag_name.to_string(),
            html_url: html_url.to_string(),
        }
    }

    #[test]
    fn deserializes_minimal_github_release_response() {
        let release: GithubRelease = serde_json::from_str(
            r#"{
                "tag_name": "v1.2.3",
                "html_url": "https://github.com/vitoluo/clash-ui/releases/v1.2.3",
                "name": "ignored"
            }"#,
        )
        .expect("GitHub Release 响应应可解析");
        assert_eq!(release.tag_name, "v1.2.3");
        assert_eq!(
            release.html_url,
            "https://github.com/vitoluo/clash-ui/releases/v1.2.3"
        );
    }

    #[test]
    fn parses_release_versions_with_optional_prefix() {
        assert_eq!(
            parse_release_version("1.2.3"),
            Some(ReleaseVersion {
                major: 1,
                minor: 2,
                patch: 3,
            })
        );
        assert_eq!(
            parse_release_version("v1.2.3"),
            parse_release_version("V1.2.3")
        );
        assert_eq!(
            parse_release_version(" v1.2.3 "),
            parse_release_version("1.2.3")
        );
    }

    #[test]
    fn compares_release_versions_numerically() {
        assert!(parse_release_version("1.10.0") > parse_release_version("1.9.0"));
        assert!(parse_release_version("2.0.0") > parse_release_version("1.99.99"));
    }

    #[test]
    fn rejects_invalid_release_versions() {
        for value in ["", "1.2", "1.2.3.4", "1.x.3", "v1.2.3-beta", "1.2.-3"] {
            assert_eq!(parse_release_version(value), None, "版本应无效：{value}");
        }
    }

    #[test]
    fn accepts_only_current_repository_release_links() {
        assert!(is_valid_release_url(
            "https://github.com/vitoluo/clash-ui/releases/v1.2.3"
        ));
        assert!(!is_valid_release_url(GITHUB_RELEASE_URL_PREFIX));
        assert!(!is_valid_release_url(
            "https://github.com/other/repo/releases/v1.2.3"
        ));
        assert!(!is_valid_release_url(
            "https://github.com/vitoluo/clash-ui/releases-other/v1.2.3"
        ));
    }

    #[test]
    fn only_reports_newer_release_with_valid_link() {
        let update = version_update_from_release(
            github_release(
                " v1.10.0 ",
                " https://github.com/vitoluo/clash-ui/releases/v1.10.0 ",
            ),
            "1.9.0",
        );
        assert_eq!(
            update,
            Some(VersionUpdate {
                new_version: "v1.10.0".to_string(),
                release_url: "https://github.com/vitoluo/clash-ui/releases/v1.10.0".to_string(),
            })
        );
        assert_eq!(
            version_update_from_release(
                github_release(
                    "v1.9.0",
                    "https://github.com/vitoluo/clash-ui/releases/v1.9.0",
                ),
                "1.9.0"
            ),
            None
        );
        assert_eq!(
            version_update_from_release(
                github_release(
                    "v1.8.0",
                    "https://github.com/vitoluo/clash-ui/releases/v1.8.0",
                ),
                "1.9.0"
            ),
            None
        );
        assert_eq!(
            version_update_from_release(
                github_release("v1.10.0", "https://example.com/v1.10.0"),
                "1.9.0"
            ),
            None
        );
    }

    #[test]
    fn core_upgrade_guard_allows_only_one_active_task() {
        let active = Arc::new(AtomicBool::new(false));
        let first = CoreUpgradeGuard::try_acquire(active.clone()).expect("首次应获取更新守卫");
        assert!(CoreUpgradeGuard::try_acquire(active.clone()).is_none());

        drop(first);
        assert!(CoreUpgradeGuard::try_acquire(active).is_some());
    }

    #[test]
    fn homepage_visibility_and_refresh_interval_follow_dynamic_contract() {
        assert!(home_page_is_active(0));
        assert!(!home_page_is_active(1));
        assert_eq!(HOME_REFRESH_INTERVAL, Duration::from_secs(1));
    }

    #[test]
    fn metrics_sampler_coalesces_pending_requests() {
        let sampler = HomeMetricsSampler::new();
        assert!(sampler.try_begin());
        assert!(!sampler.try_begin());
        sampler.finish();
        assert!(sampler.try_begin());
        sampler.finish();
    }

    fn tmp_root(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("clash_ui_home_test_{name}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("创建主页测试目录失败");
        root
    }

    #[test]
    fn homepage_proxy_address_uses_api_bind_address() {
        let endpoint = business::ProxyEndpoint {
            host: "192.168.1.20".to_string(),
            ports: business::ProxyPorts {
                mixed: None,
                http: Some(7890),
                socks: Some(7891),
            },
        };
        assert_eq!(
            business::proxy_address(&endpoint),
            Some("socks5://192.168.1.20:7891".to_string())
        );
    }

    #[test]
    fn maps_outbound_mode_to_home_label() {
        assert_eq!(outbound_mode_label("rule"), "规则模式");
        assert_eq!(outbound_mode_label("global"), "全局模式");
        assert_eq!(outbound_mode_label("direct"), "直连模式");
        assert_eq!(outbound_mode_label("unknown"), "—");
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
