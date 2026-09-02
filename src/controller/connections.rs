use std::cmp::Ordering;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use slint::{ComponentHandle, Model, ModelRc, VecModel, Weak};
use tokio::sync::{mpsc, watch};

use crate::clash::api::{self, ConnEntry, ConnectionSnapshot};
use crate::clash::stream::ConnectionUpdate;
use crate::consts::MAX_CONNECTION_HISTORY;
use crate::event::CoreState;
use crate::{ConnectionDetailRow, ConnectionRow, ConnectionsModel, MainWindow};

#[derive(Debug, Clone)]
pub struct ConnectionRecord {
    pub entry: ConnEntry,
    pub upload_rate: f64,
    pub download_rate: f64,
}

#[derive(Debug, Clone)]
pub struct ClosedConnection {
    pub history_id: u64,
    pub record: Arc<ConnectionRecord>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionTab {
    Active,
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortColumn {
    Host,
    DownloadRate,
    UploadRate,
    Download,
    Upload,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDirection {
    Desc,
    Asc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SortState {
    pub column: Option<SortColumn>,
    pub direction: SortDirection,
}

impl Default for SortState {
    fn default() -> Self {
        Self {
            column: None,
            direction: SortDirection::Desc,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ConnectionsViewState {
    pub active_by_id: HashMap<String, Arc<ConnectionRecord>>,
    pub closed: VecDeque<ClosedConnection>,
    pub initialized: bool,
    pub previous_snapshot_at: Option<Instant>,
    pub selected_tab: ConnectionTab,
    pub query: String,
    pub sort: SortState,
    pub next_history_id: u64,
    pub busy: bool,
    pub error: String,
    pub toast: String,
    pub detail_identity: Option<String>,
    pub operation_token: u64,
}

impl Default for ConnectionsViewState {
    fn default() -> Self {
        Self {
            active_by_id: HashMap::new(),
            closed: VecDeque::new(),
            initialized: false,
            previous_snapshot_at: None,
            selected_tab: ConnectionTab::Active,
            query: String::new(),
            sort: SortState::default(),
            next_history_id: 1,
            busy: false,
            error: String::new(),
            toast: String::new(),
            detail_identity: None,
            operation_token: 0,
        }
    }
}

pub type SharedConnectionsState = Arc<Mutex<ConnectionsViewState>>;

type UiNotifier = Arc<dyn Fn() + Send + Sync + 'static>;

pub(crate) fn bind_callbacks(window: &MainWindow, state: SharedConnectionsState) {
    bind_view_callbacks(window, state.clone());
    bind_action_callbacks(window, state);
}

fn bind_view_callbacks(window: &MainWindow, state: SharedConnectionsState) {
    let weak = window.as_weak();
    window.global::<ConnectionsModel>().on_select_tab({
        let weak = weak.clone();
        let state = state.clone();
        move |tab| {
            select_tab(&state, tab);
            if let Some(window) = weak.upgrade() {
                sync_ui(&window, &state);
            }
        }
    });
    window.global::<ConnectionsModel>().on_search_changed({
        let weak = weak.clone();
        let state = state.clone();
        move |query| {
            set_query(&state, query.into());
            if let Some(window) = weak.upgrade() {
                sync_ui(&window, &state);
            }
        }
    });
    window.global::<ConnectionsModel>().on_header_clicked({
        let weak = weak.clone();
        let state = state.clone();
        move |column| {
            sort_by_index(&state, column);
            if let Some(window) = weak.upgrade() {
                sync_ui(&window, &state);
            }
        }
    });
    window.global::<ConnectionsModel>().on_row_clicked({
        let weak = weak.clone();
        move |identity| {
            open_detail(&state, identity.into());
            if let Some(window) = weak.upgrade() {
                sync_ui(&window, &state);
            }
        }
    });
}

fn bind_action_callbacks(window: &MainWindow, state: SharedConnectionsState) {
    let weak = window.as_weak();
    window.global::<ConnectionsModel>().on_row_action({
        let weak = weak.clone();
        let state = state.clone();
        move |identity| {
            let identity: String = identity.into();
            if identity.starts_with("history-") {
                remove_history(weak.clone(), state.clone(), identity);
            } else {
                close_connection_async(weak.clone(), state.clone(), identity);
            }
        }
    });
    window.global::<ConnectionsModel>().on_close_all({
        let weak = weak.clone();
        let state = state.clone();
        move || close_all_async(weak.clone(), state.clone())
    });
    window.global::<ConnectionsModel>().on_clear_history({
        let weak = weak.clone();
        let state = state.clone();
        move || clear_history(weak.clone(), state.clone())
    });
    window.global::<ConnectionsModel>().on_close_detail({
        let weak = weak.clone();
        move || {
            close_detail(&state);
            if let Some(window) = weak.upgrade() {
                sync_ui(&window, &state);
            }
        }
    });
}

#[derive(Clone)]
pub struct ConnectionsRecorder {
    state: SharedConnectionsState,
    notifier: Arc<Mutex<Option<UiNotifier>>>,
}

fn lock_state(state: &SharedConnectionsState) -> MutexGuard<'_, ConnectionsViewState> {
    state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn lock_notifier(notifier: &Arc<Mutex<Option<UiNotifier>>>) -> MutexGuard<'_, Option<UiNotifier>> {
    notifier
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub fn new_state() -> SharedConnectionsState {
    Arc::new(Mutex::new(ConnectionsViewState::default()))
}

/// 清空核心运行期间的连接数据，并使未完成操作失效。
pub fn clear_runtime(state: &SharedConnectionsState) {
    let mut state = lock_state(state);
    state.active_by_id.clear();
    state.closed.clear();
    state.initialized = false;
    state.previous_snapshot_at = None;
    state.busy = false;
    state.error.clear();
    state.toast.clear();
    state.detail_identity = None;
    next_operation_token(&mut state);
}

fn sync_core_state(
    current: &mut CoreState,
    receiver: &mut watch::Receiver<CoreState>,
    state: &SharedConnectionsState,
) -> bool {
    let next = *receiver.borrow_and_update();
    if *current == next {
        return false;
    }
    *current = next;
    clear_runtime(state);
    true
}

fn accepts_update(state: CoreState, update: &ConnectionUpdate) -> bool {
    state.running && state.generation == update.core_generation
}

pub fn start_recorder(
    mut receiver: mpsc::Receiver<ConnectionUpdate>,
    mut core_state: watch::Receiver<CoreState>,
) -> ConnectionsRecorder {
    let recorder = ConnectionsRecorder {
        state: new_state(),
        notifier: Arc::new(Mutex::new(None)),
    };
    let worker = recorder.clone();
    crate::runtime::spawn_task(async move {
        let mut current_core_state = *core_state.borrow_and_update();
        loop {
            tokio::select! {
                biased;
                result = core_state.changed() => {
                    if result.is_err() {
                        return;
                    }
                    if sync_core_state(&mut current_core_state, &mut core_state, &worker.state) {
                        worker.notify();
                    }
                }
                update = receiver.recv() => {
                    let Some(update) = update else {
                        return;
                    };
                    let state_changed = sync_core_state(
                        &mut current_core_state,
                        &mut core_state,
                        &worker.state,
                    );
                    if state_changed {
                        worker.notify();
                    }
                    if !accepts_update(current_core_state, &update) {
                        continue;
                    }
                    apply_snapshot(&worker.state, update.snapshot, Instant::now());
                    worker.notify();
                }
            }
        }
    });
    recorder
}

impl ConnectionsRecorder {
    pub fn state(&self) -> SharedConnectionsState {
        self.state.clone()
    }

    pub fn set_notifier(&self, notifier: UiNotifier) {
        *lock_notifier(&self.notifier) = Some(notifier);
    }

    pub fn notify(&self) {
        let notifier = lock_notifier(&self.notifier).clone();
        if let Some(notifier) = notifier {
            notifier();
        }
    }
}

#[derive(Clone)]
struct VisibleConnection {
    identity: String,
    record: Arc<ConnectionRecord>,
}

fn visible_connections(state: &ConnectionsViewState) -> Vec<VisibleConnection> {
    let mut visible = match state.selected_tab {
        ConnectionTab::Active => state
            .active_by_id
            .values()
            .cloned()
            .map(|record| VisibleConnection {
                identity: record.entry.id.clone(),
                record,
            })
            .collect::<Vec<_>>(),
        ConnectionTab::Closed => state
            .closed
            .iter()
            .map(|closed| VisibleConnection {
                identity: format!("history-{}", closed.history_id),
                record: closed.record.clone(),
            })
            .collect::<Vec<_>>(),
    };

    let query = state.query.to_ascii_lowercase();
    visible.retain(|item| matches_query(&item.record, &query));
    visible.sort_by(|left, right| compare_visible(left, right, state.sort));
    visible
}

fn compare_visible(
    left: &VisibleConnection,
    right: &VisibleConnection,
    sort: SortState,
) -> Ordering {
    let value_order = match sort.column {
        None => right.record.entry.start.cmp(&left.record.entry.start),
        Some(SortColumn::Host) => connection_host(&left.record).cmp(connection_host(&right.record)),
        Some(SortColumn::DownloadRate) => left
            .record
            .download_rate
            .partial_cmp(&right.record.download_rate)
            .unwrap_or(Ordering::Equal),
        Some(SortColumn::UploadRate) => left
            .record
            .upload_rate
            .partial_cmp(&right.record.upload_rate)
            .unwrap_or(Ordering::Equal),
        Some(SortColumn::Download) => left.record.entry.download.cmp(&right.record.entry.download),
        Some(SortColumn::Upload) => left.record.entry.upload.cmp(&right.record.entry.upload),
    };
    let directed = if sort.column.is_some() && sort.direction == SortDirection::Asc {
        value_order
    } else if sort.column.is_some() {
        value_order.reverse()
    } else {
        value_order
    };
    if directed == Ordering::Equal {
        left.identity.cmp(&right.identity)
    } else {
        directed
    }
}

fn secondary_text(record: &ConnectionRecord) -> String {
    let mut values = Vec::new();
    if !record.entry.metadata.process.is_empty() {
        values.push(record.entry.metadata.process.clone());
    }
    if !record.entry.metadata.type_.is_empty() {
        values.push(record.entry.metadata.type_.clone());
    }
    if !record.entry.metadata.network.is_empty() {
        values.push(record.entry.metadata.network.clone());
    }
    if !record.entry.chains.is_empty() {
        values.push(record.entry.chains[0].clone());
    }
    if !record.entry.rule.is_empty() {
        values.push(format!(
            "{}: {}",
            record.entry.rule,
            if record.entry.rule_payload.is_empty() {
                "-"
            } else {
                &record.entry.rule_payload
            }
        ));
    }
    values.join(" · ")
}

pub fn project_rows(state: &ConnectionsViewState) -> Vec<ConnectionRow> {
    visible_connections(state)
        .into_iter()
        .map(|item| ConnectionRow {
            id: item.identity.into(),
            cells: ModelRc::new(VecModel::from(vec![
                connection_host(&item.record).into(),
                format_rate(item.record.download_rate).into(),
                format_rate(item.record.upload_rate).into(),
                format_bytes(item.record.entry.download).into(),
                format_bytes(item.record.entry.upload).into(),
            ])),
            secondary_content: secondary_text(&item.record).into(),
        })
        .collect()
}

fn sync_vec_model<T: Clone + PartialEq + 'static>(model: &VecModel<T>, values: Vec<T>) {
    let common_count = model.row_count().min(values.len());
    for (index, value) in values.iter().take(common_count).cloned().enumerate() {
        if model.row_data(index).as_ref() != Some(&value) {
            model.set_row_data(index, value);
        }
    }
    while model.row_count() > values.len() {
        model.remove(model.row_count() - 1);
    }
    for value in values.into_iter().skip(common_count) {
        model.push(value);
    }
}

fn reuse_cells_model(current: &ConnectionRow, next: &mut ConnectionRow) {
    let Some(current_cells) = current
        .cells
        .as_any()
        .downcast_ref::<VecModel<slint::SharedString>>()
    else {
        return;
    };
    let Some(next_cells) = next
        .cells
        .as_any()
        .downcast_ref::<VecModel<slint::SharedString>>()
    else {
        return;
    };
    let values = (0..next_cells.row_count())
        .filter_map(|index| next_cells.row_data(index))
        .collect();
    sync_vec_model(current_cells, values);
    next.cells = current.cells.clone();
}

fn sync_rows_model(model: &VecModel<ConnectionRow>, rows: Vec<ConnectionRow>) {
    let common_count = model.row_count().min(rows.len());
    for index in 0..common_count {
        let Some(mut row) = rows.get(index).cloned() else {
            continue;
        };
        if let Some(current) = model.row_data(index) {
            reuse_cells_model(&current, &mut row);
            if current.id == row.id && current.secondary_content == row.secondary_content {
                continue;
            }
        }
        model.set_row_data(index, row);
    }
    while model.row_count() > rows.len() {
        model.remove(model.row_count() - 1);
    }
    for row in rows.into_iter().skip(common_count) {
        model.push(row);
    }
}

fn sort_column_index(column: Option<SortColumn>) -> i32 {
    match column {
        None => -1,
        Some(SortColumn::Host) => 0,
        Some(SortColumn::DownloadRate) => 1,
        Some(SortColumn::UploadRate) => 2,
        Some(SortColumn::Download) => 3,
        Some(SortColumn::Upload) => 4,
    }
}

fn sort_direction_value(sort: SortState) -> i32 {
    if sort.column.is_none() {
        0
    } else if sort.direction == SortDirection::Desc {
        1
    } else {
        -1
    }
}

fn record_for_identity<'a>(
    state: &'a ConnectionsViewState,
    identity: &str,
) -> Option<&'a ConnectionRecord> {
    if let Some(history_id) = identity.strip_prefix("history-") {
        let history_id = history_id.parse::<u64>().ok()?;
        state
            .closed
            .iter()
            .find(|closed| closed.history_id == history_id)
            .map(|closed| closed.record.as_ref())
    } else {
        state.active_by_id.get(identity).map(Arc::as_ref)
    }
}

fn push_field(fields: &mut Vec<(String, String)>, label: &str, value: String) {
    fields.push((label.to_string(), value));
}

pub fn detail_fields(record: &ConnectionRecord) -> Vec<(String, String)> {
    let entry = &record.entry;
    let metadata = &entry.metadata;
    let mut fields = Vec::new();
    push_field(&mut fields, "连接 ID", entry.id.clone());
    push_field(&mut fields, "已上传", entry.upload.to_string());
    push_field(&mut fields, "已下载", entry.download.to_string());
    push_field(&mut fields, "建立时间", entry.start.clone());
    push_field(&mut fields, "代理链", entry.chains.join(" → "));
    push_field(
        &mut fields,
        "代理提供者链",
        entry.provider_chains.join(" → "),
    );
    push_field(&mut fields, "匹配规则", entry.rule.clone());
    push_field(&mut fields, "规则内容", entry.rule_payload.clone());

    push_field(&mut fields, "网络协议", metadata.network.clone());
    push_field(&mut fields, "连接类型", metadata.type_.clone());
    push_field(&mut fields, "源 IP", metadata.source_ip.clone());
    push_field(&mut fields, "目标 IP", metadata.destination_ip.clone());
    push_field(
        &mut fields,
        "源 IP 地理信息",
        metadata
            .source_geo_ip
            .as_ref()
            .map(|s| s.join(" → "))
            .unwrap_or_default(),
    );
    push_field(
        &mut fields,
        "目标 IP 地理信息",
        metadata
            .destination_geo_ip
            .as_ref()
            .map(|s| s.join(" → "))
            .unwrap_or_default(),
    );
    push_field(&mut fields, "源 IP ASN", metadata.source_ip_asn.clone());
    push_field(
        &mut fields,
        "目标 IP ASN",
        metadata.destination_ip_asn.clone(),
    );
    push_field(&mut fields, "源端口", metadata.source_port.clone());
    push_field(&mut fields, "目标端口", metadata.destination_port.clone());
    push_field(&mut fields, "入站 IP", metadata.inbound_ip.clone());
    push_field(&mut fields, "入站端口", metadata.inbound_port.clone());
    push_field(&mut fields, "入站名称", metadata.inbound_name.clone());
    push_field(&mut fields, "入站用户", metadata.inbound_user.clone());
    push_field(&mut fields, "重匹配名称", metadata.rematch_name.clone());
    push_field(&mut fields, "目标主机", metadata.host.clone());
    push_field(&mut fields, "DNS 模式", metadata.dns_mode.clone());
    push_field(&mut fields, "用户 ID", metadata.uid.to_string());
    push_field(&mut fields, "进程名称", metadata.process.clone());
    push_field(&mut fields, "进程路径", metadata.process_path.clone());
    push_field(&mut fields, "特殊代理", metadata.special_proxy.clone());
    push_field(&mut fields, "特殊规则", metadata.special_rules.clone());
    push_field(&mut fields, "远程目标", metadata.remote_destination.clone());
    push_field(&mut fields, "DSCP", metadata.dscp.to_string());
    push_field(&mut fields, "嗅探主机", metadata.sniff_host.clone());

    push_field(&mut fields, "上传速度", format_rate(record.upload_rate));
    push_field(&mut fields, "下载速度", format_rate(record.download_rate));
    fields
}

pub fn select_tab(state: &SharedConnectionsState, tab: i32) {
    let mut state = lock_state(state);
    state.selected_tab = if tab == 1 {
        ConnectionTab::Closed
    } else {
        ConnectionTab::Active
    };
    state.detail_identity = None;
}

pub fn set_query(state: &SharedConnectionsState, query: String) {
    lock_state(state).query = query;
}

pub fn sort_by_index(state: &SharedConnectionsState, index: i32) {
    let Some(column) = (match index {
        0 => Some(SortColumn::Host),
        1 => Some(SortColumn::DownloadRate),
        2 => Some(SortColumn::UploadRate),
        3 => Some(SortColumn::Download),
        4 => Some(SortColumn::Upload),
        _ => None,
    }) else {
        return;
    };
    let mut state = lock_state(state);
    state.sort = cycle_sort(state.sort, column);
}

pub fn open_detail(state: &SharedConnectionsState, identity: String) {
    let mut state = lock_state(state);
    if record_for_identity(&state, &identity).is_some() {
        state.detail_identity = Some(identity);
    }
}

pub fn close_detail(state: &SharedConnectionsState) {
    lock_state(state).detail_identity = None;
}

fn next_operation_token(state: &mut ConnectionsViewState) -> u64 {
    state.operation_token = state.operation_token.wrapping_add(1).max(1);
    state.operation_token
}

fn begin_operation(state: &SharedConnectionsState) -> Option<(u64, CoreState)> {
    let core_state = *crate::event::subscribe_core_state().borrow();
    if !core_state.running {
        return None;
    }
    let mut state = lock_state(state);
    if state.busy {
        return None;
    }
    let token = next_operation_token(&mut state);
    state.busy = true;
    state.error.clear();
    Some((token, core_state))
}

fn set_toast(window: &MainWindow, message: &str, variant: i32) {
    let model = window.global::<ConnectionsModel>();
    model.set_toast_message(message.to_string().into());
    model.set_toast_variant(variant);
    model.set_toast_visible(true);
}

fn invoke_ui<F>(callback: F)
where
    F: FnOnce() + Send + 'static,
{
    if let Err(error) = slint::invoke_from_event_loop(callback) {
        crate::log::error(format_args!("连接页 UI 回调失败：{error}"));
    }
}

fn finish_operation(
    weak: Weak<MainWindow>,
    state: SharedConnectionsState,
    token: u64,
    core_state: CoreState,
    result: Result<String, String>,
) {
    if let Err(message) = &result {
        crate::log::error(format_args!("{message}"));
    }
    invoke_ui(move || {
        let (message, variant) = {
            let mut view = lock_state(&state);
            if view.operation_token != token
                || *crate::event::subscribe_core_state().borrow() != core_state
            {
                return;
            }
            view.busy = false;
            match result {
                Ok(message) => {
                    view.error.clear();
                    view.toast = message.clone();
                    (message, 1)
                }
                Err(message) => {
                    view.error = message.clone();
                    view.toast = message.clone();
                    (message, 2)
                }
            }
        };
        if let Some(window) = weak.upgrade() {
            sync_ui(&window, &state);
            set_toast(&window, &message, variant);
        }
    });
}

pub fn close_connection_async(
    weak: Weak<MainWindow>,
    state: SharedConnectionsState,
    identity: String,
) {
    if !lock_state(&state).active_by_id.contains_key(&identity) {
        return;
    }
    let Some((token, core_state)) = begin_operation(&state) else {
        return;
    };
    if let Some(window) = weak.upgrade() {
        sync_ui(&window, &state);
    }
    let worker_state = state.clone();
    crate::runtime::spawn_task(async move {
        let result = api::close_connection(&identity)
            .await
            .map(|_| "关闭连接请求已发送".to_string())
            .map_err(|error| format!("关闭连接失败：{error}"));
        finish_operation(weak, worker_state, token, core_state, result);
    });
}

pub fn close_all_async(weak: Weak<MainWindow>, state: SharedConnectionsState) {
    let Some((token, core_state)) = begin_operation(&state) else {
        return;
    };
    if let Some(window) = weak.upgrade() {
        sync_ui(&window, &state);
    }
    let worker_state = state.clone();
    crate::runtime::spawn_task(async move {
        let result = api::close_all_connections()
            .await
            .map(|_| "关闭全部连接请求已发送".to_string())
            .map_err(|error| format!("关闭全部连接失败：{error}"));
        finish_operation(weak, worker_state, token, core_state, result);
    });
}

pub fn remove_history_local(state: &SharedConnectionsState, identity: &str) -> bool {
    let Some(history_id) = identity
        .strip_prefix("history-")
        .and_then(|value| value.parse::<u64>().ok())
    else {
        return false;
    };
    let mut state = lock_state(state);
    if state.busy {
        return false;
    }
    let Some(index) = state
        .closed
        .iter()
        .position(|closed| closed.history_id == history_id)
    else {
        return false;
    };
    state.closed.remove(index);
    if state.detail_identity.as_deref() == Some(identity) {
        state.detail_identity = None;
    }
    true
}

pub fn clear_history_local(state: &SharedConnectionsState) -> bool {
    let mut state = lock_state(state);
    if state.busy || state.closed.is_empty() {
        return false;
    }
    state.closed.clear();
    if state
        .detail_identity
        .as_deref()
        .is_some_and(|identity| identity.starts_with("history-"))
    {
        state.detail_identity = None;
    }
    true
}

pub fn remove_history(weak: Weak<MainWindow>, state: SharedConnectionsState, identity: String) {
    if !remove_history_local(&state, &identity) {
        return;
    }
    if let Some(window) = weak.upgrade() {
        sync_ui(&window, &state);
        set_toast(&window, "已移除历史连接", 1);
    }
}

pub fn clear_history(weak: Weak<MainWindow>, state: SharedConnectionsState) {
    if !clear_history_local(&state) {
        return;
    }
    if let Some(window) = weak.upgrade() {
        sync_ui(&window, &state);
        set_toast(&window, "已清空历史连接", 1);
    }
}

pub fn sync_ui(window: &MainWindow, state: &SharedConnectionsState) {
    let (rows, detail_rows, selected_tab, query, sort, busy, error, detail_open) = {
        let state = lock_state(state);
        let rows = project_rows(&state);
        let detail_rows = state
            .detail_identity
            .as_deref()
            .and_then(|identity| record_for_identity(&state, identity))
            .map(detail_fields)
            .unwrap_or_default()
            .into_iter()
            .map(|(label, value)| ConnectionDetailRow {
                label: label.into(),
                value: value.into(),
            })
            .collect::<Vec<_>>();
        (
            rows,
            detail_rows,
            state.selected_tab,
            state.query.clone(),
            state.sort,
            state.busy,
            state.error.clone(),
            state.detail_identity.is_some(),
        )
    };
    let model = window.global::<ConnectionsModel>();
    let current_rows = model.get_rows();
    if let Some(rows_model) = current_rows
        .as_any()
        .downcast_ref::<VecModel<ConnectionRow>>()
    {
        sync_rows_model(rows_model, rows);
    } else {
        model.set_rows(ModelRc::new(VecModel::from(rows)));
    }
    let current_detail_rows = model.get_detail_rows();
    if let Some(detail_model) = current_detail_rows
        .as_any()
        .downcast_ref::<VecModel<ConnectionDetailRow>>()
    {
        sync_vec_model(detail_model, detail_rows);
    } else {
        model.set_detail_rows(ModelRc::new(VecModel::from(detail_rows)));
    }
    model.set_tab(match selected_tab {
        ConnectionTab::Active => 0,
        ConnectionTab::Closed => 1,
    });
    model.set_query(query.into());
    model.set_sort_column(sort_column_index(sort.column));
    model.set_sort_direction(sort_direction_value(sort));
    model.set_loading(false);
    model.set_busy(busy);
    model.set_error(error.into());
    model.set_detail_open(detail_open);
}

pub fn attach_ui(recorder: &ConnectionsRecorder, weak: Weak<MainWindow>) {
    let state = recorder.state();
    let callback_state = state.clone();
    let callback_weak = weak.clone();
    let pending = Arc::new(AtomicBool::new(false));
    recorder.set_notifier(Arc::new(move || {
        if pending.swap(true, AtomicOrdering::AcqRel) {
            return;
        }
        let state = callback_state.clone();
        let weak = callback_weak.clone();
        let event_pending = pending.clone();
        if slint::invoke_from_event_loop(move || {
            if let Some(window) = weak.upgrade() {
                if window.global::<crate::AppState>().get_current_page() == 5 {
                    sync_ui(&window, &state);
                }
            }
            event_pending.store(false, AtomicOrdering::Release);
        })
        .is_err()
        {
            pending.store(false, AtomicOrdering::Release);
        }
    }));
    if let Some(window) = weak.upgrade() {
        sync_ui(&window, &state);
    }
}

pub fn apply_snapshot(state: &SharedConnectionsState, snapshot: ConnectionSnapshot, now: Instant) {
    let mut state = lock_state(state);
    let elapsed = state
        .previous_snapshot_at
        .map(|previous| now.saturating_duration_since(previous));

    let mut previous = std::mem::take(&mut state.active_by_id);
    let mut active_by_id = HashMap::with_capacity(snapshot.connections.len());
    for entry in snapshot.connections {
        let previous_record = previous.remove(&entry.id);
        let (upload_rate, download_rate) = previous_record
            .as_ref()
            .zip(elapsed)
            .filter(|(_, elapsed)| elapsed.as_secs_f64() > 0.0)
            .map(|(old, elapsed)| {
                let seconds = elapsed.as_secs_f64();
                (
                    entry.upload.saturating_sub(old.entry.upload) as f64 / seconds,
                    entry.download.saturating_sub(old.entry.download) as f64 / seconds,
                )
            })
            .unwrap_or((0.0, 0.0));
        active_by_id.insert(
            entry.id.clone(),
            Arc::new(ConnectionRecord {
                entry,
                upload_rate,
                download_rate,
            }),
        );
    }

    if state.initialized {
        for (_, record) in previous {
            let history_id = state.next_history_id;
            state.next_history_id = state.next_history_id.wrapping_add(1).max(1);
            state
                .closed
                .push_front(ClosedConnection { history_id, record });
            while state.closed.len() > MAX_CONNECTION_HISTORY {
                state.closed.pop_back();
            }
        }
    }

    state.active_by_id = active_by_id;
    state.previous_snapshot_at = Some(now);
    state.initialized = true;
}

pub fn connection_host(record: &ConnectionRecord) -> &str {
    if !record.entry.metadata.host.is_empty() {
        &record.entry.metadata.host
    } else if !record.entry.metadata.destination_ip.is_empty() {
        &record.entry.metadata.destination_ip
    } else {
        "未知主机"
    }
}

pub fn cycle_sort(current: SortState, column: SortColumn) -> SortState {
    if current.column != Some(column) {
        return SortState {
            column: Some(column),
            direction: SortDirection::Desc,
        };
    }
    match current.direction {
        SortDirection::Desc => SortState {
            column: Some(column),
            direction: SortDirection::Asc,
        },
        SortDirection::Asc => SortState::default(),
    }
}

pub fn matches_query(record: &ConnectionRecord, query: &str) -> bool {
    query.is_empty()
        || connection_host(record)
            .to_ascii_lowercase()
            .contains(&query.to_ascii_lowercase())
        || record
            .entry
            .rule
            .to_ascii_lowercase()
            .contains(&query.to_ascii_lowercase())
}

pub fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.2} MB", bytes as f64 / 1024.0 / 1024.0)
    } else {
        format!("{:.2} GB", bytes as f64 / 1024.0 / 1024.0 / 1024.0)
    }
}

pub fn format_rate(bytes_per_second: f64) -> String {
    let bytes_per_second = bytes_per_second.max(0.0);
    if bytes_per_second < 1024.0 {
        format!("{bytes_per_second:.0} B/s")
    } else if bytes_per_second < 1024.0 * 1024.0 {
        format!("{:.1} KB/s", bytes_per_second / 1024.0)
    } else {
        format!("{:.2} MB/s", bytes_per_second / 1024.0 / 1024.0)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        apply_snapshot, clear_history_local, clear_runtime, cycle_sort, detail_fields,
        format_bytes, format_rate, matches_query, new_state, project_rows, remove_history_local,
        start_recorder, sync_rows_model, visible_connections, ConnectionRecord, ConnectionTab,
        SortColumn, SortDirection, SortState,
    };
    use crate::clash::api::{ConnEntry, ConnMeta, ConnectionSnapshot};
    use crate::clash::stream::ConnectionUpdate;
    use crate::consts::MAX_CONNECTION_HISTORY;
    use crate::event::CoreState;
    use crate::ConnectionRow;
    use serde_json::Value;
    use slint::{Model, ModelRc, SharedString, VecModel};
    use std::rc::Rc;
    use std::thread;
    use std::time::{Duration, Instant};
    use tokio::sync::{broadcast, mpsc};

    fn entry(id: &str, host: &str, upload: u64, download: u64) -> ConnEntry {
        ConnEntry {
            id: id.to_string(),
            metadata: ConnMeta {
                host: host.to_string(),
                ..ConnMeta::default()
            },
            upload,
            download,
            start: format!("2026-08-11T08:00:0{id}Z"),
            ..ConnEntry::default()
        }
    }

    fn detail_record() -> ConnectionRecord {
        let mut metadata = ConnMeta {
            network: "tcp".to_string(),
            type_: "HTTP".to_string(),
            source_geo_ip: Some(vec!["CN".to_string()]),
            destination_geo_ip: Some(Vec::new()),
            uid: 0,
            dscp: 0,
            ..ConnMeta::default()
        };
        metadata
            .extra
            .insert("flag".to_string(), Value::Bool(false));
        metadata.extra.insert("nullable".to_string(), Value::Null);
        let mut entry = entry("a", "alpha.example", 0, 0);
        entry.metadata = metadata;
        entry.extra.insert(
            "object".to_string(),
            serde_json::json!({"items": [1, null]}),
        );
        ConnectionRecord {
            entry,
            upload_rate: 0.0,
            download_rate: 1024.0,
        }
    }

    fn expected_detail_labels() -> Vec<&'static str> {
        vec![
            "连接 ID",
            "已上传",
            "已下载",
            "建立时间",
            "代理链",
            "代理提供者链",
            "匹配规则",
            "规则内容",
            "网络协议",
            "连接类型",
            "源 IP",
            "目标 IP",
            "源 IP 地理信息",
            "目标 IP 地理信息",
            "源 IP ASN",
            "目标 IP ASN",
            "源端口",
            "目标端口",
            "入站 IP",
            "入站端口",
            "入站名称",
            "入站用户",
            "重匹配名称",
            "目标主机",
            "DNS 模式",
            "用户 ID",
            "进程名称",
            "进程路径",
            "特殊代理",
            "特殊规则",
            "远程目标",
            "DSCP",
            "嗅探主机",
            "上传速度",
            "下载速度",
        ]
    }

