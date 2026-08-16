// 应用配置读写层：app.yaml 的类型安全内存模型与持久化。
//
// 作为全局配置的唯一来源，供核心管理与 UI 读写。
// 仅使用 serde + serde-saphyr：加载用 from_str，保存用 to_string。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};

use serde::{Deserialize, Serialize};

use crate::constants::{
    APP_CONFIG_PATH, DEFAULT_PROXY_BYPASS_LIST, DEFAULT_TUN_ROUTE_EXCLUDE_ADDRESS,
};

/// 配置来源类型（file / http）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SourceType {
    #[default]
    File,
    Http,
}

/// 日志等级（silent 默认）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    #[default]
    Silent,
    Error,
    Warning,
    Info,
    Debug,
}

/// 主题（system 默认）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    #[default]
    System,
    Light,
    Dark,
}

/// TUN 协议栈（gvisor 默认）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum TunStack {
    System,
    #[default]
    Gvisor,
    Mixed,
}

/// 代理开启状态。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProxyStatus {
    #[serde(default)]
    pub system: bool,
    #[serde(default)]
    pub tun: bool,
}

/// 单条配置项。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ConfigEntry {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default, rename = "source-type")]
    pub source_type: SourceType,
    #[serde(default, rename = "source-uri")]
    pub source_uri: String,
    /// 导入内容在磁盘的实际路径（如 data/configs/config.yaml），合并时按此读取。
    #[serde(default, rename = "path")]
    pub path: String,
}

/// 单条覆写项。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OverrideEntry {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default, rename = "source-type")]
    pub source_type: SourceType,
    #[serde(default, rename = "source-uri")]
    pub source_uri: String,
    #[serde(default)]
    pub sort: i64,
    /// 导入内容在磁盘的实际路径（如 data/overrides/xxx.yaml），合并时按此读取。
    #[serde(default, rename = "path")]
    pub path: String,
}

/// 应用设置（开机自启、主题、静默启动）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppSettings {
    #[serde(default)]
    pub auto_start: bool,
    #[serde(default)]
    pub theme: ThemeMode,
    #[serde(default)]
    pub silent_start: bool,
}

/// 代理设置（跳过代理列表）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ProxySettings {
    #[serde(rename = "bypass-list")]
    pub bypass_list: Vec<String>,
}

pub(crate) fn default_proxy_bypass_list() -> Vec<String> {
    DEFAULT_PROXY_BYPASS_LIST
        .iter()
        .copied()
        .map(String::from)
        .collect()
}

impl Default for ProxySettings {
    fn default() -> Self {
        Self {
            bypass_list: default_proxy_bypass_list(),
        }
    }
}

/// TUN 段配置。默认值遵循 docs.md。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TunConfig {
    pub stack: TunStack,
    pub device: String,
    pub mtu: u32,
    #[serde(rename = "strict-route")]
    pub strict_route: bool,
    #[serde(rename = "auto-detect-interface")]
    pub auto_detect_interface: bool,
    #[serde(rename = "auto-route")]
    pub auto_route: bool,
    #[serde(rename = "route-exclude-address")]
    pub route_exclude_address: Vec<String>,
    #[serde(rename = "exclude-interface")]
    pub exclude_interface: Vec<String>,
}

pub(crate) fn default_route_exclude_address() -> Vec<String> {
    DEFAULT_TUN_ROUTE_EXCLUDE_ADDRESS
        .iter()
        .copied()
        .map(String::from)
        .collect()
}

impl Default for TunConfig {
    fn default() -> Self {
        Self {
            stack: TunStack::default(),
            device: "clash".to_string(),
            mtu: 9000,
            strict_route: true,
            auto_detect_interface: true,
            auto_route: true,
            route_exclude_address: default_route_exclude_address(),
            exclude_interface: Vec::new(),
        }
    }
}

