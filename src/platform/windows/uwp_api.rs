// Windows UWP 网络隔离 API 的最小 FFI 封装。

use std::ffi::c_void;
use std::ptr;
use std::slice;

pub(super) const ERROR_SUCCESS: u32 = 0;
pub(super) const ERROR_ACCESS_DENIED: u32 = 5;
pub(super) const HRESULT_ACCESS_DENIED: u32 = 0x8007_0005;
pub(super) const NETISO_FLAG_FORCE_COMPUTE_BINARIES: u32 = 1;
const FREE_APP_CONTAINERS_BOOL_SUCCESS: u32 = 1;

#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct SidAndAttributes {
    pub(super) sid: *mut c_void,
    pub(super) attributes: u32,
}

#[repr(C)]
#[allow(dead_code)]
pub(super) struct InetFirewallAcCapabilities {
    pub(super) count: u32,
    pub(super) capabilities: *mut c_void,
}

#[repr(C)]
#[allow(dead_code)]
pub(super) struct InetFirewallAcBinaries {
    pub(super) count: u32,
    pub(super) binaries: *mut c_void,
}

#[repr(C)]
#[allow(dead_code)]
pub(super) struct InetFirewallAppContainer {
    pub(super) app_container_sid: *mut c_void,
    pub(super) user_sid: *mut c_void,
    pub(super) app_container_name: *mut u16,
    pub(super) display_name: *mut u16,
    pub(super) description: *mut u16,
    pub(super) capabilities: InetFirewallAcCapabilities,
    pub(super) binaries: InetFirewallAcBinaries,
    pub(super) working_directory: *mut u16,
    pub(super) package_full_name: *mut u16,
}

type EnumAppContainersFn =
    unsafe extern "system" fn(u32, *mut u32, *mut *mut InetFirewallAppContainer) -> u32;
type FreeAppContainersFn = unsafe extern "system" fn(*mut InetFirewallAppContainer) -> u32;
type GetAppContainerConfigFn =
    unsafe extern "system" fn(*mut u32, *mut *mut SidAndAttributes) -> u32;
type SetAppContainerConfigFn = unsafe extern "system" fn(u32, *const SidAndAttributes) -> u32;
type LoadIndirectStringFn =
    unsafe extern "system" fn(*const u16, *mut u16, u32, *mut *mut c_void) -> i32;

#[link(name = "Kernel32")]
extern "system" {
    fn LoadLibraryW(name: *const u16) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
    fn FreeLibrary(module: *mut c_void) -> i32;
    fn GetProcessHeap() -> *mut c_void;
    fn HeapFree(heap: *mut c_void, flags: u32, memory: *mut c_void) -> i32;
}

#[link(name = "Advapi32")]
extern "system" {
    fn IsValidSid(sid: *const c_void) -> i32;
    fn GetLengthSid(sid: *const c_void) -> u32;
}

pub(super) struct FirewallApi {
    module: *mut c_void,
    enum_app_containers_fn: EnumAppContainersFn,
    free_app_containers_fn: FreeAppContainersFn,
    get_app_container_config_fn: GetAppContainerConfigFn,
    set_app_container_config_fn: SetAppContainerConfigFn,
}