    fn snapshot(entries: Vec<ConnEntry>) -> ConnectionSnapshot {
        ConnectionSnapshot {
            connections: entries,
            ..ConnectionSnapshot::default()
        }
    }

    fn projected_row(id: &str, cells: &[&str], secondary_content: &str) -> ConnectionRow {
        ConnectionRow {
            id: id.into(),
            cells: ModelRc::new(VecModel::from(
                cells
                    .iter()
                    .map(|cell| SharedString::from(*cell))
                    .collect::<Vec<_>>(),
            )),
            secondary_content: secondary_content.into(),
        }
    }

    #[test]
    fn sync_rows_model_reuses_nested_models_and_updates_data() {
        let old_cells = Rc::new(VecModel::from(vec![SharedString::from("旧主机")]));
        let rows = Rc::new(VecModel::from(vec![ConnectionRow {
            id: "old-id".into(),
            cells: ModelRc::from(old_cells.clone()),
            secondary_content: "旧信息".into(),
        }]));
        let rows_identity = ModelRc::from(rows.clone());

        sync_rows_model(
            &rows,
            vec![
                projected_row("new-id", &["新主机", "1.0 KB/s"], "新信息"),
                projected_row("second-id", &["第二行"], "第二行信息"),
            ],
        );

        assert_eq!(rows_identity, ModelRc::from(rows.clone()));
        assert_eq!(rows.row_count(), 2);
        let first = rows.row_data(0).unwrap();
        assert_eq!(first.id, SharedString::from("new-id"));
        assert_eq!(first.secondary_content, SharedString::from("新信息"));
        assert_eq!(first.cells, ModelRc::from(old_cells.clone()));
        assert_eq!(old_cells.row_count(), 2);
        assert_eq!(old_cells.row_data(0), Some(SharedString::from("新主机")));
        assert_eq!(old_cells.row_data(1), Some(SharedString::from("1.0 KB/s")));

        sync_rows_model(
            &rows,
            vec![projected_row("final-id", &["最终主机"], "最终信息")],
        );

        assert_eq!(rows.row_count(), 1);
        assert_eq!(old_cells.row_count(), 1);
        assert_eq!(old_cells.row_data(0), Some(SharedString::from("最终主机")));
        assert_eq!(rows.row_data(0).unwrap().id, SharedString::from("final-id"));
    }

