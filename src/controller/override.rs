// 覆写页面控制器：配置快照、异步状态、排序预览提交与 Slint 回写。

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use slint::{ComponentHandle, ModelRc, VecModel, Weak};

use crate::app::config::{self, OverrideEntry};
use crate::clash::core;
use crate::constants::OVERRIDES_DIR;
use crate::controller::{home, source};
use crate::{platform, MainWindow, OverrideModel, OverrideRow};

#[derive(Debug)]
pub struct OverrideViewState {
    pub(crate) root: PathBuf,
    pub(crate) loading: bool,
    pub(crate) busy: bool,
    pub(crate) error: String,
    pub(crate) next_token: u64,
    pub(crate) refresh_token: u64,
    pub(crate) operation_token: u64,
}

pub type SharedOverrideState = Arc<Mutex<OverrideViewState>>;

pub(crate) fn bind_callbacks(window: &MainWindow, state: SharedOverrideState) {
    bind_item_callbacks(window, state.clone());
    bind_form_callbacks(window, state);
}

fn bind_item_callbacks(window: &MainWindow, state: SharedOverrideState) {
    let weak = window.as_weak();
    window.global::<OverrideModel>().on_toggle_enabled({
        let weak = weak.clone();
        let state = state.clone();
        move |path| toggle_enabled(weak.clone(), state.clone(), path.into())
    });
    window.global::<OverrideModel>().on_reorder({
        let weak = weak.clone();
        let state = state.clone();
        move |path, target| reorder(weak.clone(), state.clone(), path.into(), target)
    });
    window.global::<OverrideModel>().on_edit({
        let weak = weak.clone();
        let state = state.clone();
        move |path| edit(weak.clone(), state.clone(), path.into())
    });
    window.global::<OverrideModel>().on_update({
        let weak = weak.clone();
        let state = state.clone();
        move |path| update(weak.clone(), state.clone(), path.into())
    });
    window.global::<OverrideModel>().on_delete({
        let weak = weak.clone();
        let state = state.clone();
        move |path| delete(weak.clone(), state.clone(), path.into())
    });
    window.global::<OverrideModel>().on_open_add({
        let weak = weak.clone();
        move || open_add(weak.clone())
    });
    window.global::<OverrideModel>().on_choose_file({
        let weak = weak.clone();
        move || choose_file(weak.clone(), state.clone())
    });
}