impl FirewallApi {
    pub(super) fn load() -> Result<Self, String> {
        let library_name = "FirewallAPI.dll"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let module = unsafe {
            // 加载 Windows 提供的 FirewallAPI.dll，并在 FirewallApi Drop 中释放模块句柄。
            LoadLibraryW(library_name.as_ptr())
        };
        if module.is_null() {
            return Err("加载 FirewallAPI.dll 失败".to_string());
        }

        let result = (|| {
            macro_rules! load_proc {
                ($name:literal, $function_type:ty) => {{
                    let address = unsafe {
                        // 模块已由 LoadLibraryW 成功加载，导出名使用 Windows API 的 ASCII 名称。
                        GetProcAddress(module, concat!($name, "\0").as_ptr())
                    };
                    if address.is_null() {
                        return Err(format!(
                            "FirewallAPI.dll 缺少导出函数：{}",
                            $name
                        ));
                    }
                    unsafe {
                        // GetProcAddress 返回的地址已确认非空，目标类型与 Windows 导出签名一致。
                        std::mem::transmute::<*mut c_void, $function_type>(address)
                    }
                }};
            }

            Ok(Self {
                module,
                enum_app_containers_fn: load_proc!(
                    "NetworkIsolationEnumAppContainers",
                    EnumAppContainersFn
                ),
                free_app_containers_fn: load_proc!(
                    "NetworkIsolationFreeAppContainers",
                    FreeAppContainersFn
                ),
                get_app_container_config_fn: load_proc!(
                    "NetworkIsolationGetAppContainerConfig",
                    GetAppContainerConfigFn
                ),
                set_app_container_config_fn: load_proc!(
                    "NetworkIsolationSetAppContainerConfig",
                    SetAppContainerConfigFn
                ),
            })
        })();

        match result {
            Ok(api) => Ok(api),
            Err(error) => match unsafe { free_library(module) } {
                Ok(()) => Err(error),
                Err(cleanup_error) => Err(format!(
                    "{error}；同时释放 FirewallAPI.dll 失败：{cleanup_error}"
                )),
            },
        }
    }

    pub(super) fn enum_app_containers(
        &self,
        flags: u32,
        count: *mut u32,
        containers: *mut *mut InetFirewallAppContainer,
    ) -> u32 {
        unsafe {
            // 函数指针来自已加载 DLL，输出指针由对应的 Free API 释放。
            (self.enum_app_containers_fn)(flags, count, containers)
        }
    }

    pub(super) fn free_app_containers(&self, containers: *mut InetFirewallAppContainer) -> u32 {
        unsafe {
            // containers 必须来自同一 DLL 的枚举函数，满足 Windows API 的所有权要求。
            (self.free_app_containers_fn)(containers)
        }
    }

    pub(super) fn get_app_container_config(
        &self,
        count: *mut u32,
        sids: *mut *mut SidAndAttributes,
    ) -> u32 {
        unsafe {
            // 输出 SID 数组由 Windows API 分配，调用方在复制后按官方规则释放。
            (self.get_app_container_config_fn)(count, sids)
        }
    }

    pub(super) fn set_app_container_config(
        &self,
        count: u32,
        sids: *const SidAndAttributes,
    ) -> u32 {
        unsafe {
            // 输入缓冲区由调用方持有，并在 API 返回前保持有效。
            (self.set_app_container_config_fn)(count, sids)
        }
    }

    pub(super) fn close(mut self) -> Result<(), String> {
        if self.module.is_null() {
            return Ok(());
        }
        let module = self.module;
        self.module = ptr::null_mut();
        unsafe { free_library(module) }
    }
}

impl Drop for FirewallApi {
    fn drop(&mut self) {
        if !self.module.is_null() {
            let module = self.module;
            self.module = ptr::null_mut();
            if let Err(error) = unsafe { free_library(module) } {
                crate::log::error(format_args!("释放 FirewallAPI.dll 失败：{error}"));
            }
        }
    }
}

pub(super) fn format_network_isolation_error(api_name: &str, code: u32) -> String {
    let win32_code = if code & 0xffff_0000 == 0x8007_0000 {
        Some(code & 0xffff)
    } else if code <= u16::MAX as u32 {
        Some(code)
    } else {
        None
    };
    let explanation = match code {
        ERROR_ACCESS_DENIED | HRESULT_ACCESS_DENIED => "权限不足",
        87 | 0x8007_0057 => "参数无效",
        0x8000_4005 => "系统拒绝了低层配置",
        _ => "未知错误",
    };
    let system_message = win32_code
        .map(|value| std::io::Error::from_raw_os_error(value as i32).to_string())
        .unwrap_or_else(|| "无法解析系统错误文本".to_string());
    format!("{api_name} 失败，错误码 0x{code:08X}（{explanation}：{system_message}）")
}

pub(super) fn is_access_denied(code: u32) -> bool {
    matches!(code, ERROR_ACCESS_DENIED | HRESULT_ACCESS_DENIED)
}

pub(super) fn free_app_containers_succeeded(code: u32) -> bool {
    // 部分 Windows 版本按 BOOL 实现该函数，释放成功返回 TRUE(1)，而不是 ERROR_SUCCESS(0)。
    matches!(code, ERROR_SUCCESS | FREE_APP_CONTAINERS_BOOL_SUCCESS)
}