    #[test]
    fn first_frame_only_establishes_active_baseline() {
        let state = new_state();
        apply_snapshot(
            &state,
            snapshot(vec![entry("a", "a.example", 1, 2)]),
            Instant::now(),
        );
        let state = state.lock().unwrap();
        assert!(state.closed.is_empty());
        assert_eq!(state.active_by_id.len(), 1);
    }

    #[test]
    fn clear_runtime_removes_active_history_and_detail_without_losing_preferences() {
        let state = new_state();
        let start = Instant::now();
        apply_snapshot(&state, snapshot(vec![entry("a", "a.example", 1, 2)]), start);
        apply_snapshot(&state, snapshot(Vec::new()), start + Duration::from_secs(1));
        let previous_token = {
            let mut view = state.lock().unwrap();
            view.detail_identity = Some("history-1".to_string());
            view.busy = true;
            view.error = "旧错误".to_string();
            view.toast = "旧提示".to_string();
            view.query = "保留筛选".to_string();
            view.selected_tab = ConnectionTab::Closed;
            view.operation_token
        };

        clear_runtime(&state);

        let view = state.lock().unwrap();
        assert!(view.active_by_id.is_empty());
        assert!(view.closed.is_empty());
        assert!(!view.initialized);
        assert!(view.previous_snapshot_at.is_none());
        assert!(!view.busy);
        assert!(view.error.is_empty());
        assert!(view.toast.is_empty());
        assert!(view.detail_identity.is_none());
        assert_ne!(view.operation_token, previous_token);
        assert_eq!(view.query, "保留筛选");
        assert_eq!(view.selected_tab, ConnectionTab::Closed);
    }

