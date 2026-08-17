// 代理页数据转换、状态合并和后台操作。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use slint::{ComponentHandle, Model, ModelRc, VecModel, Weak};

use crate::clash::api::{self, ApiError, ProxyEntry};
use crate::constants::{DEFAULT_TEST_URL, TEST_TIMEOUT_MS};
use crate::{MainWindow, ProxyGroup, ProxyNode};

const LATENCY_UNTESTED: i32 = 0;
const LATENCY_TESTING: i32 = 1;
const LATENCY_SUCCESS: i32 = 2;
const LATENCY_FAILED: i32 = 3;

#[derive(Clone, Debug)]
struct ProxyNodeState {
    name: String,
    type_: String,
    latency: i32,
    latency_state: i32,
    operation_token: u64,
    result_override: bool,
}

#[derive(Clone, Debug)]
struct ProxyGroupState {
    name: String,
    now: String,
    test_url: String,
    open: bool,
    testing: bool,
    test_token: u64,
    nodes: Vec<ProxyNodeState>,
}

#[derive(Debug, Default)]
pub struct ProxyViewState {
    groups: Vec<ProxyGroupState>,
    loading: bool,
    error: String,
    next_token: u64,
    refresh_token: u64,
    selection_token: u64,
}

pub type SharedProxyState = Arc<Mutex<ProxyViewState>>;

pub(crate) fn bind_callbacks(window: &MainWindow, state: SharedProxyState) {
    window
        .global::<crate::ProxyModel>()
        .set_groups(ModelRc::new(VecModel::default()));
    let weak = window.as_weak();
    window.global::<crate::ProxyModel>().on_toggle_group({
        let weak = weak.clone();
        let state = state.clone();
        move |index| toggle_group(weak.clone(), state.clone(), index)
    });
    window.global::<crate::ProxyModel>().on_select_node({
        let weak = weak.clone();
        let state = state.clone();
        move |group_index, node_index| {
            select_node_async(weak.clone(), state.clone(), group_index, node_index)
        }
    });
    window.global::<crate::ProxyModel>().on_test_group({
        let weak = weak.clone();
        let state = state.clone();
        move |index| test_group_async(weak.clone(), state.clone(), index)
    });
    window.global::<crate::ProxyModel>().on_test_node({
        let weak = weak.clone();
        move |group_index, node_index| {
            test_node_async(weak.clone(), state.clone(), group_index, node_index)
        }
    });
}

fn lock_state(state: &SharedProxyState) -> MutexGuard<'_, ProxyViewState> {
    state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub fn new_state() -> SharedProxyState {
    Arc::new(Mutex::new(ProxyViewState::default()))
}

/// 清空核心运行期间的代理数据，并使未完成请求失效。
pub fn clear_runtime(state: &SharedProxyState) {
    let mut view = lock_state(state);
    view.groups.clear();
    view.loading = false;
    view.error.clear();
    let token = next_token(&mut view);
    view.refresh_token = token;
    view.selection_token = token;
}

fn next_token(state: &mut ProxyViewState) -> u64 {
    state.next_token = state.next_token.wrapping_add(1).max(1);
    state.next_token
}

fn initial_latency(entry: &ProxyEntry) -> (i32, i32) {
    entry
        .history
        .iter()
        .rev()
        .find(|history| history.delay > 0)
        .map(|history| (history.delay as i32, LATENCY_SUCCESS))
        .unwrap_or((LATENCY_UNTESTED, LATENCY_UNTESTED))
}

fn build_group_states(proxies: &HashMap<String, ProxyEntry>) -> Vec<ProxyGroupState> {
    let mut groups: Vec<_> = proxies
        .values()
        .filter(|entry| !entry.all.is_empty())
        .map(|group| {
            let nodes = group
                .all
                .iter()
                .map(|name| {
                    let (latency, latency_state) = proxies
                        .get(name)
                        .map(initial_latency)
                        .unwrap_or((LATENCY_UNTESTED, LATENCY_UNTESTED));
                    ProxyNodeState {
                        name: name.clone(),
                        type_: proxies
                            .get(name)
                            .map(|entry| entry.type_.clone())
                            .unwrap_or_else(|| "未知".to_string()),
                        latency,
                        latency_state,
                        operation_token: 0,
                        result_override: false,
                    }
                })
                .collect();

            ProxyGroupState {
                name: group.name.clone(),
                now: group.now.clone().unwrap_or_default(),
                test_url: group
                    .test_url
                    .clone()
                    .filter(|url| !url.is_empty())
                    .unwrap_or_else(|| DEFAULT_TEST_URL.to_string()),
                open: false,
                testing: false,
                test_token: 0,
                nodes,
            }
        })
        .collect();
    groups.sort_by(
        |left, right| match (left.name.as_str(), right.name.as_str()) {
            ("GLOBAL", "GLOBAL") => std::cmp::Ordering::Equal,
            ("GLOBAL", _) => std::cmp::Ordering::Greater,
            (_, "GLOBAL") => std::cmp::Ordering::Less,
            _ => left.name.cmp(&right.name),
        },
    );
    groups
}

