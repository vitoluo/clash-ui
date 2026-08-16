// 应用模块入口，集中声明应用启动、回调、上下文和配置子模块。
#![allow(clippy::module_inception)]

mod app;
mod app_bindings;
mod app_context;
pub(crate) mod config;

pub(crate) use app::run;
