// 平台抽象层：统一封装跨平台能力，平台文件仅保留平台专属功能。
use std::collections::HashSet;
use std::path::PathBuf;

use display_info::DisplayInfo;
use sysinfo::System;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "linux")]
pub use linux::*;
#[cfg(target_os = "macos")]
pub use macos::*;
#[cfg(target_os = "windows")]
pub use windows::*;

/// UWP 应用的最小展示信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UwpApp {
    pub name: String,
    pub package_family_name: String,
    pub enabled: bool,
}

/// 平台能力错误，保留操作上下文并交由上层决定降级策略。
#[derive(Debug)]
pub enum PlatformError {
    Io {
        operation: String,
        source: std::io::Error,
    },
    Operation {
        operation: String,
        message: String,
    },
    InvalidInput(String),
    Unsupported(String),
}

impl PlatformError {
    pub(crate) fn io(operation: impl Into<String>, source: std::io::Error) -> Self {
        Self::Io {
            operation: operation.into(),
            source,
        }
    }

    pub(crate) fn operation(operation: impl Into<String>, message: impl std::fmt::Display) -> Self {
        Self::Operation {
            operation: operation.into(),
            message: message.to_string(),
        }
    }

    pub(crate) fn invalid_input(message: impl Into<String>) -> Self {
        Self::InvalidInput(message.into())
    }

    #[allow(dead_code)]
    pub(crate) fn unsupported(message: impl Into<String>) -> Self {
        Self::Unsupported(message.into())
    }
}

impl std::fmt::Display for PlatformError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io { operation, source } => write!(formatter, "{operation}：{source}"),
            Self::Operation { operation, message } => write!(formatter, "{operation}：{message}"),
            Self::InvalidInput(message) => write!(formatter, "输入无效：{message}"),
            Self::Unsupported(message) => write!(formatter, "平台不支持：{message}"),
        }
    }
}

impl std::error::Error for PlatformError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// 返回当前系统名（含架构）。
pub fn platform_name() -> String {
    let name = System::name().unwrap_or_else(|| "Unknown".into());
    let version = System::os_version().unwrap_or_else(|| "Unknown".into());
    let arch = System::cpu_arch();

    format!("{} {} ({})", name, version, arch)
}

/// 使用默认浏览器打开 URL。
pub fn open_url(url: &str) -> Result<(), PlatformError> {
    webbrowser::open(url)
        .map(|_| ())
        .map_err(|error| PlatformError::operation(format!("打开 URL 失败 {url}"), error))
}

/// 使用原生文件选择器选择 YAML 配置文件。
pub fn pick_config_file() -> Result<Option<PathBuf>, PlatformError> {
    Ok(rfd::FileDialog::new()
        .set_title("选择 Clash 配置文件")
        .add_filter("YAML 配置文件", &["yaml", "yml"])
        .pick_file())
}

/// 枚举当前已监听的 TCP/UDP 端口集合。
pub fn listening_ports() -> Result<HashSet<u16>, PlatformError> {
    listeners::get_all()
        .map(|items| {
            items
                .into_iter()
                .filter(|item| {
                    item.socket.port() != 0
                        && matches!(
                            item.state,
                            listeners::SocketState::Listen | listeners::SocketState::Unknown
                        )
                })
                .map(|item| item.socket.port())
                .collect()
        })
        .map_err(|error| PlatformError::operation("枚举监听端口失败", error))
}

/// 读取系统深色模式。
pub fn is_dark_mode() -> Result<bool, PlatformError> {
    dark_light::detect()
        .map(|mode| matches!(mode, dark_light::Mode::Dark))
        .map_err(|error| PlatformError::operation("检测系统主题失败", error))
}

/// 返回主显示器逻辑尺寸，读取失败时由调用方处理。
pub fn get_primary_screen_size() -> Result<(f32, f32), PlatformError> {
    let displays =
        DisplayInfo::all().map_err(|error| PlatformError::operation("枚举显示器失败", error))?;
    let info = displays
        .iter()
        .find(|display| display.is_primary)
        .or_else(|| displays.first())
        .ok_or_else(|| PlatformError::operation("读取主显示器失败", "未找到可用显示器"))?;
    let scale = if info.scale_factor > 0.0 {
        info.scale_factor
    } else {
        1.0
    };
    Ok((info.width as f32 / scale, info.height as f32 / scale))
}