impl ProxyViewState {
    fn merge_proxies(&mut self, proxies: &HashMap<String, ProxyEntry>) {
        let old_groups: HashMap<_, _> = self
            .groups
            .drain(..)
            .map(|group| (group.name.clone(), group))
            .collect();

        self.groups = build_group_states(proxies)
            .into_iter()
            .map(|mut group| {
                if let Some(old_group) = old_groups.get(&group.name) {
                    group.open = old_group.open;
                    group.testing = old_group.testing && !old_group.nodes.is_empty();
                    group.test_token = old_group.test_token;

                    for node in &mut group.nodes {
                        if let Some(old_node) = old_group.nodes.iter().find(|n| n.name == node.name)
                        {
                            if old_node.result_override || old_node.latency_state == LATENCY_TESTING
                            {
                                node.latency = old_node.latency;
                                node.latency_state = old_node.latency_state;
                                node.operation_token = old_node.operation_token;
                                node.result_override = old_node.result_override;
                            }
                        }
                    }

                    if group.testing {
                        group.testing = group
                            .nodes
                            .iter()
                            .any(|node| node.latency_state == LATENCY_TESTING);
                    }
                }
                group
            })
            .collect();
    }

    fn set_loading(&mut self, loading: bool) {
        self.loading = loading;
    }

    fn to_slint_groups(&self) -> Vec<ProxyGroup> {
        self.groups
            .iter()
            .map(|group| {
                let nodes = group
                    .nodes
                    .iter()
                    .map(|node| ProxyNode {
                        name: node.name.clone().into(),
                        r#type: node.type_.clone().into(),
                        latency: node.latency,
                        latency_state: node.latency_state,
                    })
                    .collect::<Vec<_>>();
                ProxyGroup {
                    name: group.name.clone().into(),
                    now: group.now.clone().into(),
                    open: group.open,
                    testing: group.testing,
                    nodes: ModelRc::new(VecModel::from(nodes)),
                }
            })
            .collect()
    }
}

fn set_ui_model(window: &MainWindow, state: &ProxyViewState) {
    let model = window.global::<crate::ProxyModel>();
    let groups = state.to_slint_groups();
    let current_groups = model.get_groups();
    if let Some(groups_model) = current_groups
        .as_any()
        .downcast_ref::<VecModel<ProxyGroup>>()
    {
        sync_groups_model(groups_model, groups);
    } else {
        model.set_groups(ModelRc::new(VecModel::from(groups)));
    }
    model.set_loading(state.loading);
    model.set_error(state.error.clone().into());
}

pub fn sync_ui(window: &MainWindow, state: &SharedProxyState) {
    let state = lock_state(state);
    set_ui_model(window, &state);
}

fn sync_vec_model<T: Clone + 'static>(model: &VecModel<T>, values: Vec<T>) {
    let common_count = model.row_count().min(values.len());
    for (index, value) in values.iter().take(common_count).cloned().enumerate() {
        model.set_row_data(index, value);
    }
    while model.row_count() > values.len() {
        model.remove(model.row_count() - 1);
    }
    for value in values.into_iter().skip(common_count) {
        model.push(value);
    }
}

fn reuse_node_model(current: &ProxyGroup, next: &mut ProxyGroup) {
    let Some(current_nodes) = current.nodes.as_any().downcast_ref::<VecModel<ProxyNode>>() else {
        return;
    };
    let Some(next_nodes) = next.nodes.as_any().downcast_ref::<VecModel<ProxyNode>>() else {
        return;
    };
    let values = (0..next_nodes.row_count())
        .filter_map(|index| next_nodes.row_data(index))
        .collect();
    sync_vec_model(current_nodes, values);
    next.nodes = current.nodes.clone();
}

fn sync_groups_model(model: &VecModel<ProxyGroup>, groups: Vec<ProxyGroup>) {
    let common_count = model.row_count().min(groups.len());
    for index in 0..common_count {
        let Some(mut group) = groups.get(index).cloned() else {
            continue;
        };
        if let Some(current) = model.row_data(index) {
            reuse_node_model(&current, &mut group);
        }
        model.set_row_data(index, group);
    }
    while model.row_count() > groups.len() {
        model.remove(model.row_count() - 1);
    }
    for group in groups.into_iter().skip(common_count) {
        model.push(group);
    }
}

