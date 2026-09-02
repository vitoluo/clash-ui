// 规则页数据获取、排序和 UI 状态更新。

use std::sync::{Arc, Mutex, MutexGuard};

use slint::{ComponentHandle, Model, ModelRc, VecModel, Weak};

use crate::clash::api::{self, ApiError, RuleEntry};
use crate::event::CoreState;
use crate::{MainWindow, TableRow};

#[derive(Debug, Default)]
pub struct RulesViewState {
    rules: Vec<RuleEntry>,
    loading: bool,
    error: String,
    core_state: CoreState,
    next_token: u64,
    refresh_token: u64,
    loaded_generation: Option<u64>,
    loading_generation: Option<u64>,
}

pub type SharedRulesState = Arc<Mutex<RulesViewState>>;

fn lock_state(state: &SharedRulesState) -> MutexGuard<'_, RulesViewState> {
    state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub fn new_state() -> SharedRulesState {
    Arc::new(Mutex::new(RulesViewState::default()))
}

pub(crate) fn listen_core_state(window: &MainWindow, state: SharedRulesState) {
    let weak = window.as_weak();
    let mut core_state = crate::event::subscribe_core_state();
    core_state.mark_changed();
    crate::runtime::spawn_task(async move {
        let mut previous_state = CoreState::default();
        loop {
            if core_state.changed().await.is_err() {
                return;
            }
            let current_state = *core_state.borrow_and_update();
            let should_clear =
                !current_state.running || current_state.generation != previous_state.generation;
            previous_state = current_state;
            if should_clear {
                clear_runtime(&state, current_state);
                let weak = weak.clone();
                let state = state.clone();
                if let Err(error) = slint::invoke_from_event_loop(move || {
                    if let Some(window) = weak.upgrade() {
                        sync_ui(&window, &state);
                    }
                }) {
                    crate::log::error(format_args!("核心状态变化后清理规则页数据失败：{error}"));
                }
            }
            if current_state.running {
                refresh_async(weak.clone(), state.clone(), current_state);
            }
        }
    });
}

/// 清空核心运行期间的规则数据，并使未完成请求失效。
pub fn clear_runtime(state: &SharedRulesState, core_state: CoreState) {
    let mut view = lock_state(state);
    view.rules.clear();
    view.loading = false;
    view.error.clear();
    view.core_state = core_state;
    view.loaded_generation = None;
    view.loading_generation = None;
    view.refresh_token = next_token(&mut view);
}

fn next_token(state: &mut RulesViewState) -> u64 {
    state.next_token = state.next_token.wrapping_add(1).max(1);
    state.next_token
}

fn sort_rules(mut rules: Vec<RuleEntry>) -> Vec<RuleEntry> {
    rules.sort_by_key(|rule| rule.index);
    rules
}

impl RulesViewState {
    fn to_slint_rules(&self) -> Vec<TableRow> {
        self.rules
            .iter()
            .map(|rule| TableRow {
                id: format!("rule-{}", rule.index).into(),
                cells: ModelRc::new(VecModel::from(vec![
                    rule.payload.clone().into(),
                    rule.type_.clone().into(),
                    rule.proxy.clone().into(),
                ])),
                secondary_content: "".into(),
            })
            .collect()
    }
}

fn set_ui_model(window: &MainWindow, state: &RulesViewState) {
    let model = window.global::<crate::RulesModel>();
    let rules = state.to_slint_rules();
    let current = model.get_rules();
    if let Some(rules_model) = current.as_any().downcast_ref::<VecModel<TableRow>>() {
        sync_rules_model(rules_model, rules);
    } else {
        model.set_rules(ModelRc::new(VecModel::from(rules)));
    }
    set_ui_state(window, state);
}

fn set_ui_state(window: &MainWindow, state: &RulesViewState) {
    let model = window.global::<crate::RulesModel>();
    model.set_loading(state.loading);
    model.set_error(state.error.clone().into());
}

fn sync_cells(model: &VecModel<slint::SharedString>, values: Vec<slint::SharedString>) {
    let common = model.row_count().min(values.len());
    for (index, value) in values.iter().take(common).cloned().enumerate() {
        if model.row_data(index).as_ref() != Some(&value) {
            model.set_row_data(index, value);
        }
    }
    while model.row_count() > values.len() {
        model.remove(model.row_count() - 1);
    }
    for value in values.into_iter().skip(common) {
        model.push(value);
    }
}

fn sync_rules_model(model: &VecModel<TableRow>, rules: Vec<TableRow>) {
    let common = model.row_count().min(rules.len());
    for index in 0..common {
        let Some(mut next) = rules.get(index).cloned() else {
            continue;
        };
        if let Some(current) = model.row_data(index) {
            if let (Some(current_cells), Some(next_cells)) = (
                current
                    .cells
                    .as_any()
                    .downcast_ref::<VecModel<slint::SharedString>>(),
                next.cells
                    .as_any()
                    .downcast_ref::<VecModel<slint::SharedString>>(),
            ) {
                let values = (0..next_cells.row_count())
                    .filter_map(|cell| next_cells.row_data(cell))
                    .collect();
                sync_cells(current_cells, values);
                next.cells = current.cells.clone();
            }
            if current.id == next.id && current.secondary_content == next.secondary_content {
                continue;
            }
        }
        model.set_row_data(index, next);
    }
    while model.row_count() > rules.len() {
        model.remove(model.row_count() - 1);
    }
    for rule in rules.into_iter().skip(common) {
        model.push(rule);
    }
}

