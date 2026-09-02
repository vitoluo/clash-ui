use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::app::config::{self, AppConfig, OverrideEntry};
use crate::consts::{FIXED_YAML_PATH, RUNTIME_DIR};

use super::core::CoreError;

// 数组合并指令：前置、追加或插入到指定索引之后。
enum ArrayOp {
    Prepend,
    Append,
    Insert(usize),
}

// 判断键名是否为数组合并指令。
fn is_instruction_key(key: &str) -> bool {
    if key.ends_with("::^") || key.ends_with("::$") {
        return true;
    }
    if let Some(index) = key.rfind("::") {
        let number = &key[index + 2..];
        !number.is_empty() && number.chars().all(|c| c.is_ascii_digit())
    } else {
        false
    }
}

// 解析数组合并指令键。
fn parse_instruction(key: &str) -> Result<Option<(String, ArrayOp)>, CoreError> {
    if let Some(stripped) = key.strip_suffix("::^") {
        return Ok(Some((stripped.to_string(), ArrayOp::Prepend)));
    }
    if let Some(stripped) = key.strip_suffix("::$") {
        return Ok(Some((stripped.to_string(), ArrayOp::Append)));
    }
    if let Some(index) = key.rfind("::") {
        let number = &key[index + 2..];
        if !number.is_empty() && number.chars().all(|c| c.is_ascii_digit()) {
            let position = number.parse().map_err(|error| {
                CoreError::Serialize(format!("数组合并索引无效 {number}：{error}"))
            })?;
            return Ok(Some((key[..index].to_string(), ArrayOp::Insert(position))));
        }
    }
    Ok(None)
}

// 对目标数组应用前置、追加或插入操作。
fn apply_array_op(target: &mut Value, op: ArrayOp, other: Value) {
    let other_array = match other {
        Value::Array(values) => values,
        value => vec![value],
    };
    let mut base_array = match target {
        Value::Array(values) => std::mem::take(values),
        _ => Vec::new(),
    };
    match op {
        ArrayOp::Prepend => {
            let mut values = other_array;
            values.extend(base_array);
            base_array = values;
        }
        ArrayOp::Append => {
            base_array.extend(other_array);
        }
        ArrayOp::Insert(index) => {
            let position = index.saturating_add(1).min(base_array.len());
            base_array.splice(position..position, other_array);
        }
    }
    *target = Value::Array(base_array);
}

// 深度合并对象；数组默认整体覆盖，数组指令按键名执行。
fn deep_merge(base: &mut Value, other: Value) -> Result<(), CoreError> {
    let Value::Object(other_map) = other else {
        *base = other;
        return Ok(());
    };
    let Value::Object(base_map) = base else {
        *base = Value::Object(other_map);
        return Ok(());
    };

    let mut instructions = Vec::new();
    for (key, value) in other_map {
        if is_instruction_key(&key) {
            instructions.push((key, value));
            continue;
        }
        match base_map.get_mut(&key) {
            Some(base_value) => deep_merge(base_value, value)?,
            None => {
                base_map.insert(key, value);
            }
        }
    }

    for (instruction_key, other_array) in instructions {
        if let Some((target, op)) = parse_instruction(&instruction_key)? {
            match base_map.get_mut(&target) {
                Some(target_value) => apply_array_op(target_value, op, other_array),
                None => {
                    let mut new_array = Value::Array(Vec::new());
                    apply_array_op(&mut new_array, op, other_array);
                    base_map.insert(target, new_array);
                }
            }
        }
    }
    Ok(())
}

/// 返回固定配置路径；数据根目录与资源根目录分离时，固定配置仍随执行文件保存。
fn fixed_yaml_path(root: &Path) -> Result<PathBuf, CoreError> {
    let local_path = root.join(FIXED_YAML_PATH);
    if local_path
        .try_exists()
        .map_err(|error| CoreError::Io("检查固定配置失败".into(), error))?
    {
        return Ok(local_path);
    }

    let executable = std::env::current_exe()
        .map_err(|error| CoreError::Io("获取当前程序路径失败".into(), error))?;
    let executable_root = executable.parent().ok_or_else(|| {
        CoreError::Io(
            "获取当前程序目录失败".into(),
            std::io::Error::new(std::io::ErrorKind::NotFound, "可执行文件没有父目录"),
        )
    })?;
    Ok(executable_root.join(FIXED_YAML_PATH))
}

