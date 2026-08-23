use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use std::time::{Duration, Instant};

use crate::app::config;
use crate::constants::{CLASH_DIR, RUNTIME_DIR};

use super::config_merge;

const CORE_READY_TIMEOUT: Duration = Duration::from_secs(30);
const CORE_READY_RETRY_INTERVAL: Duration = Duration::from_secs(1);

// Clash 核心管理错误，携带必要上下文以便定位启动或配置生成失败。
#[derive(Debug)]
pub enum CoreError {
    Io(String, std::io::Error),
    Parse(String, Box<serde_saphyr::Error>),
    Serialize(String),
    Json(serde_json::Error),
    CoreMissing,
    Spawn(std::io::Error),
    ProcessGuard(String),
    ReadyTimeout(String),
    Lock,
}

impl std::fmt::Display for CoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(context, error) => write!(formatter, "{context}: {error}"),
            Self::Parse(context, error) => write!(formatter, "{context}: {error}"),
            Self::Serialize(error) => write!(formatter, "序列化合并配置失败: {error}"),
            Self::Json(error) => write!(formatter, "clash 设置转 JSON 失败: {error}"),
            Self::CoreMissing => write!(formatter, "未找到 clash 核心可执行文件"),
            Self::Spawn(error) => write!(formatter, "启动核心失败: {error}"),
            Self::ProcessGuard(error) => write!(formatter, "监管核心进程失败：{error}"),
            Self::ReadyTimeout(error) => {
                write!(formatter, "等待 clash 核心就绪超时：{error}")
            }
            Self::Lock => write!(formatter, "会话锁被污染"),
        }
    }
}

impl std::error::Error for CoreError {}

// 核心会话，保存动态端口、密钥和子进程句柄。
pub struct CoreSession {
    #[allow(dead_code)]
    pub port: u16,
    #[allow(dead_code)]
    pub secret: String,
    pub child: Child,
    process_guard: crate::platform::CoreProcessGuard,
    ready: bool,
}

// 控制端点快照，保证端口和密钥来自同一会话。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControllerSnapshot {
    pub port: u16,
    pub secret: String,
}

// 全局核心会话，应用进程中最多运行一个核心实例。
static SESSION: Mutex<Option<CoreSession>> = Mutex::new(None);
static STOP_HANDLER: RwLock<Option<Arc<dyn Fn() + Send + Sync>>> = RwLock::new(None);
static READY_HANDLER: RwLock<Option<Arc<dyn Fn() + Send + Sync>>> = RwLock::new(None);

/// 注册核心就绪处理器，在 version 接口检查成功后调用。
pub fn set_ready_handler(handler: impl Fn() + Send + Sync + 'static) {
    if let Ok(mut current) = READY_HANDLER.write() {
        *current = Some(Arc::new(handler));
    }
}

/// 注册核心停止处理器，由应用上下文负责清理运行时页面数据。
pub fn set_stop_handler(handler: impl Fn() + Send + Sync + 'static) {
    if let Ok(mut current) = STOP_HANDLER.write() {
        *current = Some(Arc::new(handler));
    }
}

fn notify_stop_handler() {
    let handler = STOP_HANDLER
        .read()
        .ok()
        .and_then(|current| current.as_ref().cloned());
    if let Some(handler) = handler {
        handler();
    }
}

fn notify_ready_handler() {
    let handler = READY_HANDLER
        .read()
        .ok()
        .and_then(|current| current.as_ref().cloned());
    if let Some(handler) = handler {
        handler();
    }
}

// 读取当前核心控制端点。
pub fn get_controller_snapshot() -> Option<ControllerSnapshot> {
    SESSION.lock().ok().and_then(|guard| {
        guard.as_ref().map(|session| ControllerSnapshot {
            port: session.port,
            secret: session.secret.clone(),
        })
    })
}

/// 判断当前核心是否已通过 version 接口就绪检查。
pub(crate) fn is_ready() -> bool {
    SESSION
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref().map(|session| session.ready))
        .unwrap_or(false)
}

// 读取当前核心端口。
#[allow(dead_code)]
pub fn get_port() -> Option<u16> {
    SESSION.lock().ok().and_then(|guard| {
        guard
            .as_ref()
            .filter(|session| session.ready)
            .map(|session| session.port)
    })
}

// 读取当前核心密钥。
#[allow(dead_code)]
pub fn get_secret() -> Option<String> {
    SESSION.lock().ok().and_then(|guard| {
        guard
            .as_ref()
            .filter(|session| session.ready)
            .map(|session| session.secret.clone())
    })
}

