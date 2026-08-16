// Windows 平台能力入口；具体实现按职责拆分到子模块。

mod network;
mod uwp;
mod uwp_api;
mod uwp_elevation;

#[cfg(test)]
mod tests;

pub use network::normalize_tun_device;
pub(super) use network::proxy_bypass_string;
#[allow(unused_imports)]
pub use uwp::{list_uwp_apps, set_uwp_loopback, set_uwp_loopback_batch, supports_uwp};
pub use uwp_elevation::run_uwp_helper;
