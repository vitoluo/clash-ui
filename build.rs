// 构建脚本：编译 Slint UI、注册 lucide 图标库、嵌入 Windows 可执行文件图标。

fn main() {
    // 任意 .slint / 资源变化都需重新编译 UI（cargo 默认不跟踪这些文件）。
    println!("cargo:rerun-if-changed=ui");
    println!("cargo:rerun-if-changed=assets");

    // 注册 @lucide 图标库路径，供 .slint 文件 import 使用。
    let library_paths = std::collections::HashMap::from([(
        "lucide".to_string(),
        std::path::PathBuf::from(lucide_slint::lib()),
    )]);
    // 通过同名组件覆盖 Slint 官方实现
    let config = slint_build::CompilerConfiguration::new()
        .with_include_paths(vec![std::path::PathBuf::from("ui/overlay")])
        .with_library_paths(library_paths);
    slint_build::compile_with_config("ui/main.slint", config).expect("Slint UI 编译失败");

    // Windows 平台：将应用图标嵌入可执行文件。
    #[cfg(windows)]
    {
        winres::WindowsResource::new()
            .set_icon("assets/app.ico")
            .set("ProductName", "clash-ui")
            .set("FileDescription", "Clash 图形客户端")
            .compile()
            .expect("Windows 资源编译失败");
    }
}
