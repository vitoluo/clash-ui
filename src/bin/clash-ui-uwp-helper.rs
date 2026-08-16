// 独立 Windows UWP helper 入口；非 Windows 目标仅保留可编译占位路径。

#[cfg(target_os = "windows")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    clash_ui::platform::run_uwp_helper().map_err(std::io::Error::other)?;
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    Err("UWP helper 仅支持 Windows".into())
}