fn bind_form_callbacks(window: &MainWindow, state: SharedOverrideState) {
    let weak = window.as_weak();
    window.global::<OverrideModel>().on_submit_form({
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
    window.global::<OverrideModel>().on_cancel_form({
        let weak = weak.clone();
        move || cancel_form(weak.clone())
    });
    window.global::<OverrideModel>().on_update_all({
        let weak = weak.clone();
        move || update_all(weak.clone(), state.clone())
    });
}

fn lock_state(state: &SharedOverrideState) -> MutexGuard<'_, OverrideViewState> {
    state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub fn new_state(root: PathBuf) -> SharedOverrideState {
    Arc::new(Mutex::new(OverrideViewState {
        root,
        loading: false,
        busy: false,
        error: String::new(),
        next_token: 0,
        refresh_token: 0,
        operation_token: 0,
    }))
}

fn next_token(state: &mut OverrideViewState) -> u64 {
    state.next_token = state.next_token.wrapping_add(1).max(1);
    state.next_token
}

fn display_name(entry: &OverrideEntry) -> String {
    if !entry.name.trim().is_empty() {
        return entry.name.clone();
    }
    source::fallback_name(&entry.source_uri)
}

fn sorted_entries(entries: &[OverrideEntry]) -> Vec<OverrideEntry> {
    let mut entries = entries.to_vec();
    entries.sort_by_key(|entry| (!entry.enabled, entry.sort));
    entries
}

fn next_sort(entries: &[OverrideEntry]) -> i64 {
    entries
        .iter()
        .map(|entry| entry.sort)
        .max()
        .unwrap_or(-1)
        .saturating_add(1)
}

fn normalize_entries(entries: &[OverrideEntry]) -> Vec<OverrideEntry> {
    let mut entries = sorted_entries(entries);
    for (index, entry) in entries.iter_mut().enumerate() {
        entry.sort = index as i64;
    }
    entries
}

fn enabled_paths(entries: &[OverrideEntry]) -> Vec<String> {
    sorted_entries(entries)
        .into_iter()
        .filter(|entry| entry.enabled)
        .map(|entry| entry.path)
        .collect()
}

fn enabled_order_changed(before: &[OverrideEntry], after: &[OverrideEntry]) -> bool {
    enabled_paths(before) != enabled_paths(after)
}

fn toggle_snapshot(
    entries: &[OverrideEntry],
    path: &str,
) -> Result<(Vec<OverrideEntry>, bool), String> {
    let mut entries = entries.to_vec();
    let enabled = {
        let entry = entries
            .iter_mut()
            .find(|entry| entry.path == path)
            .ok_or_else(|| "未找到要切换的覆写".to_string())?;
        entry.enabled = !entry.enabled;
        entry.enabled
    };
    Ok((normalize_entries(&entries), enabled))
}

fn build_entry(
    existing: Option<&OverrideEntry>,
    name: &str,
    source_type: crate::app::config::SourceType,
    source_uri: &str,
    path: String,
    append_sort: i64,
) -> OverrideEntry {
    OverrideEntry {
        name: if name.trim().is_empty() {
            source::fallback_name(source_uri)
        } else {
            name.trim().to_string()
        },
        enabled: existing.map(|entry| entry.enabled).unwrap_or(false),
        source_type,
        source_uri: source_uri.to_string(),
        sort: existing.map(|entry| entry.sort).unwrap_or(append_sort),
        path,
    }
}

fn remove_and_normalize(
    entries: &[OverrideEntry],
    path: &str,
) -> Result<(OverrideEntry, Vec<OverrideEntry>), String> {
    let removed = entries
        .iter()
        .find(|entry| entry.path == path)
        .cloned()
        .ok_or_else(|| "未找到要删除的覆写".to_string())?;
    let remaining = entries
        .iter()
        .filter(|entry| entry.path != path)
        .cloned()
        .collect::<Vec<_>>();
    Ok((removed, normalize_entries(&remaining)))
}

fn override_rows(entries: &[OverrideEntry]) -> Vec<OverrideRow> {
    sorted_entries(entries)
        .iter()
        .map(|entry| OverrideRow {
            name: display_name(entry).into(),
            enabled: entry.enabled,
            source_type: source::source_type_name(entry.source_type).into(),
            source_uri: entry.source_uri.clone().into(),
            sort: entry.sort as i32,
            path: entry.path.clone().into(),
        })
        .collect()
}

fn set_ui_model(window: &MainWindow, state: &OverrideViewState, entries: &[OverrideEntry]) {
    let model = window.global::<OverrideModel>();
    model.set_overrides(ModelRc::new(VecModel::from(override_rows(entries))));
    model.set_loading(state.loading);
    model.set_busy(state.busy);
    model.set_error(state.error.clone().into());
}

fn set_toast(window: &MainWindow, message: &str, variant: i32) {
    let model = window.global::<OverrideModel>();
    model.set_toast_message(message.to_string().into());
    model.set_toast_variant(variant);
    model.set_toast_visible(true);
}

fn invoke_ui<F>(callback: F)
where
    F: FnOnce() + Send + 'static,
{
    if let Err(error) = slint::invoke_from_event_loop(callback) {
        crate::log::error(format_args!("覆写页 UI 回调失败：{error}"));
    }
}

/// 从 app.yaml 快照刷新覆写卡片，不轮询外部服务。
pub fn refresh_async(weak: Weak<MainWindow>, state: SharedOverrideState) {
    let token = {
        let mut view = lock_state(&state);
        if view.busy {
            return;
        }
        let token = next_token(&mut view);
        view.refresh_token = token;
        view.loading = true;
        view.error.clear();
        if let Some(window) = weak.upgrade() {
            set_ui_model(&window, &view, &config::get().overrides);
        }
        token
    };

    std::thread::spawn(move || {
        let snapshot = config::get();
        invoke_ui(move || {
            let Some(window) = weak.upgrade() else { return };
            let mut view = lock_state(&state);
            if view.refresh_token != token {
                return;
            }
            view.loading = false;
            set_ui_model(&window, &view, &snapshot.overrides);
        });
    });
}

fn begin_operation(weak: &Weak<MainWindow>, state: &SharedOverrideState) -> Option<u64> {
    let mut view = lock_state(state);
    if view.busy {
        return None;
    }
    let token = next_token(&mut view);
    view.operation_token = token;
    view.refresh_token = token;
    view.loading = false;
    view.busy = true;
    view.error.clear();
    if let Some(window) = weak.upgrade() {
        set_ui_model(&window, &view, &config::get().overrides);
    }
    Some(token)
}

pub(crate) fn finish_operation(
    weak: Weak<MainWindow>,
    state: SharedOverrideState,
    token: u64,
    result: Result<String, String>,
    close_form: bool,
) {
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
                    window.global::<OverrideModel>().set_form_open(false);
                }
                set_toast(&window, &message, 1);
            }
            Err(message) => {
                view.error = message.clone();
                set_toast(&window, &message, 2);
            }
        }
        set_ui_model(&window, &view, &config::get().overrides);
        home::refresh_runtime_state(&window);
        if !success && close_form {
            window.global::<OverrideModel>().set_form_open(true);
        }
    });
}

