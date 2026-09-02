// 配置页面控制器：配置快照、源文件导入、核心联动和 Slint 回写。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use slint::{ComponentHandle, Model, ModelRc, VecModel, Weak};

use crate::app::config::{self, ConfigEntry, SourceType};
use crate::clash::core;
use crate::consts::{CONFIGS_DIR, RUNTIME_DIR};
use crate::controller::common::source;
use crate::{platform, ConfigRow, MainWindow};

#[derive(Debug)]
pub struct ConfigViewState {
    root: PathBuf,
    loading: bool,
    busy: bool,
    error: String,
    initialized: bool,
    next_token: u64,
    refresh_token: u64,
    operation_token: u64,
}

pub type SharedConfigState = Arc<Mutex<ConfigViewState>>;

pub(crate) fn bind_callbacks(window: &MainWindow, state: SharedConfigState) {
    window
        .global::<crate::ConfigModel>()
        .set_configs(ModelRc::new(VecModel::default()));
    bind_item_callbacks(window, state.clone());
    bind_form_callbacks(window, state.clone());
    bind_runtime_callbacks(window, state);
}

fn bind_item_callbacks(window: &MainWindow, state: SharedConfigState) {
    let weak = window.as_weak();
    window.global::<crate::ConfigModel>().on_toggle_enabled({
        let weak = weak.clone();
        let state = state.clone();
        move |path| toggle_enabled(weak.clone(), state.clone(), path.into())
    });
    window.global::<crate::ConfigModel>().on_edit({
        let weak = weak.clone();
        let state = state.clone();
        move |path| edit(weak.clone(), state.clone(), path.into())
    });
    window.global::<crate::ConfigModel>().on_update({
        let weak = weak.clone();
        let state = state.clone();
        move |path| update(weak.clone(), state.clone(), path.into())
    });
    window.global::<crate::ConfigModel>().on_delete({
        let weak = weak.clone();
        let state = state.clone();
        move |path| delete(weak.clone(), state.clone(), path.into())
    });
}

fn bind_form_callbacks(window: &MainWindow, state: SharedConfigState) {
    let weak = window.as_weak();
    window.global::<crate::ConfigModel>().on_open_add({
        let weak = weak.clone();
        move || open_add(weak.clone())
    });
    window.global::<crate::ConfigModel>().on_choose_file({
        let weak = weak.clone();
        let state = state.clone();
        move || choose_file(weak.clone(), state.clone())
    });
    window.global::<crate::ConfigModel>().on_submit_form({
        let weak = weak.clone();
        let state = state.clone();
        move |editing_path, name, source_type, source_uri| {
            submit_form(
                weak.clone(),
                state.clone(),
                editing_path.into(),
                name.into(),
                source_type.into(),
                source_uri.into(),
            )
        }
    });
    window.global::<crate::ConfigModel>().on_cancel_form({
        let weak = weak.clone();
        move || cancel_form(weak.clone())
    });
}

fn bind_runtime_callbacks(window: &MainWindow, state: SharedConfigState) {
    let weak = window.as_weak();
    window.global::<crate::ConfigModel>().on_view_runtime({
        let weak = weak.clone();
        let state = state.clone();
        move || view_runtime(weak.clone(), state.clone())
    });
    window.global::<crate::ConfigModel>().on_update_all({
        let weak = weak.clone();
        move || update_all(weak.clone(), state.clone())
    });
}

fn lock_state(state: &SharedConfigState) -> MutexGuard<'_, ConfigViewState> {
    state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub fn new_state(root: PathBuf) -> SharedConfigState {
    Arc::new(Mutex::new(ConfigViewState {
        root,
        loading: false,
        busy: false,
        error: String::new(),
        initialized: false,
        next_token: 0,
        refresh_token: 0,
        operation_token: 0,
    }))
}

fn next_token(state: &mut ConfigViewState) -> u64 {
    state.next_token = state.next_token.wrapping_add(1).max(1);
    state.next_token
}

fn config_rows(entries: &[ConfigEntry]) -> Vec<ConfigRow> {
    entries
        .iter()
        .map(|entry| ConfigRow {
            name: display_name(entry).into(),
            enabled: entry.enabled,
            source_type: source::source_type_name(entry.source_type).into(),
            source_uri: entry.source_uri.clone().into(),
            path: entry.path.clone().into(),
        })
        .collect()
}

