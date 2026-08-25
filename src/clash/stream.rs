use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock, RwLock};
use std::time::Duration;

use serde::de::DeserializeOwned;
use tokio::sync::{broadcast, mpsc};

use super::api::{ConnectionSnapshot, LogLine, MemorySnapshot, Traffic};

type ConnectionsChannel = (
    mpsc::Sender<ConnectionUpdate>,
    std::sync::Mutex<Option<mpsc::Receiver<ConnectionUpdate>>>,
);

static LOGS_TX: OnceLock<broadcast::Sender<LogLine>> = OnceLock::new();
static CONNS_CHANNEL: OnceLock<ConnectionsChannel> = OnceLock::new();
static TRAFFIC_TX: OnceLock<broadcast::Sender<Traffic>> = OnceLock::new();
static MEMORY_TX: OnceLock<broadcast::Sender<MemorySnapshot>> = OnceLock::new();
static MEMORY_LATEST: OnceLock<RwLock<Option<MemorySnapshot>>> = OnceLock::new();
static STREAM_TASKS: OnceLock<Mutex<Vec<tokio::task::JoinHandle<()>>>> = OnceLock::new();
static STREAM_LIFECYCLE: OnceLock<Mutex<()>> = OnceLock::new();
static STARTED: AtomicBool = AtomicBool::new(false);
static STREAM_GENERATION: AtomicU64 = AtomicU64::new(0);

const CONNECTION_SNAPSHOT_CAPACITY: usize = 1;
const LOGS_PATH: &str = "/logs?level=debug&format=structured";
const RECONNECT_DELAY: Duration = Duration::from_secs(1);

#[derive(Debug, Clone)]
pub struct ConnectionUpdate {
    pub generation: u64,
    pub snapshot: ConnectionSnapshot,
}

fn memory_latest() -> &'static RwLock<Option<MemorySnapshot>> {
    MEMORY_LATEST.get_or_init(|| RwLock::new(None))
}

fn stream_tasks() -> &'static Mutex<Vec<tokio::task::JoinHandle<()>>> {
    STREAM_TASKS.get_or_init(|| Mutex::new(Vec::new()))
}

fn stream_lifecycle() -> &'static Mutex<()> {
    STREAM_LIFECYCLE.get_or_init(|| Mutex::new(()))
}

fn abort_stream_tasks() {
    if let Ok(mut tasks) = stream_tasks().lock() {
        for task in tasks.drain(..) {
            task.abort();
        }
    }
}

fn update_memory(snapshot: &MemorySnapshot) {
    if let Ok(mut latest) = memory_latest().write() {
        *latest = Some(snapshot.clone());
    }
}

fn stream_is_current(generation: u64) -> bool {
    STREAM_GENERATION.load(Ordering::SeqCst) == generation
}

fn mark_stopped(generation: u64) {
    if stream_is_current(generation) {
        STARTED.store(false, Ordering::SeqCst);
    }
}

fn ensure_senders() {
    let _ = LOGS_TX.get_or_init(|| broadcast::channel(1024).0);
    let _ = CONNS_CHANNEL.get_or_init(|| {
        let (sender, receiver) = mpsc::channel(CONNECTION_SNAPSHOT_CAPACITY);
        (sender, std::sync::Mutex::new(Some(receiver)))
    });
    let _ = TRAFFIC_TX.get_or_init(|| broadcast::channel(256).0);
    let _ = MEMORY_TX.get_or_init(|| broadcast::channel(64).0);
}

fn endpoint(path: &str) -> Option<(String, String)> {
    let snapshot = super::core::get_controller_snapshot()?;
    Some((
        format!("ws://127.0.0.1:{}{path}", snapshot.port),
        format!("Bearer {}", snapshot.secret),
    ))
}

#[allow(dead_code)]
pub fn logs_rx() -> Option<broadcast::Receiver<LogLine>> {
    ensure_senders();
    LOGS_TX.get().map(|sender| sender.subscribe())
}

pub fn conns_rx() -> Option<mpsc::Receiver<ConnectionUpdate>> {
    ensure_senders();
    CONNS_CHANNEL
        .get()
        .and_then(|(_, receiver)| receiver.lock().ok()?.take())
}

pub fn traffic_rx() -> Option<broadcast::Receiver<Traffic>> {
    ensure_senders();
    TRAFFIC_TX.get().map(|sender| sender.subscribe())
}

#[allow(dead_code)]
pub fn memory_rx() -> Option<broadcast::Receiver<MemorySnapshot>> {
    ensure_senders();
    MEMORY_TX.get().map(|sender| sender.subscribe())
}

pub fn latest_memory() -> Option<MemorySnapshot> {
    memory_latest()
        .read()
        .ok()
        .and_then(|snapshot| snapshot.clone())
}