/// 切换单条覆写启用状态，不影响其他覆写。
pub fn toggle_enabled(weak: Weak<MainWindow>, state: SharedOverrideState, path: String) {
    let Some(token) = begin_operation(&weak, &state) else {
        return;
    };
    let root = lock_state(&state).root.clone();
    std::thread::spawn(move || {
        let snapshot = config::get();
        let (updated, enabled) = match toggle_snapshot(&snapshot.overrides, &path) {
            Ok(updated) => updated,
            Err(error) => {
                finish_operation(weak, state, token, Err(error), false);
                return;
            }
        };
        config::update(|current| current.overrides = updated);
        let result = core::on_config_changed(&root)
            .map(|_| {
                if enabled {
                    "已启用覆写"
                } else {
                    "已禁用覆写"
                }
                .to_string()
            })
            .map_err(|error| format!("覆写已保存，但核心联动失败：{error}"));
        finish_operation(weak, state, token, result, false);
    });
}

fn reorder_snapshot(
    entries: &[OverrideEntry],
    path: &str,
    target_index: i32,
) -> Result<Option<Vec<OverrideEntry>>, String> {
    let mut entries = sorted_entries(entries);
    let source_index = entries
        .iter()
        .position(|entry| entry.path == path)
        .ok_or_else(|| "未找到要重排的覆写".to_string())?;
    if target_index < 0 || target_index as usize >= entries.len() {
        return Err("覆写目标位置无效".to_string());
    }
    let target_index = target_index as usize;
    let enabled_count = entries.iter().filter(|entry| entry.enabled).count();
    let target_index = if entries[source_index].enabled {
        target_index.min(enabled_count.saturating_sub(1))
    } else {
        target_index.max(enabled_count).min(entries.len() - 1)
    };
    if source_index == target_index {
        return Ok(None);
    }
    let entry = entries.remove(source_index);
    entries.insert(target_index, entry);
    Ok(Some(normalize_entries(&entries)))
}

/// 按视觉顺序重排覆写并规范化 sort；有效变更只联动核心一次。
pub fn reorder(weak: Weak<MainWindow>, state: SharedOverrideState, path: String, target: i32) {
    let Some(token) = begin_operation(&weak, &state) else {
        return;
    };
    let root = lock_state(&state).root.clone();
    std::thread::spawn(move || {
        let snapshot = config::get();
        let reordered = match reorder_snapshot(&snapshot.overrides, &path, target) {
            Ok(reordered) => reordered,
            Err(error) => {
                finish_operation(weak, state, token, Err(error), false);
                return;
            }
        };
        let Some(reordered) = reordered else {
            finish_operation(weak, state, token, Ok("覆写顺序未改变".to_string()), false);
            return;
        };
        let core_changed = enabled_order_changed(&snapshot.overrides, &reordered);
        config::update(|current| current.overrides = reordered);
        let result = if core_changed {
            core::on_config_changed(&root)
                .map(|_| "覆写顺序已更新".to_string())
                .map_err(|error| format!("覆写顺序已保存，但核心联动失败：{error}"))
        } else {
            Ok("覆写顺序已更新".to_string())
        };
        finish_operation(weak, state, token, result, false);
    });
}

/// 提交添加或编辑表单。
pub fn submit_form(
    weak: Weak<MainWindow>,
    state: SharedOverrideState,
    editing_path: String,
    name: String,
    source_type: String,
    source_uri: String,
) {
    let Some(token) = begin_operation(&weak, &state) else {
        return;
    };
    let root = lock_state(&state).root.clone();
    std::thread::spawn(move || {
        let result = save_entry(&root, &editing_path, &name, &source_type, &source_uri)
            .map(|_| "覆写已保存".to_string())
            .map_err(|error| format!("保存覆写失败：{error}"));
        finish_operation(weak, state, token, result, true);
    });
}

