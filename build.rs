// 构建脚本：编译 Slint UI、注册 lucide 图标库。

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
}
