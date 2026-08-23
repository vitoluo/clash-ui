use std::sync::OnceLock;

use tokio::runtime::Runtime;

static RUNTIME: OnceLock<Runtime> = OnceLock::new();

fn runtime() -> &'static Runtime {
    RUNTIME.get_or_init(|| Runtime::new().expect("创建 tokio runtime 失败"))
}

/// 在共享运行时上阻塞执行异步任务。
pub fn block<F: std::future::Future>(future: F) -> F::Output {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => handle.block_on(future),
        Err(_) => runtime().block_on(future),
    }
}

/// 将异步任务提交到共享运行时。
pub fn spawn_task<F>(future: F) -> tokio::task::JoinHandle<F::Output>
where
    F: std::future::Future + Send + 'static,
    F::Output: Send + 'static,
{
    runtime().spawn(future)
}

/// 将阻塞任务提交到共享运行时的阻塞任务池。
pub fn spawn_blocking<F, R>(task: F) -> tokio::task::JoinHandle<R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    runtime().spawn_blocking(task)
}

#[cfg(test)]
mod tests {
    use super::{block, spawn_blocking};

    #[test]
    fn shared_blocking_pool_can_drive_sync_futures() {
        let task = spawn_blocking(|| block(async { 7_u8 }));
        assert_eq!(block(task).unwrap(), 7);
    }
}
