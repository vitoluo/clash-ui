use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock, RwLock};
use std::time::Duration;

use serde::de::DeserializeOwned;
use tokio::sync::{broadcast, mpsc};

use super::api::{ConnectionSnapshot, LogLine, MemorySnapshot, Traffic};

type ConnectionsChannel = (
    mpsc::Sender<ConnectionUpdate>,
    std::sync::Mutex<Option<mpsc::Receiver<ConnectionUpdate>>>,
);
type LogsChannel = (
    mpsc::Sender<LogUpdate>,
    std::sync::Mutex<Option<mpsc::Receiver<LogUpdate>>>,
);

static LOGS_CHANNEL: OnceLock<LogsChannel> = OnceLock::new();
static CONNS_CHANNEL: OnceLock<ConnectionsChannel> = OnceLock::new();
static TRAFFIC_TX: OnceLock<broadcast::Sender<TrafficUpdate>> = OnceLock::new();
static MEMORY_TX: OnceLock<broadcast::Sender<MemoryUpdate>> = OnceLock::new();
static MEMORY_LATEST: OnceLock<RwLock<Option<MemoryUpdate>>> = OnceLock::new();
static STREAM_TASKS: OnceLock<Mutex<Vec<tokio::task::JoinHandle<()>>>> = OnceLock::new();
static STREAM_LIFECYCLE: OnceLock<Mutex<()>> = OnceLock::new();
static STARTED: AtomicBool = AtomicBool::new(false);

const CONNECTION_SNAPSHOT_CAPACITY: usize = 64;
const LOG_CHANNEL_CAPACITY: usize = 64;
const LOGS_PATH: &str = "/logs?level=debug&format=structured";
const RECONNECT_DELAY: Duration = Duration::from_secs(1);

#[derive(Debug, Clone)]
pub struct ConnectionUpdate {
    pub core_generation: u64,
    pub snapshot: ConnectionSnapshot,
}

#[derive(Debug, Clone)]
pub struct LogUpdate {
    pub core_generation: u64,
    pub line: LogLine,
}

#[derive(Debug, Clone)]
pub struct TrafficUpdate {
    pub core_generation: u64,
    pub traffic: Traffic,
}

#[derive(Debug, Clone)]
pub struct MemoryUpdate {
    pub core_generation: u64,
    pub snapshot: MemorySnapshot,
}

#[derive(Debug)]
pub enum StreamError {
    Lock(&'static str),
    ReceiverTaken,
    NotInitialized(&'static str),
    Core(String),
}

impl std::fmt::Display for StreamError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Lock(name) => write!(formatter, "流状态锁被污染：{name}"),
            Self::ReceiverTaken => write!(formatter, "连接流接收端已被取用"),
            Self::NotInitialized(name) => write!(formatter, "流通道未初始化：{name}"),
            Self::Core(error) => write!(formatter, "读取核心会话失败：{error}"),
        }
    }
}

impl std::error::Error for StreamError {}

fn memory_latest() -> &'static RwLock<Option<MemoryUpdate>> {
    MEMORY_LATEST.get_or_init(|| RwLock::new(None))
}

fn stream_tasks() -> &'static Mutex<Vec<tokio::task::JoinHandle<()>>> {
    STREAM_TASKS.get_or_init(|| Mutex::new(Vec::new()))
}

fn stream_lifecycle() -> &'static Mutex<()> {
    STREAM_LIFECYCLE.get_or_init(|| Mutex::new(()))
}

fn abort_stream_tasks() -> Result<(), StreamError> {
    let mut tasks = stream_tasks()
        .lock()
        .map_err(|_| StreamError::Lock("后台流任务"))?;
    for task in tasks.drain(..) {
        task.abort();
    }
    Ok(())
}

