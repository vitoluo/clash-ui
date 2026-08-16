// 全局编译期常量。
pub(crate) const ASSETS_DIR: &str = "resources";
pub(crate) const FIXED_YAML_PATH: &str = "resources/fixed.yaml";
pub(crate) const CLASH_DIR: &str = "resources/clash";

pub(crate) const APP_CONFIG_PATH: &str = "data/app.yaml";
pub(crate) const CONFIGS_DIR: &str = "data/configs";
pub(crate) const OVERRIDES_DIR: &str = "data/overrides";
pub(crate) const RUNTIME_DIR: &str = "data/runtime";
pub(crate) const RUNTIME_UI_DIR: &str = "data/runtime/ui";

// 设置页排除网段默认值。
pub(crate) const DEFAULT_TUN_ROUTE_EXCLUDE_ADDRESS: &[&str] = &[
    "127.0.0.0/8",
    "10.0.0.0/8",
    "172.16.0.0/12",
    "192.168.0.0/16",
    "::1/128",
    "fc00::/7",
];

// 设置页跳过代理默认值，分隔符由平台转换层负责处理。
#[cfg(target_os = "windows")]
pub(crate) const DEFAULT_PROXY_BYPASS_LIST: &[&str] = &[
    "localhost",
    "127.*",
    "192.168.*",
    "10.*",
    "172.16.*",
    "172.17.*",
    "172.18.*",
    "172.19.*",
    "172.20.*",
    "172.21.*",
    "172.22.*",
    "172.23.*",
    "172.24.*",
    "172.25.*",
    "172.26.*",
    "172.27.*",
    "172.28.*",
    "172.29.*",
    "172.30.*",
    "172.31.*",
    "<local>",
];

#[cfg(target_os = "linux")]
pub(crate) const DEFAULT_PROXY_BYPASS_LIST: &[&str] = &[
    "localhost",
    "127.0.0.1",
    "192.168.0.0/16",
    "10.0.0.0/8",
    "172.16.0.0/12",
    "::1",
];

#[cfg(target_os = "macos")]
pub(crate) const DEFAULT_PROXY_BYPASS_LIST: &[&str] = &[
    "127.0.0.1",
    "192.168.0.0/16",
    "10.0.0.0/8",
    "172.16.0.0/12",
    "localhost",
    "*.local",
    "*.crashlytics.com",
    "<local>",
];

#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
pub(crate) const DEFAULT_PROXY_BYPASS_LIST: &[&str] = &["localhost", "127.0.0.1"];

pub(crate) const FIXED_YAML: &str = r#"external-ui: ui
external-ui-url: https://github.com/Zephyruso/zashboard/releases/latest/download/dist-no-fonts.zip # webui 下载地址
profile:
  store-selected: true
  store-fake-ip: true
unified-delay: true
tcp-concurrent: false
geodata-mode: false
geodata-loader: standard
geo-auto-update: true
geo-update-interval: 24
"#;

pub(crate) const MAX_LOG_RECORDS: usize = 1000;

pub(crate) const DEFAULT_TEST_URL: &str = "https://www.gstatic.com/generate_204";
pub(crate) const TEST_TIMEOUT_MS: u32 = 5000;

pub(crate) const TRAY_ICONS: [&str; 4] = [
    // 未设置代理
    include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/assets/icons/gray.svg"
    )),
    // 系统代理
    include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/assets/icons/green.svg"
    )),
    // TUN 代理
    include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/assets/icons/blue.svg"
    )),
    // 系统代理 + TUN 代理
    include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/assets/icons/white.svg"
    )),
];