pub(super) fn read_utf16(value: *const u16) -> Option<String> {
    if value.is_null() {
        return None;
    }
    let text = unsafe {
        // Windows API 返回以 NUL 结尾且在本次调用期间有效的 UTF-16 字符串。
        let mut length = 0;
        while *value.add(length) != 0 {
            length += 1;
        }
        String::from_utf16_lossy(slice::from_raw_parts(value, length))
    };
    let text = text.trim().to_string();
    (!text.is_empty()).then_some(text)
}

fn read_utf16_buffer(value: &[u16]) -> Option<String> {
    let length = value.iter().position(|item| *item == 0)?;
    let text = String::from_utf16_lossy(&value[..length])
        .trim()
        .to_string();
    (!text.is_empty()).then_some(text)
}

pub(super) fn load_indirect_string(value: &str) -> Option<String> {
    if !value.starts_with("@{") {
        return None;
    }
    let source = value
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let mut output = vec![0u16; 512];
    let library_name = "Shlwapi.dll"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let module = unsafe {
        // 动态加载系统库，避免不同 Windows 工具链缺少 Shlwapi 导入库。
        LoadLibraryW(library_name.as_ptr())
    };
    if module.is_null() {
        return None;
    }
    let result = (|| {
        let address = unsafe {
            // SHLoadIndirectString 的导出名固定为 ASCII 字符串。
            GetProcAddress(module, c"SHLoadIndirectString".as_ptr().cast())
        };
        if address.is_null() {
            return None;
        }
        let load_string = unsafe {
            // 导出函数签名与 Windows SDK 定义一致，且地址已确认非空。
            std::mem::transmute::<*mut c_void, LoadIndirectStringFn>(address)
        };
        let result = unsafe {
            // SHLoadIndirectString 读取当前用户语言对应的 Resources.pri 文本。
            load_string(
                source.as_ptr(),
                output.as_mut_ptr(),
                output.len() as u32,
                ptr::null_mut(),
            )
        };
        (result >= 0).then(|| read_utf16_buffer(&output)).flatten()
    })();
    if let Err(error) = unsafe { free_library(module) } {
        crate::log::error(format_args!("释放 Shlwapi.dll 失败：{error}"));
    }
    result
}

pub(super) fn copy_sid(value: *const c_void) -> Result<Vec<u8>, String> {
    if value.is_null() {
        return Err("UWP SID 指针为空".to_string());
    }
    let (valid, length) = unsafe {
        // SID 指针由 Windows API 返回；先校验，再按 API 给出的长度复制。
        (IsValidSid(value), GetLengthSid(value))
    };
    if valid == 0 || length < 8 {
        return Err("UWP SID 无效".to_string());
    }
    Ok(unsafe {
        // IsValidSid 已确认缓冲区长度；复制后不再持有外部 SID 指针。
        slice::from_raw_parts(value as *const u8, length as usize).to_vec()
    })
}

pub(super) unsafe fn free_loopback_sid_config(
    count: u32,
    sids: *mut SidAndAttributes,
) -> Result<(), String> {
    if sids.is_null() {
        return Ok(());
    }
    // GetAppContainerConfig 的官方示例要求使用进程堆释放每个 SID 和外层数组。
    let heap = GetProcessHeap();
    if heap.is_null() {
        return Err("获取进程堆失败".to_string());
    }
    let mut errors = Vec::new();
    let items = slice::from_raw_parts_mut(sids, count as usize);
    for item in items {
        if !item.sid.is_null() && HeapFree(heap, 0, item.sid) == 0 {
            errors.push(format!(
                "释放 UWP SID 失败：{}",
                std::io::Error::last_os_error()
            ));
        }
    }
    if HeapFree(heap, 0, sids as *mut c_void) == 0 {
        errors.push(format!(
            "释放 UWP SID 数组失败：{}",
            std::io::Error::last_os_error()
        ));
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("；"))
    }
}

unsafe fn free_library(module: *mut c_void) -> Result<(), String> {
    if FreeLibrary(module) == 0 {
        Err(std::io::Error::last_os_error().to_string())
    } else {
        Ok(())
    }
}