fn display_name(entry: &ConfigEntry) -> String {
    if !entry.name.trim().is_empty() {
        return entry.name.clone();
    }
    source::fallback_name(&entry.source_uri)
}

fn set_ui_model(window: &MainWindow, state: &ConfigViewState, entries: &[ConfigEntry]) {
    let model = window.global::<crate::ConfigModel>();
    let rows = config_rows(entries);
    let current = model.get_configs();
    if let Some(rows_model) = current.as_any().downcast_ref::<VecModel<ConfigRow>>() {
        sync_rows(rows_model, rows);
    } else {
        model.set_configs(ModelRc::new(VecModel::from(rows)));
    }
    set_ui_state(window, state);
}

fn set_ui_state(window: &MainWindow, state: &ConfigViewState) {
    let model = window.global::<crate::ConfigModel>();
    model.set_loading(state.loading);
    model.set_busy(state.busy);
    model.set_error(state.error.clone().into());
}

fn sync_rows(model: &VecModel<ConfigRow>, rows: Vec<ConfigRow>) {
    let common = model.row_count().min(rows.len());
    for (index, row) in rows.iter().take(common).cloned().enumerate() {
        if model.row_data(index).as_ref() != Some(&row) {
            model.set_row_data(index, row);
        }
    }
    while model.row_count() > rows.len() {
        model.remove(model.row_count() - 1);
    }
    for row in rows.into_iter().skip(common) {
        model.push(row);
    }
}

fn set_toast(window: &MainWindow, message: &str, variant: i32) {
    let model = window.global::<crate::ConfigModel>();
    model.set_toast_message(message.to_string().into());
    model.set_toast_variant(variant);
    model.set_toast_visible(true);
}

fn invoke_ui<F>(callback: F)
where
    F: FnOnce() + Send + 'static,
{
    if let Err(error) = slint::invoke_from_event_loop(callback) {
        crate::log::error(format_args!("配置页 UI 回调失败：{error}"));
    }
}

/// 从 app.yaml 快照刷新配置卡片，不轮询外部服务。
pub fn refresh_async(weak: Weak<MainWindow>, state: SharedConfigState) {
    let mut view = lock_state(&state);
    if view.busy {
        return;
    }
    if view.initialized {
        if let Some(window) = weak.upgrade() {
            set_ui_state(&window, &view);
        }
        return;
    }
    view.initialized = true;
    view.refresh_token = next_token(&mut view);
    view.loading = false;
    view.error.clear();
    if let Some(window) = weak.upgrade() {
        set_ui_model(&window, &view, &config::configs());
    }
}

fn begin_operation(weak: &Weak<MainWindow>, state: &SharedConfigState) -> Option<u64> {
    let mut view = lock_state(state);
    if view.busy {
        return None;
    }
    let token = next_token(&mut view);
    view.operation_token = token;
    view.busy = true;
    view.error.clear();
    if let Some(window) = weak.upgrade() {
        set_ui_state(&window, &view);
    }
    Some(token)
}

fn finish_operation(
    weak: Weak<MainWindow>,
    state: SharedConfigState,
    token: u64,
    result: Result<String, String>,
    close_form: bool,
) {
    if let Err(message) = &result {
        crate::log::error(format_args!("{message}"));
    }
    invoke_ui(move || {
        let Some(window) = weak.upgrade() else { return };
        let mut view = lock_state(&state);
        if view.operation_token != token {
            return;
        }
        view.busy = false;
        let success = result.is_ok();
        match result {
            Ok(message) => {
                view.error.clear();
                if close_form {
                    window.global::<crate::ConfigModel>().set_form_open(false);
                }
                set_toast(&window, &message, 1);
            }
            Err(message) => {
                view.error = message.clone();
                set_toast(&window, &message, 2);
            }
        }
        if success {
            set_ui_model(&window, &view, &config::configs());
        } else {
            set_ui_state(&window, &view);
        }
        if !success && close_form {
            window.global::<crate::ConfigModel>().set_form_open(true);
        }
    });
}

/// 打开添加表单并清空上一条编辑状态。
pub fn open_add(weak: Weak<MainWindow>) {
    let Some(window) = weak.upgrade() else { return };
    let model = window.global::<crate::ConfigModel>();
    model.set_form_editing_path("".into());
    model.set_form_name("".into());
    model.set_form_type_index(1);
    model.set_form_source("".into());
    model.set_form_open(true);
}

