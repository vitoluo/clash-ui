// 规则页数据获取、排序和 UI 状态更新。

use std::sync::{Arc, Mutex, MutexGuard};

use slint::{ComponentHandle, ModelRc, VecModel, Weak};

use crate::clash::api::{self, ApiError, RuleEntry};
use crate::{MainWindow, TableRow};

#[derive(Debug, Default)]
pub struct RulesViewState {
    rules: Vec<RuleEntry>,
    loading: bool,
    error: String,
    next_token: u64,
    refresh_token: u64,
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

/// 清空核心运行期间的规则数据，并使未完成请求失效。
pub fn clear_runtime(state: &SharedRulesState) {
    let mut view = lock_state(state);
    view.rules.clear();
    view.loading = false;
    view.error.clear();
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
                secondary_cells: ModelRc::new(VecModel::from(vec![
                    "".into(),
                    "".into(),
                    "".into(),
                ])),
            })
            .collect()
    }
}

fn set_ui_model(window: &MainWindow, state: &RulesViewState) {
    let model = window.global::<crate::RulesModel>();
    model.set_rules(ModelRc::new(VecModel::from(state.to_slint_rules())));
    model.set_loading(state.loading);
    model.set_error(state.error.clone().into());
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

pub fn refresh_async(weak: Weak<MainWindow>, state: SharedRulesState) {
    let token = {
        let mut view = lock_state(&state);
        let token = next_token(&mut view);
        view.refresh_token = token;
        view.error.clear();
        view.loading = true;
        if let Some(window) = weak.upgrade() {
            set_ui_model(&window, &view);
        }
        token
    };

    let fallback_weak = weak.clone();
    let worker_state = state.clone();
    let spawn_result = std::thread::Builder::new()
        .name("rules-refresh".to_string())
        .spawn(move || {
            let result = api::get_rules().map(sort_rules);
            invoke_ui(move || {
                let Some(window) = weak.upgrade() else { return };
                let mut view = lock_state(&worker_state);
                if view.refresh_token != token {
                    return;
                }
                match result {
                    Ok(rules) => {
                        view.rules = rules;
                        view.error.clear();
                    }
                    Err(error) => {
                        view.error = format_error("加载规则数据失败", &error);
                    }
                }
                view.loading = false;
                set_ui_model(&window, &view);
            });
        });

    if let Err(error) = spawn_result {
        let mut view = lock_state(&state);
        if view.refresh_token == token {
            view.loading = false;
            view.error = format!("启动规则刷新线程失败：{error}");
            if let Some(window) = fallback_weak.upgrade() {
                set_ui_model(&window, &view);
            }
        }
    }
}

fn format_error(prefix: &str, error: &ApiError) -> String {
    format!("{prefix}：{error}")
}

#[cfg(test)]
mod tests {
    use super::{clear_runtime, new_state, sort_rules};
    use crate::clash::api::RuleEntry;

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
            view.refresh_token
        };

        clear_runtime(&state);

        let view = state.lock().unwrap();
        assert!(view.rules.is_empty());
        assert!(!view.loading);
        assert!(view.error.is_empty());
        assert_ne!(view.refresh_token, previous_token);
    }
}