/// 使用当前用户自启动配置启动或停止应用。
pub fn set_auto_start(enabled: bool) -> Result<(), PlatformError> {
    let exe = std::env::current_exe()
        .map_err(|error| PlatformError::io("获取当前程序路径失败", error))?;
    #[cfg(target_os = "windows")]
    let app_name = "Clash UI";
    #[cfg(target_os = "linux")]
    let app_name = "clash-ui";
    #[cfg(target_os = "macos")]
    let app_name = "com.vito.clash-ui";

    let mut builder = auto_launch::AutoLaunchBuilder::new();
    builder
        .set_app_name(app_name)
        .set_app_path(&exe.to_string_lossy());
    #[cfg(target_os = "windows")]
    builder.set_windows_enable_mode(auto_launch::WindowsEnableMode::CurrentUser);
    #[cfg(target_os = "linux")]
    builder.set_linux_launch_mode(auto_launch::LinuxLaunchMode::XdgAutostart);
    #[cfg(target_os = "macos")]
    builder.set_macos_launch_mode(auto_launch::MacOSLaunchMode::LaunchAgent);

    let auto = builder
        .build()
        .map_err(|error| PlatformError::operation("创建自启动配置失败", error))?;
    if enabled {
        auto.enable()
            .map_err(|error| PlatformError::operation("启用自启动失败", error))
    } else {
        auto.disable()
            .map_err(|error| PlatformError::operation("停用自启动失败", error))
    }
}

/// 检测当前进程是否已获得管理员权限。
pub fn is_admin() -> bool {
    elevated_command::Command::is_elevated()
}

/// 以管理员权限重新启动当前应用。
pub fn request_elevation() -> Result<(), PlatformError> {
    let exe = std::env::current_exe()
        .map_err(|error| PlatformError::io("获取当前程序路径失败", error))?;
    let mut command = std::process::Command::new(exe);
    command.args(std::env::args().skip(1));
    let output = elevated_command::Command::new(command)
        .output()
        .map_err(|error| PlatformError::operation("请求管理员权限失败", error))?;
    #[cfg(target_os = "windows")]
    let success = output.status.code().is_some_and(|code| code > 32);
    #[cfg(not(target_os = "windows"))]
    let success = output.status.success();
    if success {
        Ok(())
    } else {
        Err(PlatformError::operation(
            "请求管理员权限失败",
            format!("进程退出状态 {:?}", output.status.code()),
        ))
    }
}

/// 使用 sysproxy 统一设置系统代理。
pub fn set_system_proxy(
    host: &str,
    enabled: bool,
    http_port: Option<u16>,
    socks_port: Option<u16>,
    bypass: &[String],
) -> Result<(), PlatformError> {
    let port = http_port.or(socks_port).unwrap_or(0);
    if enabled && port == 0 {
        return Err(PlatformError::invalid_input(
            "没有可用的 HTTP 或 SOCKS 代理端口",
        ));
    }
    let bypass = {
        #[cfg(target_os = "linux")]
        {
            linux::proxy_bypass_string(bypass)
        }
        #[cfg(target_os = "macos")]
        {
            macos::proxy_bypass_string(bypass)
        }
        #[cfg(target_os = "windows")]
        {
            windows::proxy_bypass_string(bypass)
        }
    };

    sysproxy::Sysproxy {
        enable: enabled,
        host: host.to_string(),
        port,
        bypass,
    }
    .set_system_proxy()
    .map_err(|error| PlatformError::operation("设置系统代理失败", error))
}

/// 使用 sysproxy 更新当前系统代理的绕过列表。
pub fn set_proxy_bypass(bypass: &[String]) -> Result<(), PlatformError> {
    let mut proxy = sysproxy::Sysproxy::get_system_proxy()
        .map_err(|error| PlatformError::operation("读取系统代理失败", error))?;
    if !proxy.enable {
        return Ok(());
    }
    proxy.bypass = {
        #[cfg(target_os = "linux")]
        {
            linux::proxy_bypass_string(bypass)
        }
        #[cfg(target_os = "macos")]
        {
            macos::proxy_bypass_string(bypass)
        }
        #[cfg(target_os = "windows")]
        {
            windows::proxy_bypass_string(bypass)
        }
    };
    proxy
        .set_system_proxy()
        .map_err(|error| PlatformError::operation("更新代理绕过列表失败", error))
}

/// 跨平台写入剪贴板文本。
pub fn set_clipboard_text(text: &str) -> Result<(), PlatformError> {
    let mut clipboard = arboard::Clipboard::new()
        .map_err(|error| PlatformError::operation("打开剪贴板失败", error))?;
    clipboard
        .set_text(text.to_string())
        .map_err(|error| PlatformError::operation("写入剪贴板失败", error))
}