fn update_memory(update: &MemoryUpdate) -> Result<bool, StreamError> {
    if !stream_is_current(update.core_generation) {
        return Ok(false);
    }
    let mut latest = memory_latest()
        .write()
        .map_err(|_| StreamError::Lock("最新内存快照"))?;
    if !stream_is_current(update.core_generation) {
        return Ok(false);
    }
    *latest = Some(update.clone());
    Ok(true)
}

fn stream_is_current(generation: u64) -> bool {
    super::core::generation() == generation
}

fn mark_stopped(generation: u64) {
    if stream_is_current(generation) {
        STARTED.store(false, Ordering::SeqCst);
    }
}

fn ensure_senders() {
    let _ = LOGS_CHANNEL.get_or_init(|| {
        let (sender, receiver) = mpsc::channel(LOG_CHANNEL_CAPACITY);
        (sender, std::sync::Mutex::new(Some(receiver)))
    });
    let _ = CONNS_CHANNEL.get_or_init(|| {
        let (sender, receiver) = mpsc::channel(CONNECTION_SNAPSHOT_CAPACITY);
        (sender, std::sync::Mutex::new(Some(receiver)))
    });
    let _ = TRAFFIC_TX.get_or_init(|| broadcast::channel(256).0);
    let _ = MEMORY_TX.get_or_init(|| broadcast::channel(64).0);
}

fn endpoint(path: &str) -> Result<Option<(String, String)>, StreamError> {
    let Some(snapshot) = super::core::get_controller_snapshot()
        .map_err(|error| StreamError::Core(error.to_string()))?
    else {
        return Ok(None);
    };
    Ok(Some((
        format!("ws://127.0.0.1:{}{path}", snapshot.port),
        format!("Bearer {}", snapshot.secret),
    )))
}

#[allow(dead_code)]
pub fn logs_rx() -> Result<mpsc::Receiver<LogUpdate>, StreamError> {
    ensure_senders();
    let (_, receiver) = LOGS_CHANNEL
        .get()
        .ok_or(StreamError::NotInitialized("日志"))?;
    receiver
        .lock()
        .map_err(|_| StreamError::Lock("日志接收端"))?
        .take()
        .ok_or(StreamError::ReceiverTaken)
}

pub fn conns_rx() -> Result<mpsc::Receiver<ConnectionUpdate>, StreamError> {
    ensure_senders();
    let (_, receiver) = CONNS_CHANNEL
        .get()
        .ok_or(StreamError::NotInitialized("连接"))?;
    receiver
        .lock()
        .map_err(|_| StreamError::Lock("连接接收端"))?
        .take()
        .ok_or(StreamError::ReceiverTaken)
}

pub fn traffic_rx() -> Result<broadcast::Receiver<TrafficUpdate>, StreamError> {
    ensure_senders();
    TRAFFIC_TX
        .get()
        .map(|sender| sender.subscribe())
        .ok_or(StreamError::NotInitialized("流量"))
}

#[allow(dead_code)]
pub fn memory_rx() -> Result<broadcast::Receiver<MemoryUpdate>, StreamError> {
    ensure_senders();
    MEMORY_TX
        .get()
        .map(|sender| sender.subscribe())
        .ok_or(StreamError::NotInitialized("内存"))
}

pub fn latest_memory() -> Result<Option<MemorySnapshot>, StreamError> {
    let generation = super::core::generation();
    Ok(memory_latest()
        .read()
        .map_err(|_| StreamError::Lock("最新内存快照"))?
        .as_ref()
        .filter(|update| update.core_generation == generation)
        .map(|update| update.snapshot.clone()))
}

/// 使当前核心会话的流任务失效，并清除缓存的核心内存。
pub fn reset() -> Result<(), StreamError> {
    STARTED.store(false, Ordering::SeqCst);
    let _lifecycle = stream_lifecycle()
        .lock()
        .map_err(|_| StreamError::Lock("流生命周期"))?;
    abort_stream_tasks()?;
    let mut latest = memory_latest()
        .write()
        .map_err(|_| StreamError::Lock("最新内存快照"))?;
    *latest = None;
    Ok(())
}

