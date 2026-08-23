use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use slint::{ComponentHandle, Model, ModelRc, VecModel, Weak};
use tokio::sync::broadcast;

use crate::clash::api::LogLine;
use crate::constants::MAX_LOG_RECORDS;
use crate::{LogRow, LogsModel, MainWindow};

pub type SharedLogsState = Arc<Mutex<LogsViewState>>;

type UiNotifier = Arc<dyn Fn() + Send + Sync + 'static>;

pub(crate) fn bind_callbacks(window: &MainWindow, state: SharedLogsState) {
    let weak = window.as_weak();
    window.global::<LogsModel>().on_select_tab({
        let weak = weak.clone();
        let state = state.clone();
        move |index| {
            set_tab(&state, index);
            if let Some(window) = weak.upgrade() {
                sync_ui(&window, &state);
            }
        }
    });
    window.global::<LogsModel>().on_select_all_level({
        let weak = weak.clone();
        let state = state.clone();
        move |index| {
            set_all_level(&state, index);
            if let Some(window) = weak.upgrade() {
                sync_ui(&window, &state);
            }
        }
    });
    window.global::<LogsModel>().on_search_changed({
        let weak = weak.clone();
        let state = state.clone();
        move |query| {
            set_query(&state, query.into());
            if let Some(window) = weak.upgrade() {
                sync_ui(&window, &state);
            }
        }
    });
    window.global::<LogsModel>().on_toggle_pause({
        let weak = weak.clone();
        let state = state.clone();
        move || {
            toggle_pause(&state);
            if let Some(window) = weak.upgrade() {
                sync_ui(&window, &state);
            }
        }
    });
    window.global::<LogsModel>().on_clear({
        let weak = weak.clone();
        move || {
            clear(&state);
            if let Some(window) = weak.upgrade() {
                sync_ui(&window, &state);
            }
        }
    });
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Error,
    Warning,
    Info,
    Debug,
}

