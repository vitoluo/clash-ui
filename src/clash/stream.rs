use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{OnceLock, RwLock};
use std::time::Duration;

use serde::de::DeserializeOwned;
use tokio::sync::{broadcast, mpsc};

use super::api::{ConnectionSnapshot, LogLine, MemorySnapshot, Traffic};

type ConnectionsChannel = (
    mpsc::Sender<ConnectionSnapshot>,
    std::sync::Mutex<Option<mpsc::Receiver<ConnectionSnapshot>>>,
);

static LOGS_TX: OnceLock<broadcast::Sender<LogLine>> = OnceLock::new();
static CONNS_CHANNEL: OnceLock<ConnectionsChannel> = OnceLock::new();
static TRAFFIC_TX: OnceLock<broadcast::Sender<Traffic>> = OnceLock::new();
static MEMORY_TX: OnceLock<broadcast::Sender<MemorySnapshot>> = OnceLock::new();
static MEMORY_LATEST: OnceLock<RwLock<Option<MemorySnapshot>>> = OnceLock::new();
static STARTED: AtomicBool = AtomicBool::new(false);
static STREAM_GENERATION: AtomicU64 = AtomicU64::new(0);

const CONNECTION_SNAPSHOT_CAPACITY: usize = 1;
const LOGS_PATH: &str = "/logs?level=debug&format=structured";
const RECONNECT_DELAY: Duration = Duration::from_secs(1);

fn memory_latest() -> &'static RwLock<Option<MemorySnapshot>> {
    MEMORY_LATEST.get_or_init(|| RwLock::new(None))
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

pub fn conns_rx() -> Option<mpsc::Receiver<ConnectionSnapshot>> {
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
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let generation = STREAM_GENERATION.load(Ordering::SeqCst);
    let logs = LOGS_TX.get().unwrap();
    let connections = CONNS_CHANNEL.get().unwrap().0.clone();
    let traffic = TRAFFIC_TX.get().unwrap();
    let memory = MEMORY_TX.get().unwrap();
    crate::runtime::spawn_task(connections_loop(connections, generation));
    crate::runtime::spawn_task(stream_loop(LOGS_PATH, logs, generation, |_| {}));
    crate::runtime::spawn_task(stream_loop("/traffic", traffic, generation, |_| {}));
    crate::runtime::spawn_task(stream_loop("/memory", memory, generation, update_memory));
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

async fn connections_loop(tx: mpsc::Sender<ConnectionSnapshot>, generation: u64) {
    loop {
        if !stream_is_current(generation) {
            return;
        }
        let Some(mut stream) = connect("/connections").await else {
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
            match stream.next::<ConnectionSnapshot>().await {
                Ok(Some(snapshot)) => {
                    if tx.send(snapshot).await.is_err() {
                        mark_stopped(generation);
                        return;
                    }
                }
                Ok(None) => break,
                Err(crate::network::websocket::Error::Json(_)) => {
                    crate::log::error(format_args!("WS /connections 消息解析失败"));
                }
                Err(_) => {
                    crate::log::error(format_args!("WS /connections 消息读取失败"));
                    break;
                }
            }
        }
        if !wait_to_reconnect(generation).await {
            return;
        }
    }
}

async fn stream_loop<T, F>(
    path: &'static str,
    tx: &'static broadcast::Sender<T>,
    generation: u64,
    on_value: F,
) where
    T: DeserializeOwned + Clone + Send + 'static,
    F: Fn(&T) + Send + Sync + 'static,
{
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
                    on_value(&value);
                    let _ = tx.send(value);
                }
                Ok(None) => break,
                Err(crate::network::websocket::Error::Json(_)) => {
                    crate::log::error(format_args!("WS {path} 消息解析失败"));
                }
                Err(_) => {
                    crate::log::error(format_args!("WS {path} 消息读取失败"));
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
    use super::{ConnectionSnapshot, CONNECTION_SNAPSHOT_CAPACITY, LOGS_PATH};

    #[test]
    fn connection_channel_applies_backpressure_at_one_pending_item() {
        let (sender, _receiver) = tokio::sync::mpsc::channel(CONNECTION_SNAPSHOT_CAPACITY);
        sender.try_send(ConnectionSnapshot::default()).unwrap();
        assert!(matches!(
            sender.try_send(ConnectionSnapshot::default()),
            Err(tokio::sync::mpsc::error::TrySendError::Full(_))
        ));
    }

    #[test]
    fn structured_log_stream_uses_expected_queries() {
        assert_eq!(LOGS_PATH, "/logs?level=debug&format=structured");
    }
}