    #[test]
    fn archives_subsequent_difference_and_allows_reappearing_connection() {
        let state = new_state();
        let start = Instant::now();
        apply_snapshot(
            &state,
            snapshot(vec![
                entry("a", "a.example", 1, 2),
                entry("b", "b.example", 3, 4),
            ]),
            start,
        );
        apply_snapshot(
            &state,
            snapshot(vec![entry("b", "b.example", 4, 5)]),
            start + Duration::from_secs(1),
        );
        apply_snapshot(
            &state,
            snapshot(vec![entry("a", "a.example", 8, 9)]),
            start + Duration::from_secs(2),
        );
        apply_snapshot(&state, snapshot(Vec::new()), start + Duration::from_secs(3));
        let state = state.lock().unwrap();
        assert!(state.active_by_id.is_empty());
        assert_eq!(state.closed.len(), 3);
        assert_eq!(state.closed[0].record.entry.id, "a");
        assert_eq!(state.closed[1].record.entry.id, "b");
        assert_eq!(state.closed[2].record.entry.id, "a");
    }

    #[test]
    fn history_keeps_only_latest_thousand_complete_records() {
        let state = new_state();
        let start = Instant::now();
        for index in 0..=MAX_CONNECTION_HISTORY {
            let id = index.to_string();
            let now = start + Duration::from_millis((index * 2) as u64);
            apply_snapshot(
                &state,
                snapshot(vec![entry(&id, &format!("{id}.example"), index as u64, 0)]),
                now,
            );
            apply_snapshot(&state, snapshot(Vec::new()), now + Duration::from_millis(1));
        }

        let view = state.lock().unwrap();
        assert_eq!(view.closed.len(), MAX_CONNECTION_HISTORY);
        assert_eq!(view.closed.front().unwrap().record.entry.id, "1000");
        assert_eq!(view.closed.back().unwrap().record.entry.id, "1");
        assert_eq!(
            detail_fields(&view.closed.front().unwrap().record).len(),
            expected_detail_labels().len()
        );
    }