pub fn sync_ui(window: &MainWindow, state: &SharedRulesState) {
    let state = lock_state(state);
    set_ui_model(window, &state);
}

fn invoke_ui<F>(callback: F)
where
    F: FnOnce() + Send + 'static,
{
    if let Err(error) = slint::invoke_from_event_loop(callback) {
        crate::log::error(format_args!("规则页 UI 回调失败: {error}"));
    }
}

pub fn refresh_async(weak: Weak<MainWindow>, state: SharedRulesState, core_state: CoreState) {
    if !core_state.running {
        if let Some(window) = weak.upgrade() {
            let view = lock_state(&state);
            set_ui_model(&window, &view);
        }
        return;
    }
    let generation = core_state.generation;
    let token = {
        let mut view = lock_state(&state);
        if view.core_state != core_state {
            view.core_state = core_state;
            view.loaded_generation = None;
            view.loading_generation = None;
        }
        if view.loaded_generation == Some(generation) || view.loading_generation == Some(generation)
        {
            if let Some(window) = weak.upgrade() {
                set_ui_state(&window, &view);
            }
            return;
        }
        let token = next_token(&mut view);
        view.refresh_token = token;
        view.error.clear();
        view.loading = true;
        view.loading_generation = Some(generation);
        if let Some(window) = weak.upgrade() {
            set_ui_state(&window, &view);
        }
        token
    };

    let worker_state = state.clone();
    crate::runtime::spawn_task(async move {
        let result = api::get_rules().await.map(sort_rules);
        if let Err(error) = &result {
            crate::log::error(format_args!("加载规则数据失败：{error}"));
        }
        invoke_ui(move || {
            let Some(window) = weak.upgrade() else { return };
            let mut view = lock_state(&worker_state);
            if view.refresh_token != token
                || view.loading_generation != Some(generation)
                || view.core_state != core_state
                || *crate::event::subscribe_core_state().borrow() != core_state
            {
                return;
            }
            match result {
                Ok(rules) => {
                    view.rules = rules;
                    view.error.clear();
                    view.loaded_generation = Some(generation);
                    view.loading_generation = None;
                    view.loading = false;
                    set_ui_model(&window, &view);
                }
                Err(error) => {
                    view.error = format_error("加载规则数据失败", &error);
                    view.loading_generation = None;
                    view.loading = false;
                    set_ui_state(&window, &view);
                }
            }
        });
    });
}

fn format_error(prefix: &str, error: &ApiError) -> String {
    format!("{prefix}：{error}")
}

#[cfg(test)]
mod tests {
    use super::{clear_runtime, new_state, sort_rules, sync_rules_model};
    use crate::clash::api::RuleEntry;
    use crate::event::CoreState;
    use crate::TableRow;
    use slint::{Model, ModelRc, SharedString, VecModel};
    use std::rc::Rc;

    fn table_row(id: &str, payload: &str) -> TableRow {
        TableRow {
            id: id.into(),
            cells: ModelRc::new(VecModel::from(vec![SharedString::from(payload)])),
            secondary_content: "".into(),
        }
    }

    #[test]
    fn sorts_rules_by_index() {
        let build = |index: usize, payload: &str| RuleEntry {
            index,
            type_: "DOMAIN".to_string(),
            payload: payload.to_string(),
            proxy: "DIRECT".to_string(),
            size: 0,
            extra: None,
        };
        let rules = sort_rules(vec![build(2, "two"), build(0, "zero"), build(1, "one")]);
        assert_eq!(
            rules
                .into_iter()
                .map(|rule| rule.payload)
                .collect::<Vec<_>>(),
            vec!["zero".to_string(), "one".to_string(), "two".to_string()]
        );
    }

    #[test]
    fn clear_runtime_removes_rules_and_invalidates_refresh() {
        let state = new_state();
        let previous_token = {
            let mut view = state.lock().unwrap();
            view.rules.push(RuleEntry {
                index: 0,
                type_: "DOMAIN".to_string(),
                payload: "example.com".to_string(),
                proxy: "DIRECT".to_string(),
                size: 1,
                extra: None,
            });
            view.loading = true;
            view.error = "旧错误".to_string();
            view.loaded_generation = Some(7);
            view.refresh_token
        };

        clear_runtime(
            &state,
            CoreState {
                running: false,
                generation: 2,
            },
        );

        let view = state.lock().unwrap();
        assert!(view.rules.is_empty());
        assert!(!view.loading);
        assert!(view.error.is_empty());
        assert!(view.loaded_generation.is_none());
        assert_ne!(view.refresh_token, previous_token);
    }

    #[test]
    fn stable_rule_model_reuses_top_level_and_cell_models() {
        let cells = Rc::new(VecModel::from(vec![SharedString::from("旧规则")]));
        let rows = Rc::new(VecModel::from(vec![TableRow {
            id: "rule-0".into(),
            cells: ModelRc::from(cells.clone()),
            secondary_content: "".into(),
        }]));
        let identity = ModelRc::from(rows.clone());

        sync_rules_model(
            &rows,
            vec![table_row("rule-0", "新规则"), table_row("rule-1", "第二条")],
        );

        assert_eq!(identity, ModelRc::from(rows.clone()));
        assert_eq!(rows.row_count(), 2);
        assert_eq!(
            rows.row_data(0).unwrap().cells,
            ModelRc::from(cells.clone())
        );
        assert_eq!(cells.row_data(0), Some(SharedString::from("新规则")));
    }
}