// 按配置、覆盖、clash 设置和固定配置的顺序生成运行时配置。
fn merge_config_inner(root: &Path, cfg: &AppConfig) -> Result<(), CoreError> {
    let mut merged = Value::Object(serde_json::Map::new());

    for entry in cfg.configs.iter().filter(|entry| entry.enabled) {
        let content = fs::read_to_string(root.join(&entry.path))
            .map_err(|error| CoreError::Io(format!("读取配置 {} 失败", entry.path), error))?;
        let value: Value = serde_saphyr::from_str(&content).map_err(|error| {
            CoreError::Parse(format!("解析配置 {} 失败", entry.path), Box::new(error))
        })?;
        deep_merge(&mut merged, value)?;
    }

    let mut overrides: Vec<&OverrideEntry> =
        cfg.overrides.iter().filter(|entry| entry.enabled).collect();
    overrides.sort_by_key(|entry| std::cmp::Reverse(entry.sort));
    for entry in overrides {
        let content = fs::read_to_string(root.join(&entry.path))
            .map_err(|error| CoreError::Io(format!("读取覆盖 {} 失败", entry.path), error))?;
        let value: Value = serde_saphyr::from_str(&content).map_err(|error| {
            CoreError::Parse(format!("解析覆盖 {} 失败", entry.path), Box::new(error))
        })?;
        deep_merge(&mut merged, value)?;
    }

    let mut clash_value = serde_json::to_value(&cfg.settings.clash).map_err(CoreError::Json)?;
    if let Value::Object(map) = &mut clash_value {
        let tun_enable = cfg.proxy_status.tun;
        let tun = map
            .entry("tun")
            .or_insert_with(|| Value::Object(serde_json::Map::new()));
        if let Value::Object(tun_map) = tun {
            tun_map.insert("enable".to_string(), Value::Bool(tun_enable));
        }
    }
    deep_merge(&mut merged, clash_value)?;

    let fixed_content = fs::read_to_string(fixed_yaml_path(root)?)
        .map_err(|error| CoreError::Io("读取固定配置失败".into(), error))?;
    let fixed_value: Value = serde_saphyr::from_str(&fixed_content)
        .map_err(|error| CoreError::Parse("解析固定配置失败".into(), Box::new(error)))?;
    deep_merge(&mut merged, fixed_value)?;

    if let Value::Object(map) = &mut merged {
        // 合并完成后移除可选代理端口的 null 值，避免传给核心无效配置。
        for key in ["port", "socks-port", "mixed-port"] {
            if map.get(key).is_some_and(|value| value.is_null()) {
                map.remove(key);
            }
        }
        map.remove("external-controller");
        map.remove("secret");
    }

    let yaml = serde_saphyr::to_string(&merged)
        .map_err(|error| CoreError::Serialize(error.to_string()))?;
    let output_dir = root.join(RUNTIME_DIR);
    let output_path = output_dir.join("config.yaml");
    let should_write = match fs::read(&output_path) {
        Ok(existing) => existing != yaml.as_bytes(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
        Err(error) => return Err(CoreError::Io("读取现有合并配置失败".into(), error)),
    };
    if should_write {
        fs::create_dir_all(&output_dir)
            .map_err(|error| CoreError::Io("创建 runtime 目录失败".into(), error))?;
        fs::write(output_path, yaml)
            .map_err(|error| CoreError::Io("写入合并配置失败".into(), error))?;
    }
    Ok(())
}

// 合并当前全局配置，供核心启动和配置变更流程调用。
pub fn merge_config(root: &Path) -> Result<AppConfig, CoreError> {
    let cfg = config::get();
    merge_config_inner(root, &cfg)?;
    Ok(cfg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::config::ConfigEntry;
    use crate::consts::{ASSETS_DIR, CONFIGS_DIR, FIXED_YAML_PATH, OVERRIDES_DIR, RUNTIME_DIR};
    use std::fs;
    use std::path::PathBuf;

    // 创建独立临时目录，避免测试污染真实运行目录。
    fn tmp_root(name: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "clash_ui_merge_test_{}_{}_{}",
            name,
            std::process::id(),
            name.len()
        ));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).expect("创建测试目录失败");
        directory
    }

    #[test]
    fn yaml_value_round_trip() {
        let yaml = "num: 9000\nflag: true\nempty: ~\nlist:\n  - a\n  - b\nnested:\n  k: v\n";
        let value: Value = serde_saphyr::from_str(yaml).unwrap();
        assert_eq!(value["num"].as_u64(), Some(9000));
        assert_eq!(value["flag"].as_bool(), Some(true));
        assert!(value["empty"].is_null());
        let output = serde_saphyr::to_string(&value).unwrap();
        let round_trip: Value = serde_saphyr::from_str(&output).unwrap();
        assert_eq!(value, round_trip);
    }

    #[test]
    fn deep_merge_overwrites_and_preserves_missing_keys() {
        let mut base: Value = serde_json::from_str(r#"{"a":1,"b":{"x":1,"y":2}}"#).unwrap();
        let other: Value = serde_json::from_str(r#"{"b":{"y":20,"z":3},"c":4}"#).unwrap();
        deep_merge(&mut base, other).unwrap();
        assert_eq!(base["a"].as_i64(), Some(1));
        assert_eq!(base["b"]["y"].as_i64(), Some(20));
        assert_eq!(base["b"]["x"].as_i64(), Some(1));
        assert_eq!(base["b"]["z"].as_i64(), Some(3));
        assert_eq!(base["c"].as_i64(), Some(4));
    }

    #[test]
    fn array_instructions_apply_without_leaking_instruction_keys() {
        let mut base = serde_json::json!({"list": ["a", "b"]});
        deep_merge(&mut base, serde_json::json!({"list::^": ["z"]})).unwrap();
        assert_eq!(base["list"], serde_json::json!(["z", "a", "b"]));
        assert!(base.get("list::^").is_none());

        let mut base = serde_json::json!({"list": ["a", "b"]});
        deep_merge(&mut base, serde_json::json!({"list::$": ["z"]})).unwrap();
        assert_eq!(base["list"], serde_json::json!(["a", "b", "z"]));

        let mut base = serde_json::json!({"list": ["a", "b", "c"]});
        deep_merge(&mut base, serde_json::json!({"list::1": ["x"]})).unwrap();
        assert_eq!(base["list"], serde_json::json!(["a", "b", "x", "c"]));

        let mut base = serde_json::json!({"list": ["a", "b"]});
        deep_merge(&mut base, serde_json::json!({"list::9": ["x"]})).unwrap();
        assert_eq!(base["list"], serde_json::json!(["a", "b", "x"]));

        let mut base = serde_json::json!({});
        deep_merge(&mut base, serde_json::json!({"list::$": ["x"]})).unwrap();
        assert_eq!(base["list"], serde_json::json!(["x"]));

        let mut base = serde_json::json!({"list": ["a", "b"]});
        deep_merge(
            &mut base,
            serde_json::json!({format!("list::{}", usize::MAX): ["x", "y"]}),
        )
        .unwrap();
        assert_eq!(base["list"], serde_json::json!(["a", "b", "x", "y"]));
    }

    #[test]
    fn array_instruction_index_overflow_returns_error() {
        let mut base = serde_json::json!({"list": ["a"]});
        let key = format!("list::{}0", usize::MAX);

        assert!(deep_merge(&mut base, serde_json::json!({key: ["x"]})).is_err());
    }

    #[test]
    fn large_array_merge_preserves_order() {
        let mut base = serde_json::json!({"items": (0..5000).collect::<Vec<_>>()});
        let suffix = (5000..10000).collect::<Vec<_>>();
        deep_merge(&mut base, serde_json::json!({"items::$": suffix})).unwrap();
        let items = base["items"].as_array().unwrap();
        assert_eq!(items.len(), 10000);
        assert_eq!(items.first().unwrap().as_i64(), Some(0));
        assert_eq!(items.last().unwrap().as_i64(), Some(9999));
    }

    #[test]
    fn merge_order_and_removal_are_applied() {
        let root = tmp_root("merge");
        fs::create_dir_all(root.join(RUNTIME_DIR)).unwrap();
        fs::create_dir_all(root.join(ASSETS_DIR)).unwrap();
        fs::create_dir_all(root.join(CONFIGS_DIR)).unwrap();
        fs::create_dir_all(root.join(OVERRIDES_DIR)).unwrap();
        fs::write(
            root.join(FIXED_YAML_PATH),
            "external-ui: ui\nexternal-controller: fixed\nsecret: fixed\n",
        )
        .unwrap();
        fs::write(
            root.join(CONFIGS_DIR).join("c1.yaml"),
            "foo: from-config\nbar: 1\nsome-list:\n  - a\n  - b\n",
        )
        .unwrap();
        fs::write(
            root.join(OVERRIDES_DIR).join("o1.yaml"),
            "foo: from-override\nanother: 2\nsome-list::^:\n  - z\n",
        )
        .unwrap();

        let mut cfg = AppConfig::default();
        cfg.configs.push(ConfigEntry {
            enabled: true,
            path: format!("{CONFIGS_DIR}/c1.yaml"),
            ..ConfigEntry::default()
        });
        cfg.overrides.push(OverrideEntry {
            enabled: true,
            sort: 5,
            path: format!("{OVERRIDES_DIR}/o1.yaml"),
            ..OverrideEntry::default()
        });
        cfg.settings.clash.mixed_port = Some(7890);

        merge_config_inner(&root, &cfg).unwrap();
        let output = fs::read_to_string(root.join(RUNTIME_DIR).join("config.yaml")).unwrap();
        let value: Value = serde_saphyr::from_str(&output).unwrap();
        assert_eq!(value["foo"].as_str(), Some("from-override"));
        assert_eq!(value["bar"].as_i64(), Some(1));
        assert_eq!(value["another"].as_i64(), Some(2));
        assert_eq!(value["some-list"], serde_json::json!(["z", "a", "b"]));
        assert_eq!(value["mixed-port"].as_u64(), Some(7890));
        assert!(value.get("external-controller").is_none());
        assert!(value.get("secret").is_none());
    }

    #[test]
    fn override_sort_merges_larger_sort_first() {
        let root = tmp_root("override_order");
        fs::create_dir_all(root.join(ASSETS_DIR)).unwrap();
        fs::create_dir_all(root.join(RUNTIME_DIR)).unwrap();
        fs::create_dir_all(root.join(OVERRIDES_DIR)).unwrap();
        fs::write(root.join(FIXED_YAML_PATH), "external-ui: ui\n").unwrap();
        fs::write(root.join(OVERRIDES_DIR).join("a.yaml"), "conflict: A\n").unwrap();
        fs::write(root.join(OVERRIDES_DIR).join("b.yaml"), "conflict: B\n").unwrap();

        let mut cfg = AppConfig::default();
        cfg.overrides.push(OverrideEntry {
            enabled: true,
            sort: 0,
            path: format!("{OVERRIDES_DIR}/a.yaml"),
            ..OverrideEntry::default()
        });
        cfg.overrides.push(OverrideEntry {
            enabled: true,
            sort: 1,
            path: format!("{OVERRIDES_DIR}/b.yaml"),
            ..OverrideEntry::default()
        });

        merge_config_inner(&root, &cfg).unwrap();
        let output = fs::read_to_string(root.join(RUNTIME_DIR).join("config.yaml")).unwrap();
        let value: Value = serde_saphyr::from_str(&output).unwrap();
        assert_eq!(value["conflict"].as_str(), Some("A"));
    }

    #[test]
    fn merge_injects_tun_enable() {
        let root = tmp_root("tun");
        fs::create_dir_all(root.join(ASSETS_DIR)).unwrap();
        fs::write(root.join(FIXED_YAML_PATH), "external-ui: ui\n").unwrap();

        let mut cfg = AppConfig::default();
        cfg.proxy_status.tun = true;
        merge_config_inner(&root, &cfg).unwrap();
        let output = fs::read_to_string(root.join(RUNTIME_DIR).join("config.yaml")).unwrap();
        let value: Value = serde_saphyr::from_str(&output).unwrap();
        assert_eq!(value["tun"]["enable"].as_bool(), Some(true));
    }

    #[test]
    fn removes_null_proxy_ports_after_merge() {
        let root = tmp_root("null_ports");
        fs::create_dir_all(root.join(ASSETS_DIR)).unwrap();
        fs::write(root.join(FIXED_YAML_PATH), "external-ui: ui\n").unwrap();

        let mut cfg = AppConfig::default();
        cfg.settings.clash.port = None;
        cfg.settings.clash.socks_port = None;
        cfg.settings.clash.mixed_port = None;

        merge_config_inner(&root, &cfg).unwrap();
        let output = fs::read_to_string(root.join(RUNTIME_DIR).join("config.yaml")).unwrap();
        let value: Value = serde_saphyr::from_str(&output).unwrap();

        assert!(value.get("port").is_none());
        assert!(value.get("socks-port").is_none());
        assert!(value.get("mixed-port").is_none());
    }

    #[test]
    fn identical_runtime_config_keeps_existing_bytes() {
        let root = tmp_root("same_runtime");
        fs::create_dir_all(root.join(RUNTIME_DIR)).unwrap();
        fs::create_dir_all(root.join(ASSETS_DIR)).unwrap();
        fs::write(root.join(FIXED_YAML_PATH), "external-ui: ui\n").unwrap();

        let cfg = AppConfig::default();
        merge_config_inner(&root, &cfg).unwrap();
        let path = root.join(RUNTIME_DIR).join("config.yaml");
        let first = fs::read(&path).unwrap();
        merge_config_inner(&root, &cfg).unwrap();
        let second = fs::read(path).unwrap();
        assert_eq!(first, second);
    }
}