/// 将现有配置带入编辑表单，异步操作中的旧结果不会覆盖当前表单。
pub fn edit(weak: Weak<MainWindow>, state: SharedConfigState, path: String) {
    if lock_state(&state).busy {
        return;
    }
    let entry = config::configs()
        .into_iter()
        .find(|entry| entry.path == path);
    let Some(entry) = entry else {
        if let Some(window) = weak.upgrade() {
            window
                .global::<crate::ConfigModel>()
                .set_error("未找到要编辑的配置".into());
        }
        return;
    };
    if let Some(window) = weak.upgrade() {
        let model = window.global::<crate::ConfigModel>();
        model.set_form_editing_path(entry.path.into());
        model.set_form_name(entry.name.into());
        model.set_form_type_index(if entry.source_type == SourceType::Http {
            0
        } else {
            1
        });
        model.set_form_source(entry.source_uri.into());
        model.set_form_open(true);
    }
}

/// 取消添加/编辑表单。
pub fn cancel_form(weak: Weak<MainWindow>) {
    if let Some(window) = weak.upgrade() {
        window.global::<crate::ConfigModel>().set_form_open(false);
    }
}

/// 在平台抽象层打开文件选择器，并将返回路径写回表单。
pub fn choose_file(weak: Weak<MainWindow>, state: SharedConfigState) {
    let Some(token) = begin_operation(&weak, &state) else {
        return;
    };
    crate::runtime::spawn_blocking(move || {
        let result = platform::pick_config_file()
            .map_err(|error| error.to_string())
            .and_then(|path| match path {
                Some(path) if path.is_file() => Ok(Some(path)),
                Some(_) => Err("选择的路径不是文件".to_string()),
                None => Ok(None),
            });
        if let Err(error) = &result {
            crate::log::error(format_args!("选择配置源文件失败：{error}"));
        }
        invoke_ui(move || {
            let Some(window) = weak.upgrade() else { return };
            let mut view = lock_state(&state);
            if view.operation_token != token {
                return;
            }
            view.busy = false;
            match result {
                Ok(Some(path)) => {
                    let model = window.global::<crate::ConfigModel>();
                    model.set_form_type_index(1);
                    model.set_form_source(path.to_string_lossy().to_string().into());
                }
                Ok(None) => {}
                Err(error) => {
                    view.error = error.clone();
                    set_toast(&window, &error, 2);
                }
            }
            set_ui_state(&window, &view);
        });
    });
}

/// 切换配置启用状态；一次持久化操作清除其它启用项。
pub fn toggle_enabled(weak: Weak<MainWindow>, state: SharedConfigState, path: String) {
    let Some(token) = begin_operation(&weak, &state) else {
        return;
    };
    let root = lock_state(&state).root.clone();
    crate::runtime::spawn_blocking(move || {
        let configs = config::configs();
        let Some(target) = configs.iter().find(|entry| entry.path == path) else {
            finish_operation(
                weak,
                state,
                token,
                Err("未找到要切换的配置".to_string()),
                false,
            );
            return;
        };
        let enable = !target.enabled;
        config::update(|current| {
            for entry in &mut current.configs {
                entry.enabled = false;
            }
            if enable {
                if let Some(entry) = current.configs.iter_mut().find(|entry| entry.path == path) {
                    entry.enabled = true;
                }
            }
        });
        let result = core::on_config_changed(&root)
            .map(|_| {
                if enable {
                    "已启用配置"
                } else {
                    "已禁用配置"
                }
                .to_string()
            })
            .map_err(|error| format!("配置已保存，但核心联动失败：{error}"));
        finish_operation(weak, state, token, result, false);
    });
}

/// 提交添加或编辑表单。
pub fn submit_form(
    weak: Weak<MainWindow>,
    state: SharedConfigState,
    editing_path: String,
    name: String,
    source_type: String,
    source_uri: String,
) {
    let Some(token) = begin_operation(&weak, &state) else {
        return;
    };
    let root = lock_state(&state).root.clone();
    crate::runtime::spawn_task(async move {
        let result = save_entry_async(&root, &editing_path, &name, &source_type, &source_uri)
            .await
            .map(|_| "配置已保存".to_string())
            .map_err(|error| format!("保存配置失败：{error}"));
        finish_operation(weak, state, token, result, true);
    });
}