/// clash 设置（日志等级、IPv6、局域网、端口、TUN 等）。默认值遵循 docs.md。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ClashSettings {
    #[serde(rename = "log-level")]
    pub log_level: LogLevel,
    pub ipv6: bool,
    #[serde(rename = "allow-lan")]
    pub allow_lan: bool,
    #[serde(rename = "bind-address")]
    pub bind_address: String,
    #[serde(rename = "mixed-port")]
    pub mixed_port: Option<u16>,
    pub port: Option<u16>,
    #[serde(rename = "socks-port")]
    pub socks_port: Option<u16>,
    pub tun: TunConfig,
}

impl Default for ClashSettings {
    fn default() -> Self {
        Self {
            log_level: LogLevel::default(),
            ipv6: true,
            allow_lan: false,
            bind_address: "127.0.0.1".to_string(),
            mixed_port: None,
            port: Some(7890),
            socks_port: None,
            tun: TunConfig::default(),
        }
    }
}

/// 设置三段：应用 / 代理 / clash。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Settings {
    #[serde(default)]
    pub app: AppSettings,
    #[serde(default)]
    pub proxy: ProxySettings,
    #[serde(default)]
    pub clash: ClashSettings,
}

/// 顶层应用配置（app.yaml）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppConfig {
    #[serde(default, rename = "proxy-status")]
    pub proxy_status: ProxyStatus,
    #[serde(default)]
    pub configs: Vec<ConfigEntry>,
    #[serde(default)]
    pub overrides: Vec<OverrideEntry>,
    #[serde(default)]
    pub settings: Settings,
}

/// 全局状态：内存中的配置快照与运行时根目录（用于落盘路径）。
/// 字段为模块内部读写接口预留，后续任务（核心管理、UI）消费，先标 allow。
#[allow(dead_code)]
struct ConfigState {
    config: AppConfig,
    root: PathBuf,
}

static STATE: OnceLock<RwLock<ConfigState>> = OnceLock::new();

/// 从磁盘加载配置：文件缺失或损坏时生成默认结构并落盘。
fn load(root: &Path) -> AppConfig {
    let path = root.join(APP_CONFIG_PATH);
    if path.exists() {
        match fs::read_to_string(&path) {
            Ok(text) => match serde_saphyr::from_str(&text) {
                Ok(cfg) => return cfg,
                Err(e) => eprintln!("解析 app.yaml 失败，使用默认配置: {e}"),
            },
            Err(e) => eprintln!("读取 app.yaml 失败，使用默认配置: {e}"),
        }
    }
    let cfg = AppConfig::default();
    match serde_saphyr::to_string(&cfg) {
        Ok(text) => {
            if let Err(e) = fs::write(&path, text) {
                eprintln!("写入默认 app.yaml 失败: {e}");
            }
        }
        Err(e) => eprintln!("序列化默认 app.yaml 失败: {e}"),
    }
    cfg
}

/// 初始化全局配置：目录兜底由调用方完成，此处加载/生成并写入 OnceLock。
pub fn init(root: &Path) {
    let config = load(root);
    let state = ConfigState {
        config,
        root: root.to_path_buf(),
    };
    if STATE.set(RwLock::new(state)).is_err() {
        eprintln!("配置已初始化，忽略重复 init");
    }
}

/// 读取当前配置快照（线程安全拷贝）。
#[allow(dead_code)]
pub fn get() -> AppConfig {
    STATE
        .get()
        .expect("配置未初始化，请先调用 config::init")
        .read()
        .expect("配置读锁被污染")
        .config
        .clone()
}

