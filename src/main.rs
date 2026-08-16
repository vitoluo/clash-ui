// 应用 crate 根：声明跨页面服务、页面控制器并导入 Slint 生成类型。

mod app;
pub(crate) mod clash;
#[path = "const.rs"]
mod constants;
mod controller;

pub use clash_ui::platform;

slint::include_modules!();

fn main() -> Result<(), Box<dyn std::error::Error>> {
    app::run()
}