fn wait_for_core_ready_with<P>(
    timeout: Duration,
    retry_interval: Duration,
    mut probe: P,
) -> Result<(), CoreError>
where
    P: FnMut(Duration) -> Result<(), String>,
{
    let deadline = Instant::now() + timeout;
    let mut last_error = "未执行 version 接口检查".to_string();

    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(CoreError::ReadyTimeout(last_error));
        }

        match probe(remaining) {
            Ok(()) => return Ok(()),
            Err(error) => last_error = error,
        }

        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(CoreError::ReadyTimeout(last_error));
        }
        thread::sleep(retry_interval.min(remaining));
    }
}

fn wait_for_core_ready() -> Result<(), CoreError> {
    wait_for_core_ready_with(CORE_READY_TIMEOUT, CORE_READY_RETRY_INTERVAL, |timeout| {
        super::api::get_version_with_timeout(timeout)
            .map(|_| ())
            .map_err(|error| error.to_string())
    })
}

// 查找 resources/clash 下的平台核心文件。
pub fn find_core(root: &Path) -> Option<PathBuf> {
    let file_name = format!("clash{}", std::env::consts::EXE_SUFFIX);
    let path = root.join(CLASH_DIR).join(&file_name);
    if path.exists() {
        return Some(path);
    }

    let executable_root = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf))?;
    let path = executable_root.join(CLASH_DIR).join(file_name);
    path.exists().then_some(path)
}

// 生成 16 位字母数字密钥。
pub fn generate_secret() -> String {
    use rand::RngExt;

    let mut rng = rand::rng();
    (0..16)
        .map(|_| rng.sample(rand::distr::Alphanumeric) as char)
        .collect()
}

// 生成动态控制端口。
pub fn generate_port() -> u16 {
    find_free_port()
}

// 在指定范围内查找未占用端口，失败时交由系统分配。
fn find_free_port() -> u16 {
    let used = crate::platform::listening_ports();
    for port in 20000..=22000 {
        if used.contains(&port) {
            continue;
        }
        if TcpListener::bind(("127.0.0.1", port)).is_ok() {
            return port;
        }
    }

    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("系统端口分配失败");
    listener.local_addr().unwrap().port()
}

// 启动核心：先生成运行时配置，无启用配置时不启动进程。
pub fn start_core(root: &Path) -> Result<(), CoreError> {
    config_merge::merge_config(root)?;
    let cfg = config::get();
    if !cfg.configs.iter().any(|entry| entry.enabled) {
        return Ok(());
    }

    let core = find_core(root).ok_or(CoreError::CoreMissing)?;
    let port = generate_port();
    let secret = generate_secret();
    let runtime = root.join(RUNTIME_DIR);
    let mut command = std::process::Command::new(&core);
    command.args([
        "-d",
        &runtime.to_string_lossy(),
        "-ext-ctl",
        &format!("127.0.0.1:{port}"),
        "-secret",
        &secret,
    ]);

    // Windows 下 Clash 是控制台程序，禁止为子进程创建控制台窗口。
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;

        command.creation_flags(0x0800_0000);
    }

    let mut session = SESSION.lock().map_err(|_| CoreError::Lock)?;
    let mut child = command.spawn().map_err(CoreError::Spawn)?;
    let process_guard = match crate::platform::CoreProcessGuard::attach(&child) {
        Ok(guard) => guard,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(CoreError::ProcessGuard(error));
        }
    };
    *session = Some(CoreSession {
        port,
        secret,
        child,
        process_guard,
        ready: false,
    });
    drop(session);

    if let Err(error) = wait_for_core_ready() {
        stop_core();
        return Err(error);
    }
    let mut session = SESSION.lock().map_err(|_| CoreError::Lock)?;
    let current = session.as_mut().ok_or(CoreError::Lock)?;
    current.ready = true;
    drop(session);
    crate::clash::stream::start();
    notify_ready_handler();
    Ok(())
}

// 停止核心并清理会话。
pub fn stop_core() {
    crate::clash::stream::reset();
    let current = SESSION.lock().ok().and_then(|mut session| session.take());
    if let Some(mut current) = current {
        terminate_core_process(&current.process_guard, &mut current.child);
    }
    notify_stop_handler();
}

fn terminate_core_process(guard: &crate::platform::CoreProcessGuard, child: &mut Child) {
    if let Err(error) = guard.terminate() {
        crate::log::error(format_args!("终止 clash 核心进程树失败：{error}"));
    }
    let _ = child.kill();
    if let Err(error) = child.wait() {
        crate::log::error(format_args!("回收 clash 核心进程失败：{error}"));
    }
}

// 重启核心。
pub fn restart_core(root: &Path) -> Result<(), CoreError> {
    stop_core();
    start_core(root)
}