/// 更新单条覆写源内容；读取或校验失败时保留旧副本。
pub fn update(weak: Weak<MainWindow>, state: SharedOverrideState, path: String) {
    let Some(token) = begin_operation(&weak, &state) else {
        return;
    };
    let root = lock_state(&state).root.clone();
    std::thread::spawn(move || {
        let result = update_entry(&root, &path)
            .map(|_| "覆写已更新".to_string())
            .map_err(|error| format!("更新覆写失败：{error}"));
        finish_operation(weak, state, token, result, false);
    });
}

/// 删除单条覆写及其内部副本，不触碰用户源文件。
pub fn delete(weak: Weak<MainWindow>, state: SharedOverrideState, path: String) {
    let Some(token) = begin_operation(&weak, &state) else {
        return;
    };
    let root = lock_state(&state).root.clone();
    std::thread::spawn(move || {
        let result = delete_entry(&root, &path)
            .map(|_| "覆写已删除".to_string())
            .map_err(|error| format!("删除覆写失败：{error}"));
        finish_operation(weak, state, token, result, false);
    });
}

/// 按视觉顺序串行更新全部覆写，失败项保留旧副本并汇总错误。
pub fn update_all(weak: Weak<MainWindow>, state: SharedOverrideState) {
    let Some(token) = begin_operation(&weak, &state) else {
        return;
    };
    let root = lock_state(&state).root.clone();
    std::thread::spawn(move || {
        let entries = sorted_entries(&config::get().overrides);
        let mut failures = Vec::new();
        let mut enabled_updated = false;
        for entry in entries {
            match update_entry_without_core(&root, &entry) {
                Ok(()) => enabled_updated |= entry.enabled,
                Err(error) => failures.push(format!("{}：{error}", display_name(&entry))),
            }
        }
        if enabled_updated {
            if let Err(error) = core::on_config_changed(&root) {
                failures.push(format!("核心联动失败：{error}"));
            }
        }
        let result = if failures.is_empty() {
            Ok("已更新全部覆写".to_string())
        } else {
            Err(format!("部分覆写更新失败：{}", failures.join("；")))
        };
        finish_operation(weak, state, token, result, false);
    });
}

/// 打开平台文件选择器并回写表单源路径。
pub fn choose_file(weak: Weak<MainWindow>, state: SharedOverrideState) {
    let Some(token) = begin_operation(&weak, &state) else {
        return;
    };
    std::thread::spawn(move || {
        let result = platform::pick_config_file().and_then(|path| match path {
            Some(path) if path.is_file() => Ok(Some(path)),
            Some(_) => Err("选择的路径不是文件".to_string()),
            None => Ok(None),
        });
        invoke_ui(move || {
            let Some(window) = weak.upgrade() else { return };
            let mut view = lock_state(&state);
            if view.operation_token != token {
                return;
            }
            view.busy = false;
            match result {
                Ok(Some(path)) => {
                    let model = window.global::<OverrideModel>();
                    model.set_form_type_index(1);
                    model.set_form_source(path.to_string_lossy().to_string().into());
                }
                Ok(None) => {}
                Err(error) => {
                    view.error = error.clone();
                    set_toast(&window, &error, 2);
                }
            }
            set_ui_model(&window, &view, &config::get().overrides);
        });
    });
}

/// 打开添加表单并清空上一条编辑状态。
pub fn open_add(weak: Weak<MainWindow>) {
    let Some(window) = weak.upgrade() else { return };
    let model = window.global::<OverrideModel>();
    model.set_form_editing_path("".into());
    model.set_form_name("".into());
    model.set_form_type_index(1);
    model.set_form_source("".into());
    model.set_form_open(true);
}

/// 取消添加/编辑表单。
pub fn cancel_form(weak: Weak<MainWindow>) {
    if let Some(window) = weak.upgrade() {
        window.global::<OverrideModel>().set_form_open(false);
    }
}