fn invoke_ui<F>(callback: F)
where
    F: FnOnce() + Send + 'static,
{
    if let Err(error) = slint::invoke_from_event_loop(callback) {
        crate::log::error(format_args!("代理页 UI 回调失败: {error}"));
    }
}

pub fn refresh_async(weak: Weak<MainWindow>, state: SharedProxyState) {
    let token = {
        let mut view = lock_state(&state);
        let token = next_token(&mut view);
        view.refresh_token = token;
        view.selection_token = token;
        view.error.clear();
        view.set_loading(true);
        if let Some(window) = weak.upgrade() {
            set_ui_model(&window, &view);
        }
        token
    };

    std::thread::spawn(move || {
        let result = api::get_proxies();
        invoke_ui(move || {
            let Some(window) = weak.upgrade() else { return };
            let mut view = lock_state(&state);
            if view.refresh_token != token {
                return;
            }
            match result {
                Ok(proxies) => {
                    view.merge_proxies(&proxies);
                    view.error.clear();
                }
                Err(error) => {
                    view.error = format_error("加载代理数据失败", &error);
                }
            }
            view.set_loading(false);
            set_ui_model(&window, &view);
        });
    });
}

pub fn toggle_group(weak: Weak<MainWindow>, state: SharedProxyState, index: i32) {
    let Some(window) = weak.upgrade() else { return };
    let mut view = lock_state(&state);
    if let Some(group) = view.groups.get_mut(index.max(0) as usize) {
        group.open = !group.open;
    }
    set_ui_model(&window, &view);
}

pub fn select_node_async(
    weak: Weak<MainWindow>,
    state: SharedProxyState,
    group_index: i32,
    node_index: i32,
) {
    let (group_name, node_name, token) = {
        let mut view = lock_state(&state);
        let Some(group) = view.groups.get(group_index.max(0) as usize) else {
            return;
        };
        let Some(node) = group.nodes.get(node_index.max(0) as usize) else {
            return;
        };
        let group_name = group.name.clone();
        let node_name = node.name.clone();
        let token = next_token(&mut view);
        view.selection_token = token;
        view.refresh_token = token;
        view.error.clear();
        view.set_loading(true);
        if let Some(window) = weak.upgrade() {
            set_ui_model(&window, &view);
        }
        (group_name, node_name, token)
    };

    std::thread::spawn(move || {
        let result = api::select_proxy(&group_name, &node_name).and_then(|_| api::get_proxies());
        invoke_ui(move || {
            let Some(window) = weak.upgrade() else { return };
            let mut view = lock_state(&state);
            if view.selection_token != token {
                return;
            }
            match result {
                Ok(proxies) => {
                    view.merge_proxies(&proxies);
                    view.error.clear();
                }
                Err(error) => {
                    view.error = format_error("选择代理节点失败", &error);
                }
            }
            view.set_loading(false);
            set_ui_model(&window, &view);
        });
    });
}

pub fn test_group_async(weak: Weak<MainWindow>, state: SharedProxyState, group_index: i32) {
    let (group_name, test_url, token) = {
        let mut view = lock_state(&state);
        let index = group_index.max(0) as usize;
        let Some(group) = view.groups.get_mut(index) else {
            return;
        };
        if group.testing {
            return;
        }
        let token = next_token(&mut view);
        let Some(group) = view.groups.get_mut(index) else {
            return;
        };
        group.testing = true;
        group.test_token = token;
        for node in &mut group.nodes {
            node.latency = 0;
            node.latency_state = LATENCY_TESTING;
            node.operation_token = token;
            node.result_override = true;
        }
        let group_name = group.name.clone();
        let test_url = group.test_url.clone();
        if let Some(window) = weak.upgrade() {
            set_ui_model(&window, &view);
        }
        (group_name, test_url, token)
    };

    std::thread::spawn(move || {
        let result = api::get_group_delay(&group_name, &test_url, TEST_TIMEOUT_MS);
        invoke_ui(move || {
            let Some(window) = weak.upgrade() else { return };
            let mut view = lock_state(&state);
            let Some(group) = view
                .groups
                .iter_mut()
                .find(|group| group.name == group_name)
            else {
                return;
            };
            if group.test_token != token {
                return;
            }
            match result {
                Ok(delays) => {
                    for node in &mut group.nodes {
                        if node.operation_token != token {
                            continue;
                        }
                        match delays.get(&node.name).copied() {
                            Some(delay) if delay > 0 => {
                                node.latency = delay as i32;
                                node.latency_state = LATENCY_SUCCESS;
                            }
                            _ => {
                                node.latency = 0;
                                node.latency_state = LATENCY_FAILED;
                            }
                        }
                    }
                }
                Err(_) => {
                    for node in &mut group.nodes {
                        if node.operation_token == token {
                            node.latency = 0;
                            node.latency_state = LATENCY_FAILED;
                        }
                    }
                }
            }
            group.testing = false;
            set_ui_model(&window, &view);
        });
    });
}