/// 修改配置并写回磁盘。
#[allow(dead_code)]
pub fn update<F: FnOnce(&mut AppConfig)>(f: F) {
    let state = STATE.get().expect("配置未初始化，请先调用 config::init");
    let mut guard = state.write().expect("配置写锁被污染");
    f(&mut guard.config);
    let path = guard.root.join(APP_CONFIG_PATH);
    match serde_saphyr::to_string(&guard.config) {
        Ok(text) => {
            if let Err(e) = fs::write(&path, text) {
                eprintln!("保存 app.yaml 失败: {e}");
            }
        }
        Err(e) => eprintln!("序列化 app.yaml 失败: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// 创建独立临时根目录，避免污染真实运行时与并行测试互相干扰。
    fn tmp_root(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("clash_ui_cfg_test_{}_{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("data")).expect("创建测试目录失败");
        dir
    }

    #[test]
    fn creates_default_file_when_missing() {
        let root = tmp_root("missing");
        let cfg = load(&root);
        let path = root.join(APP_CONFIG_PATH);
        assert!(path.exists(), "缺失时应落盘默认 app.yaml");

        // 默认值符合 docs.md 要求。
        assert_eq!(cfg.settings.clash.log_level, LogLevel::Silent);
        assert!(cfg.settings.clash.ipv6);
        assert!(!cfg.settings.clash.allow_lan);
        assert_eq!(cfg.settings.clash.bind_address, "127.0.0.1");
        assert_eq!(cfg.settings.clash.tun.stack, TunStack::Gvisor);
        assert_eq!(cfg.settings.clash.tun.device, "clash");
        assert_eq!(cfg.settings.clash.tun.mtu, 9000);
        assert!(cfg.settings.clash.tun.strict_route);
        assert!(cfg.settings.clash.tun.auto_detect_interface);
        assert!(cfg.settings.clash.tun.auto_route);
        assert_eq!(
            cfg.settings.clash.tun.route_exclude_address,
            DEFAULT_TUN_ROUTE_EXCLUDE_ADDRESS
                .iter()
                .map(|value| (*value).to_string())
                .collect::<Vec<_>>()
        );
        assert_eq!(
            cfg.settings.proxy.bypass_list,
            DEFAULT_PROXY_BYPASS_LIST
                .iter()
                .map(|value| (*value).to_string())
                .collect::<Vec<_>>()
        );
        assert!(cfg.settings.clash.mixed_port.is_none());
        assert!(!cfg.settings.app.silent_start);
    }

    #[test]
    fn persists_memory_updates_across_reload() {
        let root = tmp_root("persist");
        init(&root);

        update(|c| {
            c.proxy_status.system = true;
            c.settings.clash.allow_lan = true;
            c.settings.clash.tun.mtu = 1400;
            c.settings.app.silent_start = true;
        });

        // 重新加载，验证落盘生效。
        let reloaded = load(&root);
        assert!(reloaded.proxy_status.system);
        assert!(reloaded.settings.clash.allow_lan);
        assert_eq!(reloaded.settings.clash.tun.mtu, 1400);
        assert!(reloaded.settings.app.silent_start);
    }

    #[test]
    fn loads_existing_file_with_defaults_for_missing_fields() {
        let root = tmp_root("partial");
        let partial = r#"proxy-status:
  system: true
settings:
  proxy:
    uwp: []
  clash:
    log-level: debug
"#;
        fs::write(root.join(APP_CONFIG_PATH), partial).unwrap();

        let cfg = load(&root);
        assert!(cfg.proxy_status.system);
        assert_eq!(cfg.settings.clash.log_level, LogLevel::Debug);
        // 缺失字段取默认值。
        assert!(cfg.settings.clash.ipv6);
        assert_eq!(cfg.settings.clash.bind_address, "127.0.0.1");
        assert_eq!(cfg.settings.proxy.bypass_list, default_proxy_bypass_list());
        let serialized = serde_saphyr::to_string(&cfg).unwrap();
        assert!(!serialized.contains("uwp"));
        assert_eq!(
            cfg.settings.clash.tun.route_exclude_address,
            default_route_exclude_address()
        );
        assert!(!cfg.settings.app.silent_start);
    }

    #[test]
    fn preserves_explicit_empty_network_lists() {
        let root = tmp_root("empty_lists");
        let partial = r#"settings:
  proxy:
    bypass-list: []
  clash:
    tun:
      route-exclude-address: []
"#;
        fs::write(root.join(APP_CONFIG_PATH), partial).unwrap();

        let cfg = load(&root);
        assert!(cfg.settings.proxy.bypass_list.is_empty());
        assert!(cfg.settings.clash.tun.route_exclude_address.is_empty());
    }
}
