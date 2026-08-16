// Windows UWP 保存所需的按需提权流程及 helper 协议。

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const ERROR_CANCELLED: i32 = 1223;
const UWP_HELPER_FILE_NAME: &str = "clash-ui-uwp-helper.exe";

pub(super) fn serialize_uwp_changes(changes: &[(String, bool)]) -> Result<String, String> {
    serde_json::to_string(changes).map_err(|error| format!("序列化 UWP 变更失败：{error}"))
}

pub(super) fn deserialize_uwp_changes(value: &str) -> Result<Vec<(String, bool)>, String> {
    serde_json::from_str(value).map_err(|error| format!("解析提权 UWP 变更失败：{error}"))
}

fn encode_uwp_argument(value: &str) -> String {
    value
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn decode_uwp_argument(value: &str) -> Result<String, String> {
    if !value.is_ascii() || value.len() % 2 != 0 {
        return Err("提权 UWP 参数长度无效".to_string());
    }
    let bytes = (0..value.len())
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(&value[index..index + 2], 16)
                .map_err(|_| "提权 UWP 参数不是有效十六进制".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    String::from_utf8(bytes).map_err(|_| "提权 UWP 参数不是有效 UTF-8".to_string())
}

fn format_uwp_result(result: &Result<(), String>) -> String {
    match result {
        Ok(()) => "ok\n".to_string(),
        Err(error) => format!("error\n{error}"),
    }
}

pub(super) fn parse_uwp_result(value: &str) -> Result<(), String> {
    let value = value.trim_end_matches(['\r', '\n']);
    if value == "ok" {
        return Ok(());
    }
    if let Some(error) = value.strip_prefix("error\n") {
        return Err(if error.trim().is_empty() {
            "提权 UWP helper 返回未知错误".to_string()
        } else {
            error.to_string()
        });
    }
    Err("提权 UWP helper 返回格式无效".to_string())
}

pub(super) fn uwp_result_temp_path(path: &Path) -> PathBuf {
    let mut temp_path = path.to_path_buf();
    temp_path.set_extension("tmp");
    temp_path
}

pub(super) fn write_uwp_result(path: &Path, result: &Result<(), String>) -> Result<(), String> {
    let temp_path = uwp_result_temp_path(path);
    fs::write(&temp_path, format_uwp_result(result))
        .map_err(|error| format!("写入提权 UWP 结果失败：{error}"))?;
    fs::rename(&temp_path, path).map_err(|error| format!("提交提权 UWP 结果失败：{error}"))
}

pub(super) fn cleanup_uwp_result(path: &Path) {
    let _ = fs::remove_file(path);
    let _ = fs::remove_file(uwp_result_temp_path(path));
}

pub(super) fn wait_for_uwp_result(path: &Path) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        match fs::read_to_string(path) {
            Ok(value) => return parse_uwp_result(&value),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("读取提权 UWP 结果失败：{error}")),
        }
        if Instant::now() >= deadline {
            return Err("等待提权 UWP helper 结果超时".to_string());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

pub(super) fn new_uwp_result_path() -> Result<PathBuf, String> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("获取提权 UWP 请求时间失败：{error}"))?
        .as_nanos();
    let token = rand::random::<u64>();
    let mut path = std::env::temp_dir();
    path.push(format!(
        "clash-ui-uwp-{}-{timestamp:x}-{token:x}.result",
        std::process::id()
    ));
    Ok(path)
}

pub(super) fn resolve_uwp_helper_path(current_exe: &Path) -> Result<PathBuf, String> {
    let parent = current_exe
        .parent()
        .ok_or_else(|| "当前程序路径没有可用目录".to_string())?;
    Ok(parent.join(UWP_HELPER_FILE_NAME))
}

pub(super) fn set_uwp_loopback_elevated(changes: &[(String, bool)]) -> Result<(), String> {
    let payload = serialize_uwp_changes(changes)?;
    let result_path = new_uwp_result_path()?;
    let result = (|| {
        let executable =
            std::env::current_exe().map_err(|error| format!("获取当前程序路径失败：{error}"))?;
        let helper = resolve_uwp_helper_path(&executable)?;
        if !helper.is_file() {
            return Err(format!("UWP helper 不存在：{}", helper.display()));
        }
        let mut command = std::process::Command::new(helper);
        command
            .arg(encode_uwp_argument(&payload))
            .arg(encode_uwp_argument(&result_path.to_string_lossy()));
        let output = elevated_command::Command::new(command)
            .output()
            .map_err(|error| format!("请求 UWP 管理员权限失败：{error}"))?;
        let launch_code = output.status.code();
        if let Some(error) = elevated_uwp_launch_error(launch_code) {
            return Err(error);
        }
        wait_for_uwp_result(&result_path)
    })();
    cleanup_uwp_result(&result_path);
    result
}

pub(super) fn elevated_uwp_launch_error(launch_code: Option<i32>) -> Option<String> {
    if launch_code == Some(ERROR_CANCELLED) {
        return Some("用户取消了 UWP 管理员权限请求".to_string());
    }
    launch_code
        .filter(|code| *code <= 32)
        .map(|code| format!("请求 UWP 管理员权限失败，启动返回码 {code}"))
}

/// 由独立 UWP helper 读取参数、执行写入并提交业务结果。
pub fn run_uwp_helper() -> Result<(), String> {
    let mut args = std::env::args_os();
    let _ = args.next();
    let payload = args
        .next()
        .ok_or_else(|| "缺少提权 UWP 变更参数".to_string())?;
    let result_path = args
        .next()
        .ok_or_else(|| "缺少提权 UWP 结果路径".to_string())?;
    if args.next().is_some() {
        return Err("提权 UWP helper 参数数量无效".to_string());
    }
    let result_path = result_path
        .to_str()
        .ok_or_else(|| "提权 UWP 结果路径不是有效 UTF-8".to_string())
        .and_then(decode_uwp_argument)
        .map(PathBuf::from)?;
    let result = match payload.to_str() {
        Some(payload) => decode_uwp_argument(payload)
            .and_then(|payload| deserialize_uwp_changes(&payload))
            .and_then(|changes| super::uwp::set_uwp_loopback_batch_impl(&changes)),
        None => Err("提权 UWP 变更参数不是有效 UTF-8".to_string()),
    };
    write_uwp_result(&result_path, &result)
}