/// 将现有覆写带入编辑表单。
pub fn edit(weak: Weak<MainWindow>, state: SharedOverrideState, path: String) {
    if lock_state(&state).busy {
        return;
    }
    let entry = config::get()
        .overrides
        .into_iter()
        .find(|entry| entry.path == path);
    let Some(entry) = entry else {
        if let Some(window) = weak.upgrade() {
            window
                .global::<OverrideModel>()
                .set_error("未找到要编辑的覆写".into());
        }
        return;
    };
    if let Some(window) = weak.upgrade() {
        let model = window.global::<OverrideModel>();
        model.set_form_editing_path(entry.path.into());
        model.set_form_name(entry.name.into());
        model.set_form_type_index(
            if entry.source_type == crate::app::config::SourceType::Http {
                0
            } else {
                1
            },
        );
        model.set_form_source(entry.source_uri.into());
        model.set_form_open(true);
    }
}

fn save_entry(
    root: &std::path::Path,
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
    let snapshot = config::get();
    source::ensure_unique_source_uri(
        snapshot
            .overrides
            .iter()
            .map(|entry| (entry.path.as_str(), entry.source_uri.as_str())),
        editing_path,
        source_uri,
        "覆写",
    )?;
    let content = source::read_source(source_type, source_uri)?;
    source::validate_yaml(&content)?;

    let existing = if editing_path.is_empty() {
        None
    } else {
        Some(
            snapshot
                .overrides
                .iter()
                .find(|entry| entry.path == editing_path)
                .ok_or_else(|| "未找到要编辑的覆写".to_string())?,
        )
    };
    let internal_path = if let Some(entry) = existing {
        source::safe_internal_path(root, OVERRIDES_DIR, &entry.path)?
    } else {
        source::unique_internal_path(root, OVERRIDES_DIR, source_uri)?
    };
    source::write_internal(&internal_path, &content)?;

    let entry = build_entry(
        existing,
        name,
        source_type,
        source_uri,
        source::relative_internal_path(root, &internal_path),
        next_sort(&snapshot.overrides),
    );
    config::update(|current| {
        if let Some(index) = current
            .overrides
            .iter()
            .position(|current| current.path == editing_path && !editing_path.is_empty())
        {
            current.overrides[index] = entry;
        } else {
            current.overrides.push(entry);
        }
    });

    if existing.map(|entry| entry.enabled).unwrap_or(false) {
        core::on_config_changed(root)
            .map_err(|error| format!("覆写已保存，但核心联动失败：{error}"))?;
    }
    Ok(())
}

fn update_entry(root: &std::path::Path, path: &str) -> Result<(), String> {
    let entry = config::get()
        .overrides
        .into_iter()
        .find(|entry| entry.path == path)
        .ok_or_else(|| "未找到要更新的覆写".to_string())?;
    update_entry_without_core(root, &entry)?;
    if entry.enabled {
        core::on_config_changed(root)
            .map_err(|error| format!("覆写已更新，但核心联动失败：{error}"))?;
    }
    Ok(())
}

fn update_entry_without_core(root: &std::path::Path, entry: &OverrideEntry) -> Result<(), String> {
    let content = source::read_source(entry.source_type, &entry.source_uri)?;
    source::validate_yaml(&content)?;
    let target = source::safe_internal_path(root, OVERRIDES_DIR, &entry.path)?;
    source::write_internal(&target, &content)
}

