use std::any::Any;
use std::panic::{self, AssertUnwindSafe};
use std::sync::OnceLock;

use futures_util::FutureExt;
use tokio::runtime::{Builder, Runtime};

static RUNTIME: OnceLock<Runtime> = OnceLock::new();

// 共享运行时的异步工作线程数，覆盖应用的少量长期 I/O 任务。
const RUNTIME_WORKER_THREADS: usize = 2;
// 共享运行时的阻塞任务线程上限，避免桌面端长期保留过大的线程池。
const RUNTIME_MAX_BLOCKING_THREADS: usize = 8;

fn runtime() -> &'static Runtime {
    RUNTIME.get_or_init(|| {
        install_abort_panic_hook();
        Builder::new_multi_thread()
            .worker_threads(RUNTIME_WORKER_THREADS)
            .max_blocking_threads(RUNTIME_MAX_BLOCKING_THREADS)
            .enable_all()
            .build()
            .unwrap_or_else(|error| {
                crate::log::error(format_args!("创建 Tokio 运行时失败：{error}"));
                panic!("创建 Tokio 运行时失败：{error}");
            })
    })
}

fn panic_message(payload: &(dyn Any + Send)) -> &str {
    if let Some(message) = payload.downcast_ref::<&str>() {
        message
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message
    } else {
        "未知 panic"
    }
}

#[cfg(panic = "abort")]
fn install_abort_panic_hook() {
    let previous = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        let message = panic_message(info.payload());
        if let Some(location) = info.location() {
            crate::log::error(format_args!(
                "应用发生 panic：{message}（{}:{}）",
                location.file(),
                location.line()
            ));
        } else {
            crate::log::error(format_args!("应用发生 panic：{message}"));
        }
        previous(info);
    }));
}

#[cfg(not(panic = "abort"))]
fn install_abort_panic_hook() {}

fn resume_panic(task_kind: &str, payload: Box<dyn Any + Send>) -> ! {
    let message = panic_message(payload.as_ref());
    crate::log::error(format_args!("{task_kind}发生 panic：{message}"));
    panic::resume_unwind(payload)
}

/// 在共享运行时上阻塞执行异步任务。
pub fn block<F: std::future::Future>(future: F) -> F::Output {
    let result = panic::catch_unwind(AssertUnwindSafe(
        || match tokio::runtime::Handle::try_current() {
            Ok(handle) => handle.block_on(future),
            Err(_) => runtime().block_on(future),
        },
    ));
    match result {
        Ok(output) => output,
        Err(payload) => resume_panic("阻塞异步任务", payload),
    }
}

/// 将异步任务提交到共享运行时。
pub fn spawn_task<F>(future: F) -> tokio::task::JoinHandle<F::Output>
where
    F: std::future::Future + Send + 'static,
    F::Output: Send + 'static,
{
    runtime().spawn(async move {
        match AssertUnwindSafe(future).catch_unwind().await {
            Ok(output) => output,
            Err(payload) => resume_panic("异步任务", payload),
        }
    })
}

/// 将阻塞任务提交到共享运行时的阻塞任务池。
pub fn spawn_blocking<F, R>(task: F) -> tokio::task::JoinHandle<R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    runtime().spawn_blocking(move || match panic::catch_unwind(AssertUnwindSafe(task)) {
        Ok(output) => output,
        Err(payload) => resume_panic("阻塞任务", payload),
    })
}

#[cfg(test)]
mod tests {
    use super::{
        block, spawn_blocking, spawn_task, RUNTIME_MAX_BLOCKING_THREADS, RUNTIME_WORKER_THREADS,
    };

    #[test]
    fn shared_runtime_thread_limits_are_fixed() {
        assert_eq!(RUNTIME_WORKER_THREADS, 2);
        assert_eq!(RUNTIME_MAX_BLOCKING_THREADS, 8);
        assert!(RUNTIME_WORKER_THREADS > 0);
        assert!(RUNTIME_MAX_BLOCKING_THREADS >= RUNTIME_WORKER_THREADS);
    }

    #[test]
    fn shared_blocking_pool_can_drive_sync_futures() {
        let task = spawn_blocking(|| block(async { 7_u8 }));
        assert_eq!(block(task).unwrap(), 7);
    }

    #[test]
    fn async_task_panic_is_propagated() {
        let task = spawn_task(async { panic!("异步任务测试 panic") });
        assert!(block(task).unwrap_err().is_panic());
    }

    #[test]
    fn blocking_task_panic_is_propagated() {
        let task = spawn_blocking(|| panic!("阻塞任务测试 panic"));
        assert!(block(task).unwrap_err().is_panic());
    }
}