/// 启动后台 Clash 数据流。核心启动后调用，不依赖页面是否打开。
pub fn start() -> Result<(), StreamError> {
    ensure_senders();
    let _lifecycle = stream_lifecycle()
        .lock()
        .map_err(|_| StreamError::Lock("流生命周期"))?;

    let logs = LOGS_CHANNEL
        .get()
        .ok_or(StreamError::NotInitialized("日志"))?
        .0
        .clone();
    let connections = CONNS_CHANNEL
        .get()
        .ok_or(StreamError::NotInitialized("连接"))?
        .0
        .clone();
    let traffic = TRAFFIC_TX
        .get()
        .ok_or(StreamError::NotInitialized("流量"))?
        .clone();
    let memory = MEMORY_TX
        .get()
        .ok_or(StreamError::NotInitialized("内存"))?
        .clone();

    if STARTED.swap(true, Ordering::SeqCst) {
        return Ok(());
    }
    // 清除上一代自然结束但尚未从集合移除的句柄。
    if let Err(error) = abort_stream_tasks() {
        STARTED.store(false, Ordering::SeqCst);
        return Err(error);
    }
    let generation = super::core::generation();
    let tasks = vec![
        crate::runtime::spawn_task(stream_loop(LOGS_PATH, generation, move |value| {
            let logs = logs.clone();
            async move {
                logs.send(LogUpdate {
                    core_generation: generation,
                    line: value,
                })
                .await
                .is_ok()
            }
        })),
        crate::runtime::spawn_task(stream_loop("/connections", generation, move |snapshot| {
            let connections = connections.clone();
            async move {
                let update = ConnectionUpdate {
                    core_generation: generation,
                    snapshot,
                };
                connections.send(update).await.is_ok()
            }
        })),
        crate::runtime::spawn_task(stream_loop("/traffic", generation, move |value| {
            let traffic = traffic.clone();
            async move {
                let _ = traffic.send(TrafficUpdate {
                    core_generation: generation,
                    traffic: value,
                });
                true
            }
        })),
        crate::runtime::spawn_task(stream_loop("/memory", generation, move |value| {
            let memory = memory.clone();
            async move {
                let update = MemoryUpdate {
                    core_generation: generation,
                    snapshot: value,
                };
                match update_memory(&update) {
                    Ok(true) => {}
                    Ok(false) => return false,
                    Err(error) => {
                        crate::log::error(format_args!("更新核心内存快照失败：{error}"));
                        return false;
                    }
                }
                let _ = memory.send(update);
                true
            }
        })),
    ];
    let mut current = match stream_tasks().lock() {
        Ok(current) => current,
        Err(_) => {
            for task in tasks {
                task.abort();
            }
            STARTED.store(false, Ordering::SeqCst);
            return Err(StreamError::Lock("后台流任务"));
        }
    };
    *current = tasks;
    Ok(())
}

