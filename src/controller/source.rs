// 页面共享源处理：读取、校验、名称回退与内部副本路径安全。

use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use crate::app::config::SourceType;
use crate::clash::api;

/// 将表单中的来源类型转换为领域模型。
pub fn parse_source_type(value: &str) -> Result<SourceType, String> {
    match value {
        "http" => Ok(SourceType::Http),
        "file" => Ok(SourceType::File),
        _ => Err("配置类型必须是 http 或 file".to_string()),
    }
}

/// 校验当前页面列表中的 source-uri 唯一性；编辑当前项时排除自身。
pub fn ensure_unique_source_uri<'a, I>(
    entries: I,
    editing_path: &str,
    source_uri: &str,
    item_label: &str,
) -> Result<(), String>
where
    I: IntoIterator<Item = (&'a str, &'a str)>,
{
    let source_uri = source_uri.trim();
    let duplicated = entries.into_iter().any(|(path, existing_uri)| {
        let is_current = !editing_path.is_empty() && path == editing_path;
        !is_current && existing_uri.trim() == source_uri
    });
    if duplicated {
        Err(format!("{item_label}源地址已存在"))
    } else {
        Ok(())
    }
}

/// 返回来源类型的稳定展示名称。
pub fn source_type_name(source_type: SourceType) -> &'static str {
    match source_type {
        SourceType::Http => "http",
        SourceType::File => "file",
    }
}

/// 读取本地文件或 HTTP 来源。
pub fn read_source(source_type: SourceType, source_uri: &str) -> Result<String, String> {
    match source_type {
        SourceType::File => {
            fs::read_to_string(source_uri).map_err(|error| format!("读取源文件失败：{error}"))
        }
        SourceType::Http => {
            if !(source_uri.starts_with("http://") || source_uri.starts_with("https://")) {
                return Err("HTTP 地址必须以 http:// 或 https:// 开头".to_string());
            }
            let uri = source_uri.to_string();
            api::block(async move {
                let response = reqwest::Client::new()
                    .get(&uri)
                    .timeout(Duration::from_secs(30))
                    .send()
                    .await
                    .map_err(|error| format!("HTTP 下载失败：{error}"))?;
                if !response.status().is_success() {
                    return Err(format!("HTTP 下载失败：状态码 {}", response.status()));
                }
                response
                    .text()
                    .await
                    .map_err(|error| format!("读取 HTTP 响应失败：{error}"))
            })
        }
    }
}

/// 校验来源内容非空且根节点为 YAML 对象。
pub fn validate_yaml(content: &str) -> Result<(), String> {
    if content.trim().is_empty() {
        return Err("配置内容不能为空".to_string());
    }
    let value: serde_json::Value =
        serde_saphyr::from_str(content).map_err(|error| format!("YAML 解析失败：{error}"))?;
    if !value.is_object() {
        return Err("配置内容必须是 YAML 对象".to_string());
    }
    Ok(())
}

/// 从来源地址末段生成展示名称。
pub fn fallback_name(source_uri: &str) -> String {
    let without_query = source_uri.split(['?', '#']).next().unwrap_or(source_uri);
    let candidate = without_query
        .trim_end_matches('/')
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
        .trim();
    if candidate.is_empty() {
        "未命名配置".to_string()
    } else {
        candidate.to_string()
    }
}

/// 将来源末段清理为内部副本文件名。
pub fn safe_file_name(source_uri: &str) -> String {
    let candidate = fallback_name(source_uri);
    let mut name = String::new();
    for character in candidate.chars() {
        if character.is_alphanumeric() || matches!(character, '-' | '_' | '.') {
            name.push(character);
        } else {
            name.push('_');
        }
    }
    if name.is_empty() || name == "." || name == ".." {
        name = "source".to_string();
    }
    if !name.contains('.') {
        name.push_str(".yaml");
    }
    name
}

/// 为指定内部目录生成不冲突的副本路径。
pub fn unique_internal_path(
    root: &Path,
    directory: &str,
    source_uri: &str,
) -> Result<PathBuf, String> {
    let directory = resolve_directory(root, directory)?;
    fs::create_dir_all(&directory).map_err(|error| format!("创建内部目录失败：{error}"))?;
    let base = safe_file_name(source_uri);
    let mut candidate = directory.join(&base);
    let mut index = 1;
    while candidate.exists() {
        let stem = Path::new(&base)
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("source");
        let extension = Path::new(&base)
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("yaml");
        candidate = directory.join(format!("{stem}-{index}.{extension}"));
        index += 1;
    }
    Ok(candidate)
}

/// 解析并校验指定内部目录下的相对路径。
pub fn safe_internal_path(root: &Path, directory: &str, relative: &str) -> Result<PathBuf, String> {
    let path = Path::new(relative);
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::CurDir
                    | Component::ParentDir
                    | Component::RootDir
                    | Component::Prefix(_)
            )
        })
    {
        return Err("内部文件路径不安全".to_string());
    }

    let directory = resolve_directory(root, directory)?;
    let target = root.join(path);
    if !target.starts_with(&directory) || target == directory {
        return Err(format!("内部文件路径必须位于 {} 下", directory.display()));
    }
    Ok(target)
}