pub fn test_node_async(
    weak: Weak<MainWindow>,
    state: SharedProxyState,
    group_index: i32,
    node_index: i32,
) {
    let (group_name, node_name, test_url, token) = {
        let mut view = lock_state(&state);
        let group_position = group_index.max(0) as usize;
        let node_position = node_index.max(0) as usize;
        let Some(group) = view.groups.get(group_position) else {
            return;
        };
        let Some(node) = group.nodes.get(node_position) else {
            return;
        };
        if node.latency_state == LATENCY_TESTING {
            return;
        }
        let group_name = group.name.clone();
        let node_name = node.name.clone();
        let test_url = group.test_url.clone();
        let token = next_token(&mut view);
        let Some(group) = view.groups.get_mut(group_position) else {
            return;
        };
        let Some(node) = group.nodes.get_mut(node_position) else {
            return;
        };
        node.latency = 0;
        node.latency_state = LATENCY_TESTING;
        node.operation_token = token;
        node.result_override = true;
        if let Some(window) = weak.upgrade() {
            set_ui_model(&window, &view);
        }
        (group_name, node_name, test_url, token)
    };

    std::thread::spawn(move || {
        let result = api::get_proxy_delay(&node_name, &test_url, TEST_TIMEOUT_MS);
        invoke_ui(move || {
            let Some(window) = weak.upgrade() else { return };
            let mut view = lock_state(&state);
            let Some(group) = view
                .groups
                .iter_mut()
                .find(|group| group.name == group_name)
            else {
                return;
            };
            let Some(node) = group.nodes.iter_mut().find(|node| node.name == node_name) else {
                return;
            };
            if node.operation_token != token {
                return;
            }
            match result {
                Ok(delay) if delay > 0 => {
                    node.latency = delay as i32;
                    node.latency_state = LATENCY_SUCCESS;
                }
                Ok(_) | Err(_) => {
                    node.latency = 0;
                    node.latency_state = LATENCY_FAILED;
                }
            }
            set_ui_model(&window, &view);
        });
    });
}

fn format_error(prefix: &str, error: &ApiError) -> String {
    format!("{prefix}：{error}")
}

#[cfg(test)]
mod tests {
    use super::{build_group_states, clear_runtime, sync_groups_model, sync_vec_model};
    use crate::clash::api::ProxyEntry;
    use crate::{ProxyGroup, ProxyNode};
    use slint::{Model, ModelRc, VecModel};
    use std::collections::HashMap;
    use std::rc::Rc;

    fn proxy_node(name: &str) -> ProxyNode {
        ProxyNode {
            name: name.into(),
            r#type: "Http".into(),
            latency: 0,
            latency_state: 0,
        }
    }

    fn proxy_group(name: &str, nodes: Rc<VecModel<ProxyNode>>) -> ProxyGroup {
        ProxyGroup {
            name: name.into(),
            now: "".into(),
            open: false,
            testing: false,
            nodes: ModelRc::from(nodes),
        }
    }

    #[test]
    fn sync_vec_model_updates_rows_without_replacing_model() {
        let model = Rc::new(VecModel::from(vec![1, 2]));
        let model_rc = ModelRc::from(model.clone());

        sync_vec_model(&model, vec![3, 4, 5]);
        assert_eq!(model_rc, ModelRc::from(model.clone()));
        assert_eq!(model.row_count(), 3);
        assert_eq!(model.row_data(0), Some(3));
        assert_eq!(model.row_data(2), Some(5));

        sync_vec_model(&model, vec![6]);
        assert_eq!(model_rc, ModelRc::from(model.clone()));
        assert_eq!(model.row_count(), 1);
        assert_eq!(model.row_data(0), Some(6));
    }