impl LogLevel {
    pub fn parse(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "error" => Some(Self::Error),
            "warning" | "warn" => Some(Self::Warning),
            "info" => Some(Self::Info),
            "debug" => Some(Self::Debug),
            _ => None,
        }
    }

    pub fn severity(self) -> u8 {
        match self {
            Self::Debug => 0,
            Self::Info => 1,
            Self::Warning => 2,
            Self::Error => 3,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Error => "ERROR",
            Self::Warning => "WARN",
            Self::Info => "INFO",
            Self::Debug => "DEBUG",
        }
    }

    pub fn index(self) -> i32 {
        match self {
            Self::Error => 0,
            Self::Warning => 1,
            Self::Info => 2,
            Self::Debug => 3,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogRecord {
    pub sequence: u64,
    pub time: String,
    pub level: LogLevel,
    pub message: String,
    message_lowercase: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogTab {
    All,
    Error,
    Warning,
    Info,
    Debug,
}

impl LogTab {
    fn exact_level(self) -> Option<LogLevel> {
        match self {
            Self::All => None,
            Self::Error => Some(LogLevel::Error),
            Self::Warning => Some(LogLevel::Warning),
            Self::Info => Some(LogLevel::Info),
            Self::Debug => Some(LogLevel::Debug),
        }
    }
}

#[derive(Debug, Clone)]
pub struct LogsViewState {
    records: VecDeque<LogRecord>,
    pub selected_tab: LogTab,
    pub all_level: LogLevel,
    pub query: String,
    query_lowercase: String,
    pub auto_scroll: bool,
    next_sequence: u64,
    filter_revision: u64,
    ui_projection: LogsUiProjectionState,
}

#[derive(Debug, Clone, Default)]
struct LogsUiProjectionState {
    last_sequence: Option<u64>,
    filter_revision: u64,
    visible_sequences: VecDeque<u64>,
}

#[derive(Clone)]
pub struct LogsRecorder {
    state: SharedLogsState,
    notifier: Arc<Mutex<Option<UiNotifier>>>,
}

fn lock_state(state: &SharedLogsState) -> MutexGuard<'_, LogsViewState> {
    state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn lock_notifier(notifier: &Arc<Mutex<Option<UiNotifier>>>) -> MutexGuard<'_, Option<UiNotifier>> {
    notifier
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub fn new_state() -> SharedLogsState {
    Arc::new(Mutex::new(LogsViewState::default()))
}

#[cfg(test)]
pub fn read_state(state: &SharedLogsState) -> LogsViewState {
    lock_state(state).clone()
}

pub fn start_recorder(mut receiver: broadcast::Receiver<LogLine>) -> LogsRecorder {
    let recorder = LogsRecorder {
        state: new_state(),
        notifier: Arc::new(Mutex::new(None)),
    };
    let worker = recorder.clone();
    crate::runtime::spawn_task(async move {
        loop {
            let first = match receiver.recv().await {
                Ok(line) => line,
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    crate::log::error(format_args!("日志记录器跳过 {skipped} 条过期消息"));
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => break,
            };
            let mut accepted = lock_state(&worker.state).append_line(first);
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(50);
            loop {
                match tokio::time::timeout_at(deadline, receiver.recv()).await {
                    Ok(Ok(line)) => accepted |= lock_state(&worker.state).append_line(line),
                    Ok(Err(broadcast::error::RecvError::Lagged(skipped))) => {
                        crate::log::error(format_args!("日志记录器跳过 {skipped} 条过期消息"));
                    }
                    Ok(Err(broadcast::error::RecvError::Closed)) => {
                        if accepted {
                            worker.notify();
                        }
                        return;
                    }
                    Err(_) => break,
                }
            }
            if accepted {
                worker.notify();
            }
        }
    });
    recorder
}

impl LogsRecorder {
    pub fn state(&self) -> SharedLogsState {
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

pub fn tab_from_index(index: i32) -> Option<LogTab> {
    match index {
        0 => Some(LogTab::All),
        1 => Some(LogTab::Error),
        2 => Some(LogTab::Warning),
        3 => Some(LogTab::Info),
        4 => Some(LogTab::Debug),
        _ => None,
    }
}

pub fn level_from_index(index: i32) -> Option<LogLevel> {
    match index {
        0 => Some(LogLevel::Error),
        1 => Some(LogLevel::Warning),
        2 => Some(LogLevel::Info),
        3 => Some(LogLevel::Debug),
        _ => None,
    }
}

pub fn set_tab(state: &SharedLogsState, index: i32) {
    if let Some(tab) = tab_from_index(index) {
        lock_state(state).set_tab(tab);
    }
}

pub fn set_all_level(state: &SharedLogsState, index: i32) {
    if let Some(level) = level_from_index(index) {
        lock_state(state).set_all_level(level);
    }
}

pub fn set_query(state: &SharedLogsState, query: String) {
    lock_state(state).set_query(query);
}

pub fn toggle_pause(state: &SharedLogsState) {
    lock_state(state).toggle_pause();
}

pub fn clear(state: &SharedLogsState) {
    lock_state(state).clear();
}

/// 清空核心运行期间的日志数据。
pub fn clear_runtime(state: &SharedLogsState) {
    clear(state);
}

fn project_row(record: &LogRecord) -> LogRow {
    LogRow {
        time: record.time.clone().into(),
        level: record.level.label().into(),
        message: record.message.clone().into(),
        level_index: record.level.index(),
    }
}

#[cfg(test)]
pub fn project_rows(state: &LogsViewState) -> Vec<LogRow> {
    state
        .visible_records()
        .into_iter()
        .map(project_row)
        .collect()
}

fn tab_index(tab: LogTab) -> i32 {
    match tab {
        LogTab::All => 0,
        LogTab::Error => 1,
        LogTab::Warning => 2,
        LogTab::Info => 3,
        LogTab::Debug => 4,
    }
}

pub fn sync_ui(window: &MainWindow, state: &SharedLogsState) {
    let model = window.global::<LogsModel>();
    let current_rows = model.get_rows();
    if current_rows
        .as_any()
        .downcast_ref::<VecModel<LogRow>>()
        .is_none()
    {
        model.set_rows(ModelRc::new(VecModel::default()));
    }
    let rows = model.get_rows();
    let rows = rows
        .as_any()
        .downcast_ref::<VecModel<LogRow>>()
        .expect("日志模型必须是 VecModel");
    let mut state = lock_state(state);
    sync_projection(rows, &mut state);
    model.set_selected_tab(tab_index(state.selected_tab));
    model.set_all_level_index(state.all_level.index());
    model.set_query(state.query.clone().into());
    model.set_paused(state.paused());
}

fn sync_projection(model: &VecModel<LogRow>, state: &mut LogsViewState) {
    let oldest = state.records.front().map(|record| record.sequence);
    let newest = state.records.back().map(|record| record.sequence);
    let projection_invalid = state.ui_projection.filter_revision != state.filter_revision
        || state.ui_projection.visible_sequences.len() != model.row_count()
        || matches!(
            (state.ui_projection.last_sequence, newest),
            (Some(last), Some(current)) if current < last
        );

    if projection_invalid {
        let visible = state
            .records
            .iter()
            .filter(|record| state.matches_record(record))
            .map(|record| (record.sequence, project_row(record)))
            .collect::<Vec<_>>();
        sync_log_rows(model, visible.iter().map(|(_, row)| row.clone()).collect());
        state.ui_projection.visible_sequences =
            visible.into_iter().map(|(sequence, _)| sequence).collect();
        state.ui_projection.last_sequence = newest;
        state.ui_projection.filter_revision = state.filter_revision;
        return;
    }

    if let Some(oldest) = oldest {
        while state
            .ui_projection
            .visible_sequences
            .front()
            .is_some_and(|sequence| *sequence < oldest)
        {
            state.ui_projection.visible_sequences.pop_front();
            model.remove(0);
        }
    }

    let last_sequence = state.ui_projection.last_sequence.unwrap_or(0);
    for record in state
        .records
        .iter()
        .filter(|record| record.sequence > last_sequence)
    {
        if state.matches_record(record) {
            model.push(project_row(record));
            state
                .ui_projection
                .visible_sequences
                .push_back(record.sequence);
        }
    }
    state.ui_projection.last_sequence = newest.or(state.ui_projection.last_sequence);
}

fn sync_log_rows(model: &VecModel<LogRow>, rows: Vec<LogRow>) {
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

pub fn attach_ui(recorder: &LogsRecorder, weak: Weak<MainWindow>) {
    let state = recorder.state();
    let callback_state = state.clone();
    let callback_weak = weak.clone();
    let pending = Arc::new(AtomicBool::new(false));
    recorder.set_notifier(Arc::new(move || {
        if pending.swap(true, Ordering::AcqRel) {
            return;
        }
        let state = callback_state.clone();
        let weak = callback_weak.clone();
        let event_pending = pending.clone();
        if slint::invoke_from_event_loop(move || {
            if let Some(window) = weak.upgrade() {
                if window.global::<crate::AppState>().get_current_page() == 6 {
                    sync_ui(&window, &state);
                }
            }
            event_pending.store(false, Ordering::Release);
        })
        .is_err()
        {
            pending.store(false, Ordering::Release);
        }
    }));
    if let Some(window) = weak.upgrade() {
        sync_ui(&window, &state);
    }
}

impl Default for LogsViewState {
    fn default() -> Self {
        Self {
            records: VecDeque::new(),
            selected_tab: LogTab::All,
            all_level: LogLevel::Debug,
            query: String::new(),
            query_lowercase: String::new(),
            auto_scroll: true,
            next_sequence: 1,
            filter_revision: 1,
            ui_projection: LogsUiProjectionState::default(),
        }
    }
}

impl LogsViewState {
    #[cfg(test)]
    pub fn records(&self) -> &VecDeque<LogRecord> {
        &self.records
    }

    pub fn append_line(&mut self, line: LogLine) -> bool {
        let Some(level) = LogLevel::parse(&line.level) else {
            crate::log::error(format_args!("忽略未知日志级别: {}", line.level));
            return false;
        };

        let message = line.message;
        let record = LogRecord {
            sequence: self.next_sequence,
            time: line.time,
            level,
            message_lowercase: message.to_lowercase(),
            message,
        };
        self.next_sequence = self.next_sequence.wrapping_add(1).max(1);
        self.records.push_back(record);
        while self.records.len() > MAX_LOG_RECORDS {
            self.records.pop_front();
        }
        true
    }

    #[cfg(test)]
    pub fn visible_records(&self) -> Vec<&LogRecord> {
        self.records()
            .iter()
            .filter(|record| {
                self.matches_level(record) && Self::matches_query(record, &self.query_lowercase)
            })
            .collect()
    }

    pub fn set_tab(&mut self, tab: LogTab) {
        if self.selected_tab != tab {
            self.selected_tab = tab;
            self.bump_filter_revision();
        }
    }

    pub fn set_all_level(&mut self, level: LogLevel) {
        if self.all_level != level {
            self.all_level = level;
            self.bump_filter_revision();
        }
    }

    pub fn set_query(&mut self, query: String) {
        if self.query != query {
            self.query_lowercase = query.to_lowercase();
            self.query = query;
            self.bump_filter_revision();
        }
    }

    pub fn toggle_pause(&mut self) {
        self.auto_scroll = !self.auto_scroll;
    }

    pub fn clear(&mut self) {
        self.records.clear();
        self.bump_filter_revision();
    }

    pub fn paused(&self) -> bool {
        !self.auto_scroll
    }

    fn matches_level(&self, record: &LogRecord) -> bool {
        match self.selected_tab.exact_level() {
            Some(level) => record.level == level,
            None => record.level.severity() >= self.all_level.severity(),
        }
    }

    fn matches_query(record: &LogRecord, query: &str) -> bool {
        query.is_empty() || record.message_lowercase.contains(query)
    }

    fn matches_record(&self, record: &LogRecord) -> bool {
        self.matches_level(record) && Self::matches_query(record, &self.query_lowercase)
    }

    fn bump_filter_revision(&mut self) {
        self.filter_revision = self.filter_revision.wrapping_add(1).max(1);
    }
}

#[cfg(test)]
mod tests {
    use std::thread;
    use std::time::{Duration, Instant};

    use super::{
        clear_runtime, new_state, project_rows, read_state, start_recorder, sync_projection,
        LogLevel, LogTab, LogsViewState, MAX_LOG_RECORDS,
    };
    use crate::clash::api::LogLine;
    use slint::{Model, ModelRc, VecModel};
    use std::rc::Rc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tokio::sync::broadcast;

    fn line(sequence: u64, level: &str, message: &str) -> LogLine {
        LogLine {
            time: format!("08:00:{sequence:02}"),
            level: level.to_string(),
            message: message.to_string(),
        }
    }

    fn sample_state() -> LogsViewState {
        let mut state = LogsViewState::default();
        assert!(state.append_line(line(1, "debug", "debug message")));
        assert!(state.append_line(line(2, "info", "Info message")));
        assert!(state.append_line(line(3, "warn", "warning message")));
        assert!(state.append_line(line(4, "error", "ERROR message")));
        state
    }

    #[test]
    fn defaults_to_all_debug_and_all_levels() {
        let state = sample_state();
        assert_eq!(state.selected_tab, LogTab::All);
        assert_eq!(state.all_level, LogLevel::Debug);
        assert_eq!(state.visible_records().len(), 4);
    }

    #[test]
    fn filters_all_level_by_severity() {
        let mut state = sample_state();
        state.set_all_level(LogLevel::Error);
        assert_eq!(state.visible_records().len(), 1);
        state.set_all_level(LogLevel::Warning);
        assert_eq!(state.visible_records().len(), 2);
        state.set_all_level(LogLevel::Info);
        assert_eq!(state.visible_records().len(), 3);
        state.set_all_level(LogLevel::Debug);
        assert_eq!(state.visible_records().len(), 4);
    }

    #[test]
    fn filters_exactly_by_four_level_tabs() {
        let mut state = sample_state();
        for (tab, expected) in [
            (LogTab::Error, LogLevel::Error),
            (LogTab::Warning, LogLevel::Warning),
            (LogTab::Info, LogLevel::Info),
            (LogTab::Debug, LogLevel::Debug),
        ] {
            state.set_tab(tab);
            let records = state.visible_records();
            assert_eq!(records.len(), 1);
            assert_eq!(records[0].level, expected);
        }
    }

    #[test]
    fn search_only_message_with_unicode_lowercase() {
        let mut state = sample_state();
        state.set_query("INFO MESSAGE".to_string());
        assert_eq!(state.visible_records().len(), 1);
        assert_eq!(state.visible_records()[0].level, LogLevel::Info);
        state.set_query("08:00".to_string());
        assert!(state.visible_records().is_empty());
    }

    #[test]
    fn keeps_latest_1000_records_in_sequence_order() {
        let mut state = LogsViewState::default();
        for index in 1..=1001 {
            assert!(state.append_line(line(index, "debug", "message")));
        }
        assert_eq!(state.records().len(), MAX_LOG_RECORDS);
        assert_eq!(state.records().front().unwrap().sequence, 2);
        assert_eq!(state.records().back().unwrap().sequence, 1001);
    }

    #[test]
    fn continues_receiving_after_clear() {
        let mut state = sample_state();
        state.clear();
        assert!(state.records().is_empty());
        assert!(state.append_line(line(5, "info", "after clear")));
        assert_eq!(state.records().len(), 1);
        assert_eq!(state.records().front().unwrap().sequence, 5);
    }

    #[test]
    fn clear_runtime_clears_shared_records() {
        let state = new_state();
        {
            let mut view = state.lock().unwrap();
            assert!(view.append_line(line(1, "info", "核心日志")));
        }

        clear_runtime(&state);

        assert!(read_state(&state).records().is_empty());
    }

    #[test]
    fn normalize_warning_and_reject_unknown_level() {
        let mut state = LogsViewState::default();
        assert!(state.append_line(line(1, "warning", "one")));
        assert!(state.append_line(line(2, "warn", "two")));
        assert!(!state.append_line(line(3, "silent", "three")));
        assert_eq!(state.records().len(), 2);
        assert!(state
            .records()
            .iter()
            .all(|record| record.level == LogLevel::Warning));
    }

    #[test]
    fn preserves_dropdown_state_after_leaving_all() {
        let mut state = sample_state();
        state.set_all_level(LogLevel::Warning);
        state.set_tab(LogTab::Error);
        assert_eq!(state.visible_records().len(), 1);
        state.set_tab(LogTab::All);
        assert_eq!(state.all_level, LogLevel::Warning);
        assert_eq!(state.visible_records().len(), 2);
    }

    #[test]
    fn projection_exports_only_time_level_and_full_message() {
        let state = sample_state();
        let rows = project_rows(&state);
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[0].time.to_string(), "08:00:01");
        assert_eq!(rows[0].level.to_string(), "DEBUG");
        assert_eq!(rows[0].message.to_string(), "debug message");
        assert_eq!(rows[0].level_index, 3);
    }

    #[test]
    fn incremental_projection_reuses_model_and_rebuilds_only_after_filter_change() {
        let mut state = LogsViewState::default();
        let rows = Rc::new(VecModel::default());
        let identity = ModelRc::from(rows.clone());
        assert!(state.append_line(line(1, "info", "first")));
        sync_projection(&rows, &mut state);
        assert_eq!(rows.row_count(), 1);

        assert!(state.append_line(line(2, "debug", "second")));
        sync_projection(&rows, &mut state);
        assert_eq!(identity, ModelRc::from(rows.clone()));
        assert_eq!(rows.row_count(), 2);

        state.set_query("second".to_string());
        sync_projection(&rows, &mut state);
        assert_eq!(identity, ModelRc::from(rows.clone()));
        assert_eq!(rows.row_count(), 1);
        assert_eq!(rows.row_data(0).unwrap().message.as_str(), "second");
    }

    #[test]
    fn recorder_batches_burst_notifications() {
        let (sender, receiver) = broadcast::channel(32);
        let recorder = start_recorder(receiver);
        let notifications = Arc::new(AtomicUsize::new(0));
        let count = notifications.clone();
        recorder.set_notifier(Arc::new(move || {
            count.fetch_add(1, Ordering::AcqRel);
        }));
        for sequence in 1..=20 {
            sender.send(line(sequence, "debug", "burst")).unwrap();
        }
        thread::sleep(Duration::from_millis(150));
        assert_eq!(notifications.load(Ordering::Acquire), 1);
    }

    #[test]
    fn recorder_initializes_before_receive_and_consumes_after_lag() {
        let (sender, receiver) = broadcast::channel(1);
        let recorder = start_recorder(receiver);
        let state = recorder.state();
        for sequence in 1..=20 {
            sender
                .send(line(sequence, "debug", &format!("message-{sequence}")))
                .unwrap();
        }

        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if read_state(&state)
                .records()
                .back()
                .map(|record| record.message == "message-20")
                .unwrap_or(false)
            {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            read_state(&state).records().back().unwrap().message,
            "message-20"
        );
        drop(sender);
    }

    #[test]
    fn state_is_independent_and_projects_cached_items_before_attach() {
        let state = new_state();
        {
            let mut state_guard = state.lock().unwrap();
            assert!(state_guard.append_line(line(1, "info", "窗口创建前")));
        }
        let rows = project_rows(&read_state(&state));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].message.to_string(), "窗口创建前");
    }
}
