#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

// 应用 crate 根：声明跨页面服务、页面控制器并导入 Slint 生成类型。

mod app;
mod clash;
mod consts;
mod controller;
mod event;
mod network;
mod runtime;

use clash_ui::{log, platform};

slint::include_modules!();

fn main() -> Result<(), Box<dyn std::error::Error>> {
    Ok(app::run()?)
}