    #[test]
    fn sync_groups_model_reuses_nested_node_model() {
        let old_nodes = Rc::new(VecModel::from(vec![proxy_node("旧节点")]));
        let groups = Rc::new(VecModel::from(vec![proxy_group("组 A", old_nodes.clone())]));
        let new_nodes = Rc::new(VecModel::from(vec![
            proxy_node("新节点 1"),
            proxy_node("新节点 2"),
        ]));

        sync_groups_model(&groups, vec![proxy_group("组 A", new_nodes)]);

        let group = groups.row_data(0).unwrap();
        assert_eq!(group.nodes, ModelRc::from(old_nodes.clone()));
        let nodes = group
            .nodes
            .as_any()
            .downcast_ref::<VecModel<ProxyNode>>()
            .unwrap();
        assert_eq!(nodes.row_count(), 2);
        assert_eq!(
            nodes.row_data(0).unwrap().name,
            slint::SharedString::from("新节点 1")
        );
        assert_eq!(
            nodes.row_data(1).unwrap().name,
            slint::SharedString::from("新节点 2")
        );
    }

    #[test]
    fn builds_stable_groups_and_uses_latest_valid_history() {
        let raw = serde_json::json!({
            "name": "组 A",
            "type": "Selector",
            "all": ["节点 2", "节点 1"],
            "now": "节点 1",
            "history": [],
            "test-url": ""
        });
        let group: ProxyEntry = serde_json::from_value(raw).unwrap();
        let node_1: ProxyEntry = serde_json::from_value(serde_json::json!({
            "name": "节点 1", "type": "Shadowsocks", "history": [
                { "time": "2026-08-04T00:00:00Z", "delay": 0 },
                { "time": "2026-08-04T00:01:00Z", "delay": 180 }
            ]
        }))
        .unwrap();
        let node_2: ProxyEntry = serde_json::from_value(serde_json::json!({
            "name": "节点 2", "type": "Http"
        }))
        .unwrap();
        let mut proxies = HashMap::new();
        proxies.insert(group.name.clone(), group);
        proxies.insert(node_1.name.clone(), node_1);
        proxies.insert(node_2.name.clone(), node_2);

        let groups = build_group_states(&proxies);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].nodes[0].name, "节点 2");
        assert_eq!(groups[0].nodes[1].latency, 180);
        assert_eq!(groups[0].nodes[1].type_, "Shadowsocks");
        assert_eq!(groups[0].test_url, super::DEFAULT_TEST_URL);
    }

    #[test]
    fn keeps_global_group_last() {
        let mut proxies = HashMap::new();
        for name in ["GLOBAL", "自动选择", "直连"] {
            proxies.insert(
                name.to_string(),
                serde_json::from_value(serde_json::json!({
                    "name": name,
                    "type": "Selector",
                    "all": ["节点"],
                    "now": "节点"
                }))
                .unwrap(),
            );
        }

        let groups = build_group_states(&proxies);
        assert_eq!(
            groups
                .iter()
                .map(|group| group.name.as_str())
                .collect::<Vec<_>>(),
            vec!["直连", "自动选择", "GLOBAL"]
        );
    }

    #[test]
    fn keeps_other_groups_sorted_without_global() {
        let mut proxies = HashMap::new();
        for name in ["乙", "甲", "丙"] {
            proxies.insert(
                name.to_string(),
                serde_json::from_value(serde_json::json!({
                    "name": name,
                    "type": "Selector",
                    "all": ["节点"],
                    "now": "节点"
                }))
                .unwrap(),
            );
        }
        let groups = build_group_states(&proxies);
        assert_eq!(
            groups
                .iter()
                .map(|group| group.name.as_str())
                .collect::<Vec<_>>(),
            vec!["丙", "乙", "甲"]
        );
    }

    #[test]
    fn clear_runtime_removes_groups_and_invalidates_requests() {
        let state = super::new_state();
        let mut proxies = HashMap::new();
        proxies.insert(
            "组".to_string(),
            serde_json::from_value(serde_json::json!({
                "name": "组",
                "type": "Selector",
                "all": ["节点"]
            }))
            .unwrap(),
        );

        let (refresh_token, selection_token) = {
            let mut view = state.lock().unwrap();
            view.merge_proxies(&proxies);
            view.loading = true;
            view.error = "旧错误".to_string();
            (view.refresh_token, view.selection_token)
        };

        clear_runtime(&state);

        let view = state.lock().unwrap();
        assert!(view.groups.is_empty());
        assert!(!view.loading);
        assert!(view.error.is_empty());
        assert_ne!(view.refresh_token, refresh_token);
        assert_ne!(view.selection_token, selection_token);
        assert_eq!(view.refresh_token, view.selection_token);
    }
}
