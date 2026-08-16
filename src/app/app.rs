// 应用入口：准备运行目录和配置，并驱动主窗口生命周期。

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use super::{app_context::AppContext, config};
use crate::constants::{
    ASSETS_DIR, CLASH_DIR, CONFIGS_DIR, FIXED_YAML, FIXED_YAML_PATH, OVERRIDES_DIR, RUNTIME_DIR,
    RUNTIME_UI_DIR,
};
use crate::platform;

const GEO_DATA_FILES: &[&str] = &["geoip.dat", "geosite.dat", "country.mmdb", "asn.mmdb"];

/// 返回可执行文件所在目录（运行时资源根目录）。
fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .expect("无法获取可执行文件路径")
        .parent()
        .expect("可执行文件路径无父目录")
        .to_path_buf()
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn user_data_root(home: &Path) -> PathBuf {
    home.join(".config").join("clash-ui")
}

#[cfg(target_os = "windows")]
fn data_root(exe_root: &Path) -> Result<PathBuf, std::io::Error> {
    Ok(exe_root.to_path_buf())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn data_root(_exe_root: &Path) -> Result<PathBuf, std::io::Error> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "无法获取当前用户目录"))?;
    Ok(user_data_root(&home))
}

#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
fn data_root(exe_root: &Path) -> Result<PathBuf, std::io::Error> {
    Ok(exe_root.to_path_buf())
}

/// 确保数据目录结构存在（忽略已存在的情况）。
fn ensure_data_dirs(root: &Path) {
    let dirs = [
        root.join(CONFIGS_DIR),
        root.join(OVERRIDES_DIR),
        root.join(RUNTIME_DIR),
        root.join(RUNTIME_UI_DIR),
    ];
    for directory in dirs {
        if let Err(error) = fs::create_dir_all(&directory) {
            eprintln!("创建目录失败 {}: {}", directory.display(), error);
        }
    }
}

/// 确保随程序分发的资源目录存在（忽略已存在的情况）。
fn ensure_resource_dirs(root: &Path) {
    for directory in [root.join(ASSETS_DIR), root.join(CLASH_DIR)] {
        if let Err(error) = fs::create_dir_all(&directory) {
            eprintln!("创建目录失败 {}: {}", directory.display(), error);
        }
    }
}

/// 将资源目录中缺少的地理数据库复制到运行时目录。
fn copy_missing_geo_data(resource_root: &Path, data_root: &Path) {
    let source_dir = resource_root.join(CLASH_DIR);
    let runtime_dir = data_root.join(RUNTIME_DIR);
    for file_name in GEO_DATA_FILES {
        let destination = runtime_dir.join(file_name);
        if destination.exists() {
            continue;
        }

        let source = source_dir.join(file_name);
        if let Err(error) = fs::copy(&source, &destination) {
            eprintln!(
                "复制地理数据库失败 {} -> {}: {}",
                source.display(),
                destination.display(),
                error
            );
        }
    }
}

/// 若文件不存在则写入默认内容。
fn write_if_missing(path: &Path, content: &str) {
    if !path.exists() {
        if let Err(error) = fs::write(path, content) {
            eprintln!("写入文件失败 {}: {}", path.display(), error);
        }
    }
}

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let resource_root = exe_dir();
    let data_root = data_root(&resource_root)?;
    let start = Instant::now();
    ensure_resource_dirs(&resource_root);
    ensure_data_dirs(&data_root);
    copy_missing_geo_data(&resource_root, &data_root);
    write_if_missing(&resource_root.join(FIXED_YAML_PATH), FIXED_YAML);
    config::init(&data_root);

    let needs_elevation = config::get().proxy_status.tun && !platform::is_admin();
    if needs_elevation {
        platform::request_elevation();
        return Ok(());
    }

    let context = AppContext::new(data_root, start)?;
    context.bind_callbacks();
    context.start_services();
    context.show_and_run()
}

#[cfg(test)]
mod tests {
    use super::*;

    // 创建独立临时目录，避免测试污染真实运行目录。
    fn tmp_root(name: &str) -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("clash_ui_app_test_{}_{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).expect("创建测试目录失败");
        directory
    }

    #[test]
    fn copy_missing_geo_data_only_copies_missing_files() {
        let root = tmp_root("geo_data");
        fs::create_dir_all(root.join(CLASH_DIR)).unwrap();
        fs::create_dir_all(root.join(RUNTIME_DIR)).unwrap();

        for &file_name in GEO_DATA_FILES {
            fs::write(
                root.join(CLASH_DIR).join(file_name),
                format!("source-{file_name}"),
            )
            .unwrap();
        }
        fs::write(root.join(RUNTIME_DIR).join("geoip.dat"), "existing").unwrap();

        copy_missing_geo_data(&root, &root);

        assert_eq!(
            fs::read_to_string(root.join(RUNTIME_DIR).join("geoip.dat")).unwrap(),
            "existing"
        );
        for &file_name in GEO_DATA_FILES.iter().skip(1) {
            assert_eq!(
                fs::read_to_string(root.join(RUNTIME_DIR).join(file_name)).unwrap(),
                format!("source-{file_name}")
            );
        }
    }
}