/// 将内部绝对路径转换为 app.yaml 使用的相对路径。
pub fn relative_internal_path(root: &Path, target: &Path) -> String {
    target
        .strip_prefix(root)
        .unwrap_or(target)
        .to_string_lossy()
        .replace('\\', "/")
}

/// 写入内部副本。
pub fn write_internal(path: &Path, content: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("创建内部目录失败：{error}"))?;
    }
    fs::write(path, content).map_err(|error| format!("写入内部副本失败：{error}"))
}

fn resolve_directory(root: &Path, directory: &str) -> Result<PathBuf, String> {
    let relative = Path::new(directory);
    if relative.is_absolute()
        || relative.components().any(|component| {
            matches!(
                component,
                Component::CurDir
                    | Component::ParentDir
                    | Component::RootDir
                    | Component::Prefix(_)
            )
        })
    {
        return Err("内部目录路径不安全".to_string());
    }
    let target = root.join(relative);
    if target == *root || !target.starts_with(root) {
        return Err("内部目录必须位于运行时根目录下".to_string());
    }
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constants::{CONFIGS_DIR, OVERRIDES_DIR};
    use std::fs;

    fn temp_root(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "clash_ui_source_test_{name}_{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("创建测试目录失败");
        path
    }

    #[test]
    fn keeps_name_and_filename_fallback_stable() {
        assert_eq!(fallback_name("C:\\configs\\demo.yaml"), "demo.yaml");
        assert_eq!(
            fallback_name("https://example.com/a/demo.yaml?x=1"),
            "demo.yaml"
        );
        assert_eq!(safe_file_name("../../secret"), "secret.yaml");
    }

    #[test]
    fn requires_source_content_object() {
        assert!(validate_yaml("foo: bar\n").is_ok());
        assert!(validate_yaml("\n").is_err());
        assert!(validate_yaml("- value\n").is_err());
    }

    #[test]
    fn rejects_duplicate_source_uri_in_same_page() {
        let entries = [("configs/one.yaml", " https://example.com/shared.yaml ")];
        assert!(ensure_unique_source_uri(
            entries.iter().copied(),
            "",
            "https://example.com/shared.yaml",
            "配置"
        )
        .is_err());
    }

    #[test]
    fn allows_same_source_uri_in_different_pages() {
        let configs = [("configs/one.yaml", "https://example.com/shared.yaml")];
        let overrides = [("overrides/one.yaml", "https://example.com/other.yaml")];
        assert!(
            ensure_unique_source_uri(configs.iter().copied(), "", overrides[0].1, "配置").is_ok()
        );
        assert!(
            ensure_unique_source_uri(overrides.iter().copied(), "", configs[0].1, "覆写").is_ok()
        );
    }

    #[test]
    fn allows_editing_current_source_uri() {
        let entries = [("configs/one.yaml", "https://example.com/shared.yaml")];
        assert!(ensure_unique_source_uri(
            entries.iter().copied(),
            "configs/one.yaml",
            " https://example.com/shared.yaml ",
            "配置"
        )
        .is_ok());
    }

    #[test]
    fn rejects_editing_to_another_entry_source_uri() {
        let entries = [
            ("configs/one.yaml", "https://example.com/one.yaml"),
            ("configs/two.yaml", "https://example.com/two.yaml"),
        ];
        assert!(ensure_unique_source_uri(
            entries.iter().copied(),
            "configs/one.yaml",
            " https://example.com/two.yaml ",
            "配置"
        )
        .is_err());
    }

    #[test]
    fn isolates_internal_paths_and_rejects_traversal() {
        let root = Path::new("C:\\clash-ui");
        assert!(safe_internal_path(root, CONFIGS_DIR, &format!("{CONFIGS_DIR}/demo.yaml")).is_ok());
        assert!(
            safe_internal_path(root, CONFIGS_DIR, &format!("{CONFIGS_DIR}/../app.yaml")).is_err()
        );
        assert!(safe_internal_path(root, CONFIGS_DIR, "C:\\other.yaml").is_err());
        assert!(
            safe_internal_path(root, CONFIGS_DIR, &format!("{OVERRIDES_DIR}/demo.yaml")).is_err()
        );
        assert!(safe_internal_path(root, CONFIGS_DIR, CONFIGS_DIR).is_err());
        assert!(safe_internal_path(root, CONFIGS_DIR, &format!("{CONFIGS_DIR}/")).is_err());
        assert!(safe_internal_path(root, CONFIGS_DIR, &format!("{CONFIGS_DIR}/.")).is_err());
    }

    #[test]
    fn increments_duplicate_unique_paths() {
        let root = temp_root("unique");
        let first = unique_internal_path(&root, OVERRIDES_DIR, "https://host/a/demo.yaml")
            .expect("生成首个路径失败");
        fs::write(&first, "foo: 1\n").expect("写入测试副本失败");
        let second = unique_internal_path(&root, OVERRIDES_DIR, "https://host/a/demo.yaml")
            .expect("生成第二个路径失败");
        assert_ne!(first, second);
        assert!(second.ends_with("demo-1.yaml"));
    }
}