    #[test]
    fn rate_uses_real_interval_and_saturating_difference() {
        let state = new_state();
        let start = Instant::now();
        apply_snapshot(
            &state,
            snapshot(vec![entry("a", "a.example", 100, 200)]),
            start,
        );
        apply_snapshot(
            &state,
            snapshot(vec![entry("a", "a.example", 50, 600)]),
            start + Duration::from_millis(500),
        );
        let state = state.lock().unwrap();
        let record = &state.active_by_id["a"];
        assert_eq!(record.upload_rate, 0.0);
        assert_eq!(record.download_rate, 800.0);
    }

    #[test]
    fn consumes_latest_snapshot_and_archives_after_lag() {
        let (sender, mut receiver) = broadcast::channel(1);
        sender
            .send(snapshot(vec![entry("a", "a.example", 1, 1)]))
            .unwrap();
        sender
            .send(snapshot(vec![entry("a", "a.example", 2, 2)]))
            .unwrap();
        assert!(matches!(
            receiver.try_recv(),
            Err(broadcast::error::TryRecvError::Lagged(_))
        ));
        let state = new_state();
        let latest = receiver.try_recv().unwrap();
        apply_snapshot(&state, latest, Instant::now());
        apply_snapshot(&state, snapshot(Vec::new()), Instant::now());
        assert_eq!(state.lock().unwrap().closed.len(), 1);
    }