fn delete_entry(root: &std::path::Path, path: &str) -> Result<(), String> {
    let snapshot = config::get();
    let (entry, remaining) = remove_and_normalize(&snapshot.overrides, path)?;
    let target = source::safe_internal_path(root, OVERRIDES_DIR, &entry.path)?;
    if target.exists() {
        std::fs::remove_file(&target).map_err(|error| format!("删除内部副本失败：{error}"))?;
    }

    config::update(|current| current.overrides = remaining);
    if entry.enabled {
        core::on_config_changed(root)
            .map_err(|error| format!("覆写已删除，但核心状态刷新失败：{error}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        build_entry, display_name, enabled_order_changed, next_sort, override_rows,
        remove_and_normalize, reorder_snapshot, sorted_entries, toggle_snapshot,
    };
    use crate::app::config::{OverrideEntry, SourceType};

    fn entry(path: &str, sort: i64) -> OverrideEntry {
        OverrideEntry {
            name: String::new(),
            enabled: false,
            source_type: SourceType::File,
            source_uri: format!("{path}.yaml"),
            sort,
            path: path.to_string(),
        }
    }

    #[test]
    fn shows_overrides_stably_by_ascending_sort() {
        let entries = vec![entry("first", 2), entry("second", 1), entry("third", 1)];
        let sorted = sorted_entries(&entries);
        assert_eq!(sorted[0].path, "second");
        assert_eq!(sorted[1].path, "third");
        assert_eq!(sorted[2].path, "first");
        assert_eq!(display_name(&sorted[0]), "second.yaml");
        assert_eq!(override_rows(&entries)[0].sort, 1);
    }

    #[test]
    fn shows_enabled_overrides_before_disabled_overrides() {
        let disabled = entry("disabled", 0);
        let mut enabled = entry("enabled", 9);
        enabled.enabled = true;
        let mut same_sort = entry("same-sort", 9);
        same_sort.enabled = true;

        let sorted = sorted_entries(&[disabled.clone(), enabled, same_sort]);
        assert_eq!(sorted[0].path, "enabled");
        assert_eq!(sorted[1].path, "same-sort");
        assert_eq!(sorted[2].path, "disabled");

        let mut other = entry("other", 0);
        other.enabled = true;
        let candidate = entry("candidate", 9);
        let (toggled, _) = toggle_snapshot(&[candidate, other], "candidate").unwrap();
        assert_eq!(toggled[0].path, "other");
        assert_eq!(toggled[1].path, "candidate");
        assert_eq!(toggled[0].sort, 0);
        assert_eq!(toggled[1].sort, 1);
    }

    #[test]
    fn toggling_one_override_does_not_affect_others() {
        let mut first = entry("first", 0);
        first.enabled = true;
        let second = entry("second", 1);
        let (updated, enabled) = toggle_snapshot(&[first, second], "second").unwrap();
        assert!(enabled);
        assert!(updated[0].enabled);
        assert!(updated[1].enabled);
    }

    #[test]
    fn normalizes_sort_after_reordering() {
        let entries = vec![entry("first", 9), entry("second", 9), entry("third", 2)];
        let reordered = reorder_snapshot(&entries, "first", 2).unwrap().unwrap();
        assert_eq!(
            reordered
                .iter()
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>(),
            vec!["third", "second", "first"]
        );
        assert_eq!(
            reordered.iter().map(|entry| entry.sort).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
    }

    #[test]
    fn disabled_only_reordering_does_not_change_enabled_order() {
        let mut enabled = entry("enabled", 0);
        enabled.enabled = true;
        let entries = vec![enabled, entry("disabled-a", 1), entry("disabled-b", 1)];
        let reordered = reorder_snapshot(&entries, "disabled-a", 2)
            .unwrap()
            .unwrap();
        assert!(!enabled_order_changed(&entries, &reordered));
    }

    #[test]
    fn enabled_reordering_changes_enabled_order() {
        let mut first = entry("first", 0);
        first.enabled = true;
        let mut second = entry("second", 0);
        second.enabled = true;
        let entries = vec![first, second, entry("disabled", 2)];
        let reordered = reorder_snapshot(&entries, "first", 1).unwrap().unwrap();
        assert!(enabled_order_changed(&entries, &reordered));
    }

    #[test]
    fn appends_new_sort_to_visual_end() {
        let entries = vec![entry("first", -2), entry("second", 7)];
        assert_eq!(next_sort(&entries), 8);
        assert_eq!(next_sort(&[]), 0);
    }

    #[test]
    fn normalizes_sort_after_deletion() {
        let mut removed = entry("removed", 8);
        removed.enabled = true;
        let entries = vec![removed, entry("first", 20), entry("second", 3)];
        let (removed, remaining) = remove_and_normalize(&entries, "removed").unwrap();
        assert!(removed.enabled);
        assert_eq!(remaining[0].path, "second");
        assert_eq!(remaining[0].sort, 0);
        assert_eq!(remaining[1].sort, 1);
    }

    #[test]
    fn editing_preserves_enabled_sort_and_path_identity() {
        let mut existing = entry("stable", 6);
        existing.enabled = true;
        let updated = build_entry(
            Some(&existing),
            "新名称",
            SourceType::Http,
            "https://example.com/new.yaml",
            existing.path.clone(),
            99,
        );
        assert_eq!(updated.name, "新名称");
        assert_eq!(updated.source_type, SourceType::Http);
        assert_eq!(updated.source_uri, "https://example.com/new.yaml");
        assert!(updated.enabled);
        assert_eq!(updated.sort, 6);
        assert_eq!(updated.path, "stable");
    }
}