async fn connect(path: &str) -> Result<Option<crate::network::websocket::JsonStream>, StreamError> {
    let Some((url, authorization)) = endpoint(path)? else {
        return Ok(None);
    };
    match crate::network::websocket::connect_json(&url, Some(&authorization)).await {
        Ok(stream) => Ok(Some(stream)),
        Err(error) => {
            crate::log::error(format_args!("WS {path} 连接失败: {error}"));
            Ok(None)
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
        let Some(mut stream) = (match connect(path).await {
            Ok(stream) => stream,
            Err(error) => {
                crate::log::error(format_args!("WS {path} 获取核心会话失败：{error}"));
                mark_stopped(generation);
                return;
            }
        }) else {
            match super::core::get_controller_snapshot() {
                Ok(None) => {
                    mark_stopped(generation);
                    return;
                }
                Ok(Some(_)) => {}
                Err(error) => {
                    crate::log::error(format_args!("WS {path} 获取核心会话失败：{error}"));
                    mark_stopped(generation);
                    return;
                }
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
    match super::core::get_controller_snapshot() {
        Ok(Some(_)) => {}
        Ok(None) => {
            mark_stopped(generation);
            return false;
        }
        Err(error) => {
            crate::log::error(format_args!("等待 WS 重连时获取核心会话失败：{error}"));
            mark_stopped(generation);
            return false;
        }
    }
    tokio::time::sleep(RECONNECT_DELAY).await;
    true
}

#[cfg(test)]
mod tests {
    use super::{
        update_memory, ConnectionUpdate, MemoryUpdate, CONNECTION_SNAPSHOT_CAPACITY, LOGS_PATH,
        LOG_CHANNEL_CAPACITY,
    };

    #[test]
    fn connection_channel_has_fixed_capacity_64() {
        let (sender, _receiver) = tokio::sync::mpsc::channel(CONNECTION_SNAPSHOT_CAPACITY);
        let update = ConnectionUpdate {
            core_generation: 0,
            snapshot: super::ConnectionSnapshot::default(),
        };
        for _ in 0..CONNECTION_SNAPSHOT_CAPACITY {
            sender.try_send(update.clone()).unwrap();
        }
        assert!(matches!(
            sender.try_send(update),
            Err(tokio::sync::mpsc::error::TrySendError::Full(_))
        ));
        assert_eq!(CONNECTION_SNAPSHOT_CAPACITY, LOG_CHANNEL_CAPACITY);
    }

    #[test]
    fn log_channel_applies_backpressure_at_one_pending_item() {
        let (sender, _receiver) = tokio::sync::mpsc::channel(1);
        let update = super::LogUpdate {
            core_generation: 0,
            line: super::LogLine {
                time: "08:00:01".to_string(),
                level: "debug".to_string(),
                message: "日志".to_string(),
            },
        };
        sender.try_send(update.clone()).unwrap();
        assert!(matches!(
            sender.try_send(update),
            Err(tokio::sync::mpsc::error::TrySendError::Full(_))
        ));
    }

    #[test]
    fn log_channel_has_fixed_capacity_64() {
        let (sender, mut receiver) = tokio::sync::mpsc::channel(LOG_CHANNEL_CAPACITY);
        for sequence in 1..=LOG_CHANNEL_CAPACITY {
            sender
                .try_send(super::LogUpdate {
                    core_generation: 0,
                    line: super::LogLine {
                        time: format!("08:00:{sequence:02}"),
                        level: "debug".to_string(),
                        message: sequence.to_string(),
                    },
                })
                .unwrap();
        }
        assert!(matches!(
            sender.try_send(super::LogUpdate {
                core_generation: 0,
                line: super::LogLine {
                    time: "08:01:05".to_string(),
                    level: "debug".to_string(),
                    message: "overflow".to_string(),
                },
            }),
            Err(tokio::sync::mpsc::error::TrySendError::Full(_))
        ));
        assert_eq!(receiver.try_recv().unwrap().line.message, "1");
        assert!(receiver.try_recv().is_ok());
    }

    #[test]
    fn structured_log_stream_uses_expected_queries() {
        assert_eq!(LOGS_PATH, "/logs?level=debug&format=structured");
    }

    #[test]
    fn stale_memory_update_cannot_replace_current_snapshot() {
        let generation = crate::clash::core::generation();
        let current = MemoryUpdate {
            core_generation: generation,
            snapshot: super::MemorySnapshot {
                inuse: 100,
                oslimit: 200,
            },
        };
        assert!(update_memory(&current).expect("写入当前内存快照失败"));

        let stale = MemoryUpdate {
            core_generation: generation.wrapping_add(1),
            snapshot: super::MemorySnapshot {
                inuse: 999,
                oslimit: 999,
            },
        };
        assert!(!update_memory(&stale).expect("校验旧内存快照失败"));
        assert_eq!(super::latest_memory().unwrap().unwrap().inuse, 100);
        super::reset().expect("清理测试内存快照失败");
    }
}