/// 更新单条配置的源内容；读取或校验失败时不触碰旧副本。
pub fn update(weak: Weak<MainWindow>, state: SharedConfigState, path: String) {
    let Some(token) = begin_operation(&weak, &state) else {
        return;
    };
    let root = lock_state(&state).root.clone();
    crate::runtime::spawn_task(async move {
        let result = update_entry_async(&root, &path)
            .await
            .map(|_| "配置已更新".to_string())
            .map_err(|error| format!("更新配置失败：{error}"));
        finish_operation(weak, state, token, result, false);
    });
}

/// 删除配置；启用项严格按停止核心、删除、重新合并顺序执行。
pub fn delete(weak: Weak<MainWindow>, state: SharedConfigState, path: String) {
    let Some(token) = begin_operation(&weak, &state) else {
        return;
    };
    let root = lock_state(&state).root.clone();
    crate::runtime::spawn_blocking(move || {
        let result = delete_entry(&root, &path)
            .map(|_| "配置已删除".to_string())
            .map_err(|error| format!("删除配置失败：{error}"));
        finish_operation(weak, state, token, result, false);
    });
}

/// 读取真实运行配置文件并展示只读内容。
pub fn view_runtime(weak: Weak<MainWindow>, state: SharedConfigState) {
    let Some(token) = begin_operation(&weak, &state) else {
        return;
    };
    let root = lock_state(&state).root.clone();
    crate::runtime::spawn_blocking(move || {
        let result = fs::read_to_string(root.join(RUNTIME_DIR).join("config.yaml"))
            .map_err(|error| format!("读取运行配置失败：{error}"));
        if let Err(error) = &result {
            crate::log::error(format_args!("{error}"));
        }
        invoke_ui(move || {
            let Some(window) = weak.upgrade() else { return };
            let mut view = lock_state(&state);
            if view.operation_token != token {
                return;
            }
            view.busy = false;
            match result {
                Ok(content) => {
                    let model = window.global::<crate::ConfigModel>();
                    model.set_runtime_config(content.into());
                    model.set_runtime_open(true);
                    view.error.clear();
                }
                Err(error) => {
                    view.error = error.clone();
                    set_toast(&window, &error, 2);
                }
            }
            set_ui_state(&window, &view);
        });
    });
}

/// 按列表顺序更新全部配置，失败项保留旧副本并汇总错误。
pub fn update_all(weak: Weak<MainWindow>, state: SharedConfigState) {
    let Some(token) = begin_operation(&weak, &state) else {
        return;
    };
    let root = lock_state(&state).root.clone();
    crate::runtime::spawn_task(async move {
        let entries = config::configs();
        let mut failures = Vec::new();
        let mut enabled_updated = false;
        for entry in entries {
            let display_name = display_name(&entry);
            let enabled = entry.enabled;
            let result = match source::read_source(entry.source_type, &entry.source_uri).await {
                Ok(content) => {
                    if let Err(error) = source::validate_yaml(&content) {
                        Err(error)
                    } else {
                        let root = root.clone();
                        let entry_for_write = entry.clone();
                        match crate::runtime::spawn_blocking(move || {
                            update_entry_without_core(&root, &entry_for_write, &content)
                        })
                        .await
                        {
                            Ok(result) => result,
                            Err(error) => Err(format!("更新配置任务失败：{error}")),
                        }
                    }
                }
                Err(error) => Err(error),
            };
            match result {
                Ok(()) => enabled_updated |= enabled,
                Err(error) => failures.push(format!("{}：{error}", display_name)),
            }
        }
        if enabled_updated {
            let result = crate::runtime::spawn_blocking(move || core::on_config_changed(&root))
                .await
                .map_err(|error| format!("核心联动任务失败：{error}"))
                .and_then(|result| result.map_err(|error| error.to_string()));
            if let Err(error) = result {
                failures.push(format!("核心联动失败：{error}"));
            }
        }
        let result = if failures.is_empty() {
            Ok("已更新全部配置".to_string())
        } else {
            Err(format!("部分配置更新失败：{}", failures.join("；")))
        };
        finish_operation(weak, state, token, result, false);
    });
}

