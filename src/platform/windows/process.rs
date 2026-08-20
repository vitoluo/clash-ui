use std::mem::size_of;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::process::Child;
use std::ptr;

use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};

/// Windows 核心进程守卫，确保主程序结束时终止完整核心进程树。
pub struct CoreProcessGuard {
    job: OwnedHandle,
}

impl CoreProcessGuard {
    pub fn attach(child: &Child) -> Result<Self, String> {
        let raw_job = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
        if raw_job.is_null() {
            return Err(last_error("创建核心作业对象"));
        }
        let job = unsafe { OwnedHandle::from_raw_handle(raw_job) };

        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let configured = unsafe {
            SetInformationJobObject(
                job.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if configured == 0 {
            return Err(last_error("配置核心作业对象"));
        }

        let assigned =
            unsafe { AssignProcessToJobObject(job.as_raw_handle(), child.as_raw_handle()) };
        if assigned == 0 {
            return Err(last_error("关联核心进程到作业对象"));
        }

        Ok(Self { job })
    }

    pub fn terminate(&self) -> Result<(), String> {
        let terminated = unsafe { TerminateJobObject(self.job.as_raw_handle(), 1) };
        if terminated == 0 {
            return Err(last_error("终止核心进程树"));
        }
        Ok(())
    }
}

fn last_error(operation: &str) -> String {
    format!("{operation}失败：{}", std::io::Error::last_os_error())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use std::thread;
    use std::time::{Duration, Instant};

    #[test]
    fn closes_job_and_reaps_attached_process() {
        let mut child = Command::new("cmd")
            .args(["/C", "ping -n 30 127.0.0.1 >NUL"])
            .spawn()
            .expect("启动作业对象测试进程失败");
        let guard = CoreProcessGuard::attach(&child).expect("关联作业对象测试进程失败");

        drop(guard);
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if child.try_wait().expect("读取测试进程状态失败").is_some() {
                child.wait().expect("回收作业对象测试进程失败");
                return;
            }
            thread::sleep(Duration::from_millis(20));
        }

        let _ = child.kill();
        let _ = child.wait();
        panic!("关闭作业对象后测试进程未在限定时间内退出");
    }
}
