// 页面控制器模块：统一导出各页面的状态与后台操作接口。

pub mod config;
pub mod connections;
pub mod home;
pub mod logs;
pub mod r#override;
pub mod proxy;
pub mod rules;
pub mod settings;
pub mod source;
pub mod speed_stats;
#[allow(clippy::missing_const_for_thread_local)]
pub mod tray;