    #[test]
    fn recorder_discards_snapshot_from_old_core_generation() {
        let (sender, receiver) = mpsc::channel(1);
        let (core_sender, core_state) = tokio::sync::watch::channel(CoreState {
            running: true,
            generation: 1,
        });
        let recorder = start_recorder(receiver, core_state);
        let state = recorder.state();

        core_sender.send_replace(CoreState {
            running: false,
            generation: 2,
        });
        crate::runtime::block(sender.send(ConnectionUpdate {
            core_generation: 1,
            snapshot: snapshot(vec![entry("old", "old.example", 1, 1)]),
        }))
        .unwrap();
        thread::sleep(Duration::from_millis(50));
        assert!(state.lock().unwrap().active_by_id.is_empty());

        core_sender.send_replace(CoreState {
            running: true,
            generation: 3,
        });
        crate::runtime::block(sender.send(ConnectionUpdate {
            core_generation: 3,
            snapshot: snapshot(vec![entry("new", "new.example", 1, 1)]),
        }))
        .unwrap();

        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if state.lock().unwrap().active_by_id.contains_key("new") {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let state = state.lock().unwrap();
        assert_eq!(state.active_by_id.len(), 1);
        assert!(state.active_by_id.contains_key("new"));
    }

    #[test]
    fn search_and_format_follow_page_semantics() {
        let record = ConnectionRecord {
            entry: entry("a", "Example.COM", 0, 0),
            upload_rate: 1024.0,
            download_rate: 0.0,
        };
        assert!(matches_query(&record, "example"));
        assert!(!matches_query(&record, "rule-not-found"));
        assert_eq!(format_bytes(1024), "1.0 KB");
        assert_eq!(format_rate(1024.0), "1.0 KB/s");
    }

    #[test]
    fn sort_state_cycles_descending_ascending_default() {
        let default = SortState::default();
        let descending = cycle_sort(default, SortColumn::Host);
        let ascending = cycle_sort(descending, SortColumn::Host);
        assert_eq!(descending.direction, SortDirection::Desc);
        assert_eq!(ascending.direction, SortDirection::Asc);
        assert_eq!(cycle_sort(ascending, SortColumn::Host), default);
    }

    #[test]
    fn two_tabs_filter_and_tri_state_sort_raw_values_separately() {
        let state = new_state();
        let mut first = entry("a", "alpha.example", 1, 20);
        first.rule = "MATCH-A".to_string();
        let mut second = entry("b", "beta.example", 2, 40);
        second.rule = "MATCH-B".to_string();
        let start = Instant::now();
        apply_snapshot(&state, snapshot(vec![first, second]), start);
        apply_snapshot(
            &state,
            snapshot(vec![entry("b", "beta.example", 3, 60)]),
            start + Duration::from_secs(1),
        );

        {
            let mut view = state.lock().unwrap();
            view.sort = SortState {
                column: Some(SortColumn::Download),
                direction: SortDirection::Desc,
            };
        }
        let view = state.lock().unwrap().clone();
        let active = visible_connections(&view);
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].identity, "b");