async fn save_entry_async(
    root: &Path,
    editing_path: &str,
    name: &str,
    source_type: &str,
    source_uri: &str,
) -> Result<(), String> {
    let source_type = source::parse_source_type(source_type)?;
    let source_uri = source_uri.trim();
    if source_uri.is_empty() {
        return Err("源地址不能为空".to_string());
    }
    let configs = config::configs();
    source::ensure_unique_source_uri(
        configs
            .iter()
            .map(|entry| (entry.path.as_str(), entry.source_uri.as_str())),
        editing_path,
        source_uri,
        "配置",
    )?;
    let content = source::read_source(source_type, source_uri).await?;
    source::validate_yaml(&content)?;

    let root = root.to_path_buf();
    let editing_path = editing_path.to_string();
    let name = name.to_string();
    let source_uri = source_uri.to_string();
    crate::runtime::spawn_blocking(move || {
        save_entry_with_content(
            &root,
            &editing_path,
            &name,
            source_type,
            &source_uri,
            &content,
        )
    })
    .await
    .map_err(|error| format!("保存配置任务失败：{error}"))?
}

fn save_entry_with_content(
    root: &Path,
    editing_path: &str,
    name: &str,
    source_type: SourceType,
    source_uri: &str,
    content: &str,
) -> Result<(), String> {
    let configs = config::configs();

    let existing = if editing_path.is_empty() {
        None
    } else {
        Some(
            configs
                .iter()
                .find(|entry| entry.path == editing_path)
                .ok_or_else(|| "未找到要编辑的配置".to_string())?,
        )
    };
    let internal_path = if let Some(entry) = existing {
        source::safe_internal_path(root, CONFIGS_DIR, &entry.path)?
    } else {
        source::unique_internal_path(root, CONFIGS_DIR, source_uri)?
    };
    source::write_internal(&internal_path, content)?;

    let was_enabled = existing.map(|entry| entry.enabled).unwrap_or(false);
    let entry = ConfigEntry {
        name: if name.trim().is_empty() {
            source::fallback_name(source_uri)
        } else {
            name.trim().to_string()
        },
        enabled: was_enabled,
        source_type,
        source_uri: source_uri.to_string(),
        path: source::relative_internal_path(root, &internal_path),
    };
    config::update(|current| {
        if let Some(index) = current
            .configs
            .iter()
            .position(|current| current.path == editing_path && !editing_path.is_empty())
        {
            current.configs[index] = entry;
        } else {
            current.configs.push(entry);
        }
    });

    if was_enabled {
        core::on_config_changed(root)
            .map_err(|error| format!("配置已保存，但核心联动失败：{error}"))?;
    }
    Ok(())
}

async fn update_entry_async(root: &Path, path: &str) -> Result<(), String> {
    let entry = config::configs()
        .into_iter()
        .find(|entry| entry.path == path)
        .ok_or_else(|| "未找到要更新的配置".to_string())?;
    let content = source::read_source(entry.source_type, &entry.source_uri).await?;
    source::validate_yaml(&content)?;
    let root = root.to_path_buf();
    crate::runtime::spawn_blocking(move || update_entry_with_content(&root, &entry, &content))
        .await
        .map_err(|error| format!("更新配置任务失败：{error}"))?
}

fn update_entry_without_core(
    root: &Path,
    entry: &ConfigEntry,
    content: &str,
) -> Result<(), String> {
    let target = source::safe_internal_path(root, CONFIGS_DIR, &entry.path)?;
    source::write_internal(&target, content)
}

fn update_entry_with_content(
    root: &Path,
    entry: &ConfigEntry,
    content: &str,
) -> Result<(), String> {
    update_entry_without_core(root, entry, content)?;
    if entry.enabled {
        core::on_config_changed(root)
            .map_err(|error| format!("配置已更新，但核心联动失败：{error}"))?;
    }
    Ok(())
}

fn delete_entry(root: &Path, path: &str) -> Result<(), String> {
    let entry = config::configs()
        .into_iter()
        .find(|entry| entry.path == path)
        .ok_or_else(|| "未找到要删除的配置".to_string())?;
    let target = source::safe_internal_path(root, CONFIGS_DIR, &entry.path)?;
    if entry.enabled {
        core::stop_core().map_err(|error| format!("停止核心失败：{error}"))?;
    }
    if target.exists() {
        fs::remove_file(&target).map_err(|error| format!("删除内部副本失败：{error}"))?;
    }
    config::update(|current| current.configs.retain(|current| current.path != path));
    if entry.enabled {
        core::on_config_changed(root)
            .map_err(|error| format!("配置已删除，但核心状态刷新失败：{error}"))?;
    }
    Ok(())
}