// 配置变更统一入口：重新合并配置，并按启用状态启动、重启或停止核心。
pub fn on_config_changed(root: &Path) -> Result<(), CoreError> {
    config_merge::merge_config(root)?;
    let cfg = config::get();
    if cfg.configs.iter().any(|entry| entry.enabled) {
        let running = SESSION.lock().map_err(|_| CoreError::Lock)?.is_some();
        if running {
            restart_core(root)?;
        } else {
            start_core(root)?;
        }
    } else {
        stop_core();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constants::{ASSETS_DIR, CLASH_DIR, FIXED_YAML_PATH, RUNTIME_DIR};
    use std::cell::Cell;
    use std::fs;
    use std::process::Command;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn spawn_long_running_child() -> Child {
        #[cfg(windows)]
        let child = Command::new("cmd")
            .args(["/C", "ping -n 30 127.0.0.1 >NUL"])
            .spawn();
        #[cfg(unix)]
        let child = Command::new("sh").args(["-c", "sleep 30"]).spawn();
        child.expect("启动核心停止测试进程失败")
    }

    // 创建独立临时目录，避免测试污染真实运行目录。
    fn tmp_root(name: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "clash_ui_core_test_{}_{}_{}",
            name,
            std::process::id(),
            name.len()
        ));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).expect("创建测试目录失败");
        directory
    }

    #[test]
    fn secret_has_expected_length() {
        assert_eq!(generate_secret().len(), 16);
    }

    #[test]
    fn readiness_probe_retries_until_success() {
        let attempts = Cell::new(0);

        wait_for_core_ready_with(Duration::from_secs(1), Duration::ZERO, |_| {
            let current = attempts.get() + 1;
            attempts.set(current);
            if current < 3 {
                Err(format!("第 {current} 次检查失败"))
            } else {
                Ok(())
            }
        })
        .unwrap();

        assert_eq!(attempts.get(), 3);
        assert_eq!(CORE_READY_RETRY_INTERVAL, Duration::from_secs(1));
        assert_eq!(CORE_READY_TIMEOUT, Duration::from_secs(30));
    }

    #[test]
    fn readiness_probe_returns_last_error_after_timeout() {
        let error =
            wait_for_core_ready_with(Duration::from_millis(20), Duration::from_millis(1), |_| {
                Err("核心尚未监听".to_string())
            })
            .unwrap_err();

        assert!(matches!(
            error,
            CoreError::ReadyTimeout(message) if message == "核心尚未监听"
        ));
    }

    #[test]
    fn lifecycle_handlers_receive_ready_and_stop_events() {
        let ready_calls = Arc::new(AtomicUsize::new(0));
        let stop_calls = Arc::new(AtomicUsize::new(0));
        set_ready_handler({
            let ready_calls = ready_calls.clone();
            move || {
                ready_calls.fetch_add(1, Ordering::Relaxed);
            }
        });
        set_stop_handler({
            let stop_calls = stop_calls.clone();
            move || {
                stop_calls.fetch_add(1, Ordering::Relaxed);
            }
        });

        notify_ready_handler();
        notify_stop_handler();

        assert_eq!(ready_calls.load(Ordering::Relaxed), 1);
        assert_eq!(stop_calls.load(Ordering::Relaxed), 1);
        set_ready_handler(|| {});
        set_stop_handler(|| {});
    }

    #[test]
    fn free_port_is_in_range_and_bindable() {
        let port = find_free_port();
        assert!((20000..=22000).contains(&port), "端口 {port} 超出范围");
        let _listener = TcpListener::bind(("127.0.0.1", port)).expect("端口应当可绑定");
    }

    #[test]
    fn missing_core_returns_none() {
        let root = tmp_root("findcore");
        fs::create_dir_all(root.join(CLASH_DIR)).unwrap();
        assert!(find_core(&root).is_none());
    }

    #[test]
    fn no_enabled_config_does_not_start_core() {
        let root = tmp_root("nostart");
        fs::create_dir_all(root.join(ASSETS_DIR)).unwrap();
        fs::create_dir_all(root.join(RUNTIME_DIR)).unwrap();
        fs::write(root.join(FIXED_YAML_PATH), "external-ui: ui\n").unwrap();
        config::init(&root);
        assert!(!config::get().configs.iter().any(|entry| entry.enabled));
        assert!(start_core(&root).is_ok());
        assert!(SESSION.lock().unwrap().is_none());
    }

    #[test]
    fn terminates_and_reaps_running_child() {
        let mut child = spawn_long_running_child();
        let guard =
            crate::platform::CoreProcessGuard::attach(&child).expect("创建测试进程守卫失败");

        terminate_core_process(&guard, &mut child);

        assert!(child.try_wait().unwrap().is_some());
    }

    #[test]
    fn reaps_child_that_has_already_exited() {
        let mut child = spawn_long_running_child();
        let guard =
            crate::platform::CoreProcessGuard::attach(&child).expect("创建测试进程守卫失败");
        child.kill().expect("终止测试进程失败");
        child.wait().expect("首次回收测试进程失败");

        terminate_core_process(&guard, &mut child);

        assert!(child.try_wait().unwrap().is_some());
    }
}