/// 使当前核心会话的流任务失效，并清除缓存的核心内存。
pub fn reset() {
    STREAM_GENERATION.fetch_add(1, Ordering::SeqCst);
    STARTED.store(false, Ordering::SeqCst);
    let _lifecycle = stream_lifecycle().lock().ok();
    abort_stream_tasks();
    if let Ok(mut latest) = memory_latest().write() {
        *latest = None;
    }
}

/// 返回当前核心流会话代次，供页面缓存判定是否失效。
pub fn runtime_generation() -> u64 {
    STREAM_GENERATION.load(Ordering::SeqCst)
}

/// 启动后台 Clash 数据流。核心启动后调用，不依赖页面是否打开。
pub fn start() {
    ensure_senders();
    let _lifecycle = stream_lifecycle().lock().ok();
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    // 清除上一代自然结束但尚未从集合移除的句柄。
    abort_stream_tasks();
    let generation = STREAM_GENERATION.load(Ordering::SeqCst);
    let logs = LOGS_TX.get().unwrap().clone();
    let connections = CONNS_CHANNEL.get().unwrap().0.clone();
    let traffic = TRAFFIC_TX.get().unwrap().clone();
    let memory = MEMORY_TX.get().unwrap().clone();
    let tasks = vec![
        crate::runtime::spawn_task(stream_loop(LOGS_PATH, generation, move |value| {
            let logs = logs.clone();
            async move {
                let _ = logs.send(value);
                true
            }
        })),
        crate::runtime::spawn_task(stream_loop("/connections", generation, move |snapshot| {
            let connections = connections.clone();
            async move {
                let update = ConnectionUpdate {
                    generation,
                    snapshot,
                };
                connections.send(update).await.is_ok()
            }
        })),
        crate::runtime::spawn_task(stream_loop("/traffic", generation, move |value| {
            let traffic = traffic.clone();
            async move {
                let _ = traffic.send(value);
                true
            }
        })),
        crate::runtime::spawn_task(stream_loop("/memory", generation, move |value| {
            let memory = memory.clone();
            async move {
                update_memory(&value);
                let _ = memory.send(value);
                true
            }
        })),
    ];
    if let Ok(mut current) = stream_tasks().lock() {
        *current = tasks;
    }
}

async fn connect(path: &str) -> Option<crate::network::websocket::JsonStream> {
    let (url, authorization) = endpoint(path)?;
    match crate::network::websocket::connect_json(&url, Some(&authorization)).await {
        Ok(stream) => Some(stream),
        Err(error) => {
            crate::log::error(format_args!("WS {path} 连接失败: {error}"));
            None
        }
    }
}

async fn stream_loop<T, F, Fut>(path: &'static str, generation: u64, on_value: F)
where
    T: DeserializeOwned + Clone + Send + 'static,
    F: FnMut(T) -> Fut + Send + 'static,
    Fut: Future<Output = bool> + Send + 'static,
{
    let mut on_value = on_value;
    loop {
        if !stream_is_current(generation) {
            return;
        }
        let Some(mut stream) = connect(path).await else {
            if super::core::get_controller_snapshot().is_none() {
                mark_stopped(generation);
                return;
            }
            tokio::time::sleep(RECONNECT_DELAY).await;
            continue;
        };
        loop {
            if !stream_is_current(generation) {
                return;
            }
            match stream.next::<T>().await {
                Ok(Some(value)) => {
                    if !stream_is_current(generation) {
                        return;
                    }
                    if !on_value(value).await {
                        mark_stopped(generation);
                        return;
                    }
                }
                Ok(None) => break,
                Err(error @ crate::network::websocket::Error::Json(_)) => {
                    crate::log::error(format_args!("WS {path} 消息解析失败：{error}"));
                }
                Err(error) => {
                    crate::log::error(format_args!("WS {path} 消息读取失败：{error}"));
                    break;
                }
            }
        }
        if !wait_to_reconnect(generation).await {
            return;
        }
    }
}

async fn wait_to_reconnect(generation: u64) -> bool {
    if !stream_is_current(generation) {
        return false;
    }
    if super::core::get_controller_snapshot().is_none() {
        mark_stopped(generation);
        return false;
    }
    tokio::time::sleep(RECONNECT_DELAY).await;
    true
}

#[cfg(test)]
mod tests {
    use super::{ConnectionUpdate, CONNECTION_SNAPSHOT_CAPACITY, LOGS_PATH};

    #[test]
    fn connection_channel_applies_backpressure_at_one_pending_item() {
        let (sender, _receiver) = tokio::sync::mpsc::channel(CONNECTION_SNAPSHOT_CAPACITY);
        let update = ConnectionUpdate {
            generation: 0,
            snapshot: super::ConnectionSnapshot::default(),
        };
        sender.try_send(update.clone()).unwrap();
        assert!(matches!(
            sender.try_send(update),
            Err(tokio::sync::mpsc::error::TrySendError::Full(_))
        ));
    }

    #[test]
    fn structured_log_stream_uses_expected_queries() {
        assert_eq!(LOGS_PATH, "/logs?level=debug&format=structured");
    }
}