        let mut view = view;
        view.selected_tab = ConnectionTab::Closed;
        view.query = "alpha".to_string();
        let closed = visible_connections(&view);
        assert_eq!(closed.len(), 1);
        assert_eq!(closed[0].identity, "history-1");
        assert_eq!(project_rows(&view).len(), 1);
    }

    #[test]
    fn details_include_fixed_chinese_fields_and_hide_unknown_extensions() {
        let fields = detail_fields(&detail_record());
        let labels = fields
            .iter()
            .map(|(label, _)| label.as_str())
            .collect::<Vec<_>>();
        assert_eq!(labels, expected_detail_labels());
        assert!(!labels.iter().any(|label| label.contains("metadata.")));
        assert!(!labels.iter().any(|label| label.contains("extra")));
        assert_eq!(
            fields
                .iter()
                .find(|(label, _)| label == "源 IP 地理信息")
                .unwrap()
                .1,
            "CN"
        );
        assert_eq!(fields.len(), 35);
    }

    #[test]
    fn history_remove_matches_exactly_and_clear_isolated_from_active() {
        let state = new_state();
        let start = Instant::now();
        apply_snapshot(
            &state,
            snapshot(vec![
                entry("a", "a.example", 1, 1),
                entry("b", "b.example", 1, 1),
            ]),
            start,
        );
        apply_snapshot(
            &state,
            snapshot(vec![entry("b", "b.example", 2, 2)]),
            start + Duration::from_secs(1),
        );
        apply_snapshot(&state, snapshot(Vec::new()), start + Duration::from_secs(2));
        assert!(remove_history_local(&state, "history-1"));
        assert!(!remove_history_local(&state, "history-1"));
        assert!(clear_history_local(&state));
        let view = state.lock().unwrap();
        assert!(view.closed.is_empty());
        assert!(view.active_by_id.is_empty());
    }

    #[test]
    fn rejects_history_actions_when_busy() {
        let state = new_state();
        let start = Instant::now();
        apply_snapshot(&state, snapshot(vec![entry("a", "a.example", 1, 1)]), start);
        apply_snapshot(&state, snapshot(Vec::new()), start + Duration::from_secs(1));
        state.lock().unwrap().busy = true;
        assert!(!remove_history_local(&state, "history-1"));
        assert!(!clear_history_local(&state));
        assert_eq!(state.lock().unwrap().closed.len(), 1);
    }
}
