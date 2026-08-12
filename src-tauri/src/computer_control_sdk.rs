use std::{
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex},
    time::Duration,
};

use serde::Serialize;
use serde_json::{json, Value};

const ABI_MAJOR: u16 = 1;
const ABI_MINOR: u16 = 1;

#[derive(Debug, Clone, Serialize)]
pub struct CuaDriverSdkRuntimeInfo {
    pub abi_version: String,
    pub driver_version: Option<String>,
    pub embedded: bool,
    pub metadata: Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct CuaDriverSdkStatus {
    pub initialized: bool,
    pub resource_dir: Option<String>,
    pub library_path: Option<String>,
    pub abi_version: Option<String>,
    pub driver_version: Option<String>,
    pub embedded: Option<bool>,
    pub error: Option<String>,
}

pub struct CuaDriverSdk {
    resource_dir: Mutex<Option<PathBuf>>,
    runtime: Mutex<Option<Runtime>>,
    runtime_info: Mutex<Option<CuaDriverSdkRuntimeInfo>>,
    last_error: Mutex<Option<String>>,
}

impl CuaDriverSdk {
    pub fn new() -> Self {
        Self {
            resource_dir: Mutex::new(None),
            runtime: Mutex::new(None),
            runtime_info: Mutex::new(None),
            last_error: Mutex::new(None),
        }
    }

    pub fn set_resource_dir(&self, resource_dir: Option<PathBuf>) {
        if let Ok(mut current) = self.resource_dir.lock() {
            if self
                .runtime
                .lock()
                .map(|runtime| runtime.is_none())
                .unwrap_or(false)
            {
                *current = resource_dir;
            }
        }
    }

    pub fn status(&self) -> CuaDriverSdkStatus {
        let resource_dir = self
            .resource_dir
            .lock()
            .ok()
            .and_then(|value| value.clone());
        let library_path = resource_dir
            .as_deref()
            .and_then(resolve_library_path)
            .map(|path| path.to_string_lossy().into_owned());
        let runtime_info = self
            .runtime_info
            .lock()
            .ok()
            .and_then(|value| value.clone());
        let error = self.last_error.lock().ok().and_then(|value| value.clone());

        CuaDriverSdkStatus {
            initialized: runtime_info.is_some(),
            resource_dir: resource_dir.map(|path| path.to_string_lossy().into_owned()),
            library_path,
            abi_version: runtime_info.as_ref().map(|value| value.abi_version.clone()),
            driver_version: runtime_info
                .as_ref()
                .and_then(|value| value.driver_version.clone()),
            embedded: runtime_info.as_ref().map(|value| value.embedded),
            error,
        }
    }

    pub fn initialize(&self) -> Result<CuaDriverSdkRuntimeInfo, String> {
        {
            let runtime_info = self
                .runtime_info
                .lock()
                .map_err(|_| "CUA SDK runtime state is poisoned".to_string())?;
            if let Some(runtime_info) = runtime_info.clone() {
                return Ok(runtime_info);
            }
        }

        let result = (|| {
            let resource_dir = self
                .resource_dir
                .lock()
                .map_err(|_| "CUA SDK resource state is poisoned".to_string())?
                .clone()
                .ok_or_else(|| "Panes resource directory is not configured".to_string())?;
            let library_path = resolve_library_path(&resource_dir).ok_or_else(|| {
                format!(
                    "official CUA SDK library was not found under {}",
                    resource_dir.display()
                )
            })?;

            let runtime = Runtime::create(&library_path).map_err(|error| error.to_string())?;
            let runtime_info = runtime.info().map_err(|error| error.to_string())?;

            let mut runtime_slot = self
                .runtime
                .lock()
                .map_err(|_| "CUA SDK runtime state is poisoned".to_string())?;
            if runtime_slot.is_none() {
                *runtime_slot = Some(runtime);
                *self
                    .runtime_info
                    .lock()
                    .map_err(|_| "CUA SDK runtime info is poisoned".to_string())? =
                    Some(runtime_info.clone());
                self.set_error(None);
                Ok(runtime_info)
            } else {
                Err("CUA SDK runtime was initialized concurrently".to_string())
            }
        })();

        if let Err(error) = &result {
            self.set_error(Some(error.clone()));
        }
        result
    }

    pub fn list_tools(&self) -> Result<Value, String> {
        let mut runtime = self
            .runtime
            .lock()
            .map_err(|_| "CUA SDK runtime state is poisoned".to_string())?;
        runtime
            .as_mut()
            .ok_or_else(|| "CUA SDK runtime is not initialized".to_string())?
            .list_tools()
            .map_err(|error| self.remember_error(error))
    }

    pub fn get_screen_size(&self) -> Result<Value, String> {
        let mut runtime = self
            .runtime
            .lock()
            .map_err(|_| "CUA SDK runtime state is poisoned".to_string())?;
        runtime
            .as_mut()
            .ok_or_else(|| "CUA SDK runtime is not initialized".to_string())?
            .invoke("get_screen_size", &json!({}))
            .map_err(|error| self.remember_error(error))
    }

    pub fn invoke(&self, tool: &str, arguments: &Value) -> Result<Value, String> {
        let mut runtime = self
            .runtime
            .lock()
            .map_err(|_| "CUA SDK runtime state is poisoned".to_string())?;
        runtime
            .as_mut()
            .ok_or_else(|| "CUA SDK runtime is not initialized".to_string())?
            .invoke(tool, arguments)
            .map_err(|error| self.remember_error(error))
    }

    pub fn shutdown(&self) -> Result<(), String> {
        let runtime = self
            .runtime
            .lock()
            .map_err(|_| "CUA SDK runtime state is poisoned".to_string())?
            .take();
        *self
            .runtime_info
            .lock()
            .map_err(|_| "CUA SDK runtime info is poisoned".to_string())? = None;
        if let Some(mut runtime) = runtime {
            runtime
                .shutdown()
                .map_err(|error| self.remember_error(error))?;
        }
        Ok(())
    }

    fn set_error(&self, value: Option<String>) {
        if let Ok(mut error) = self.last_error.lock() {
            *error = value;
        }
    }

    fn remember_error(&self, error: RuntimeError) -> String {
        let message = error.to_string();
        self.set_error(Some(message.clone()));
        message
    }
}

impl Default for CuaDriverSdk {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for CuaDriverSdk {
    fn drop(&mut self) {
        let runtime = self.runtime.get_mut().ok().and_then(Option::take);
        if let Some(mut runtime) = runtime {
            let _ = runtime.shutdown();
        }
    }
}

fn resolve_library_path(resource_dir: &Path) -> Option<PathBuf> {
    [
        resource_dir
            .join("resources")
            .join("cua-driver")
            .join("windows-x86_64")
            .join("cua_driver_sdk.dll"),
        resource_dir
            .join("resources")
            .join("cua-driver")
            .join("cua_driver_sdk.dll"),
        resource_dir
            .join("cua-driver")
            .join("windows-x86_64")
            .join("cua_driver_sdk.dll"),
        resource_dir.join("cua-driver").join("cua_driver_sdk.dll"),
        resource_dir.join("cua_driver_sdk.dll"),
    ]
    .into_iter()
    .find(|path| path.is_file())
}

#[tauri::command]
pub fn get_computer_control_sdk_status(
    state: tauri::State<'_, Arc<CuaDriverSdk>>,
) -> CuaDriverSdkStatus {
    state.status()
}

#[tauri::command]
pub fn initialize_computer_control_sdk(
    state: tauri::State<'_, Arc<CuaDriverSdk>>,
) -> Result<CuaDriverSdkRuntimeInfo, String> {
    state.initialize()
}

#[tauri::command]
pub fn get_computer_control_sdk_tools(
    state: tauri::State<'_, Arc<CuaDriverSdk>>,
) -> Result<Value, String> {
    state.list_tools()
}

#[tauri::command]
pub fn get_computer_control_sdk_screen_size(
    state: tauri::State<'_, Arc<CuaDriverSdk>>,
) -> Result<Value, String> {
    state.get_screen_size()
}

#[tauri::command]
pub fn shutdown_computer_control_sdk(
    state: tauri::State<'_, Arc<CuaDriverSdk>>,
) -> Result<(), String> {
    state.shutdown()
}

#[derive(Debug)]
struct RuntimeError(String);

impl RuntimeError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl std::fmt::Display for RuntimeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for RuntimeError {}

#[cfg(not(target_os = "windows"))]
struct Runtime;

#[cfg(not(target_os = "windows"))]
impl Runtime {
    fn create(_library_path: &Path) -> Result<Self, RuntimeError> {
        Err(RuntimeError::new(
            "CUA SDK direct runtime is not implemented for this platform yet",
        ))
    }

    fn info(&self) -> Result<CuaDriverSdkRuntimeInfo, RuntimeError> {
        Err(RuntimeError::new("CUA SDK runtime is unavailable"))
    }

    fn list_tools(&mut self) -> Result<Value, RuntimeError> {
        Err(RuntimeError::new("CUA SDK runtime is unavailable"))
    }

    fn invoke(&mut self, _name: &str, _arguments: &Value) -> Result<Value, RuntimeError> {
        Err(RuntimeError::new("CUA SDK runtime is unavailable"))
    }

    fn shutdown(&mut self) -> Result<(), RuntimeError> {
        Ok(())
    }
}

#[cfg(target_os = "windows")]
mod windows_impl {
    use super::{
        Condvar, CuaDriverSdkRuntimeInfo, Duration, Mutex, Path, RuntimeError, ABI_MAJOR, ABI_MINOR,
    };
    use serde_json::Value;
    use std::{
        ffi::{c_char, c_void, OsStr},
        mem,
        os::windows::ffi::OsStrExt,
        ptr, slice,
    };

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct AbiVersion {
        struct_size: u32,
        major: u16,
        minor: u16,
        patch: u16,
        reserved: u16,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Buffer {
        data: *mut u8,
        len: usize,
        capacity: usize,
    }

    #[repr(C)]
    struct Driver;

    #[repr(C)]
    struct Operation;

    type Completion = unsafe extern "C" fn(*mut c_void, i32, Buffer, Buffer);
    type AbiVersionFn = unsafe extern "C" fn(*mut AbiVersion) -> i32;
    type AbiCompatibleFn = unsafe extern "C" fn(u16, u16) -> bool;
    type BufferFreeFn = unsafe extern "C" fn(*mut Buffer);
    type CreateFn = unsafe extern "C" fn(*const u8, usize, *mut *mut Driver, *mut Buffer) -> i32;
    type DestroyFn = unsafe extern "C" fn(*mut *mut Driver);
    type AvailableFn = unsafe extern "C" fn(*mut Driver, *mut bool, *mut Buffer) -> i32;
    type JsonFn = unsafe extern "C" fn(*mut Driver, *mut Buffer, *mut Buffer) -> i32;
    type InvokeFn = unsafe extern "C" fn(
        *mut Driver,
        *const u8,
        usize,
        *const u8,
        usize,
        Option<Completion>,
        *mut c_void,
        *mut *mut Operation,
        *mut Buffer,
    ) -> i32;
    type ShutdownFn = unsafe extern "C" fn(
        *mut Driver,
        Option<Completion>,
        *mut c_void,
        *mut *mut Operation,
        *mut Buffer,
    ) -> i32;
    type OperationReleaseFn = unsafe extern "C" fn(*mut *mut Operation);

    struct NativeApi {
        module: *mut c_void,
        abi_version: AbiVersionFn,
        abi_compatible: AbiCompatibleFn,
        buffer_free: BufferFreeFn,
        create: CreateFn,
        destroy: DestroyFn,
        available: AvailableFn,
        metadata: JsonFn,
        list_tools: JsonFn,
        invoke: InvokeFn,
        shutdown: ShutdownFn,
        operation_release: OperationReleaseFn,
    }

    unsafe impl Send for NativeApi {}
    unsafe impl Sync for NativeApi {}

    struct CompletionState {
        value: Mutex<Option<(i32, String, String)>>,
        wake: Condvar,
    }

    struct CompletionContext {
        api: *const NativeApi,
        state: *const CompletionState,
    }

    pub(super) struct Runtime {
        api: Option<NativeApi>,
        driver: *mut Driver,
    }

    unsafe impl Send for Runtime {}
    unsafe impl Sync for Runtime {}

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LoadLibraryW(name: *const u16) -> *mut c_void;
        fn GetProcAddress(module: *mut c_void, name: *const c_char) -> *mut c_void;
        fn FreeLibrary(module: *mut c_void) -> i32;
    }

    unsafe fn load_symbol<T>(module: *mut c_void, name: &'static [u8]) -> Result<T, RuntimeError> {
        let address = GetProcAddress(module, name.as_ptr() as *const c_char);
        if address.is_null() {
            return Err(RuntimeError::new(format!(
                "official CUA SDK is missing symbol {}",
                String::from_utf8_lossy(name)
            )));
        }
        Ok(mem::transmute_copy(&address))
    }

    fn load_api(path: &Path) -> Result<NativeApi, RuntimeError> {
        let wide = OsStr::new(path)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let module = unsafe { LoadLibraryW(wide.as_ptr()) };
        if module.is_null() {
            return Err(RuntimeError::new(format!(
                "failed to load official CUA SDK library {}",
                path.display()
            )));
        }

        let result = (|| unsafe {
            Ok(NativeApi {
                module,
                abi_version: load_symbol(module, b"cua_driver_abi_version_v1\0")?,
                abi_compatible: load_symbol(module, b"cua_driver_abi_is_compatible_v1\0")?,
                buffer_free: load_symbol(module, b"cua_driver_buffer_free_v1\0")?,
                create: load_symbol(module, b"cua_driver_create_v1\0")?,
                destroy: load_symbol(module, b"cua_driver_destroy_v1\0")?,
                available: load_symbol(module, b"cua_driver_is_available_v1\0")?,
                metadata: load_symbol(module, b"cua_driver_metadata_json_v1\0")?,
                list_tools: load_symbol(module, b"cua_driver_list_tools_json_v1\0")?,
                invoke: load_symbol(module, b"cua_driver_invoke_v1\0")?,
                shutdown: load_symbol(module, b"cua_driver_shutdown_v1\0")?,
                operation_release: load_symbol(module, b"cua_driver_operation_release_v1\0")?,
            })
        })();
        if result.is_err() {
            unsafe {
                let _ = FreeLibrary(module);
            }
        }
        result
    }

    unsafe fn read_buffer(api: &NativeApi, mut buffer: Buffer) -> String {
        let value = if buffer.data.is_null() || buffer.len == 0 {
            String::new()
        } else {
            String::from_utf8_lossy(slice::from_raw_parts(buffer.data, buffer.len)).into_owned()
        };
        (api.buffer_free)(&mut buffer);
        value
    }

    unsafe extern "C" fn completion_callback(
        context: *mut c_void,
        status: i32,
        result: Buffer,
        error: Buffer,
    ) {
        if context.is_null() {
            return;
        }
        let context = &*(context as *const CompletionContext);
        let api = &*context.api;
        let result = read_buffer(api, result);
        let error = read_buffer(api, error);
        let state = &*context.state;
        *state.value.lock().expect("CUA completion state poisoned") = Some((status, result, error));
        state.wake.notify_one();
    }

    fn wait_for_completion(state: &CompletionState) -> Result<(i32, String, String), RuntimeError> {
        let guard = state
            .value
            .lock()
            .map_err(|_| RuntimeError::new("CUA completion state poisoned"))?;
        let (guard, result) = state
            .wake
            .wait_timeout_while(guard, Duration::from_secs(30), |value| value.is_none())
            .map_err(|_| RuntimeError::new("CUA completion wait failed"))?;
        if result.timed_out() {
            return Err(RuntimeError::new(
                "CUA native operation timed out after 30 seconds",
            ));
        }
        guard
            .clone()
            .ok_or_else(|| RuntimeError::new("CUA operation completed without a result"))
    }

    unsafe fn call_json(
        api: &NativeApi,
        driver: *mut Driver,
        call: JsonFn,
    ) -> Result<Value, RuntimeError> {
        let mut output = Buffer {
            data: ptr::null_mut(),
            len: 0,
            capacity: 0,
        };
        let mut error = output;
        let status = call(driver, &mut output, &mut error);
        let output = read_buffer(api, output);
        let error = read_buffer(api, error);
        if status != 0 {
            return Err(RuntimeError::new(format!(
                "CUA JSON call failed with status {status}: {error}"
            )));
        }
        serde_json::from_str(&output).map_err(|parse_error| {
            RuntimeError::new(format!("CUA returned invalid JSON: {parse_error}"))
        })
    }

    unsafe fn call_async(
        api: &NativeApi,
        driver: *mut Driver,
        name: Option<&[u8]>,
        arguments: Option<&[u8]>,
        shutdown: bool,
    ) -> Result<(i32, String, String), RuntimeError> {
        let state = Box::new(CompletionState {
            value: Mutex::new(None),
            wake: Condvar::new(),
        });
        let context = Box::new(CompletionContext {
            api: api as *const NativeApi,
            state: &*state as *const CompletionState,
        });
        let mut operation = ptr::null_mut();
        let mut error = Buffer {
            data: ptr::null_mut(),
            len: 0,
            capacity: 0,
        };
        let status = if shutdown {
            (api.shutdown)(
                driver,
                Some(completion_callback),
                &*context as *const CompletionContext as *mut c_void,
                &mut operation,
                &mut error,
            )
        } else {
            let name = name.expect("CUA invoke name is required");
            let arguments = arguments.expect("CUA invoke arguments are required");
            (api.invoke)(
                driver,
                name.as_ptr(),
                name.len(),
                arguments.as_ptr(),
                arguments.len(),
                Some(completion_callback),
                &*context as *const CompletionContext as *mut c_void,
                &mut operation,
                &mut error,
            )
        };
        let immediate_error = read_buffer(api, error);
        if status != 0 {
            if !operation.is_null() {
                (api.operation_release)(&mut operation);
            }
            return Err(RuntimeError::new(format!(
                "CUA operation admission failed with status {status}: {immediate_error}"
            )));
        }

        let completed = wait_for_completion(&state);
        if completed.is_err() {
            // A late callback must not point to stack memory. Keep the callback
            // context alive if the native runtime ever violates its time bound.
            let _ = Box::into_raw(state);
            let _ = Box::into_raw(context);
        }
        if !operation.is_null() {
            (api.operation_release)(&mut operation);
        }
        completed
    }

    impl Runtime {
        pub(super) fn create(path: &Path) -> Result<Self, RuntimeError> {
            unsafe {
                let api = load_api(path)?;
                let mut version = AbiVersion {
                    struct_size: std::mem::size_of::<AbiVersion>() as u32,
                    major: 0,
                    minor: 0,
                    patch: 0,
                    reserved: 0,
                };
                let status = (api.abi_version)(&mut version);
                if status != 0 {
                    let _ = FreeLibrary(api.module);
                    return Err(RuntimeError::new(format!(
                        "CUA ABI version call failed with status {status}"
                    )));
                }
                if version.major != ABI_MAJOR || !(api.abi_compatible)(ABI_MAJOR, ABI_MINOR) {
                    let _ = FreeLibrary(api.module);
                    return Err(RuntimeError::new(format!(
                        "CUA ABI mismatch: runtime={}.{}.{} expected={}.{}",
                        version.major, version.minor, version.patch, ABI_MAJOR, ABI_MINOR
                    )));
                }

                let options = b"{}";
                let mut driver = ptr::null_mut();
                let mut error = Buffer {
                    data: ptr::null_mut(),
                    len: 0,
                    capacity: 0,
                };
                let status = (api.create)(options.as_ptr(), options.len(), &mut driver, &mut error);
                let error = read_buffer(&api, error);
                if status != 0 || driver.is_null() {
                    let _ = FreeLibrary(api.module);
                    return Err(RuntimeError::new(format!(
                        "CUA runtime creation failed with status {status}: {error}"
                    )));
                }
                let mut available = false;
                let mut availability_error = Buffer {
                    data: ptr::null_mut(),
                    len: 0,
                    capacity: 0,
                };
                let availability_status =
                    (api.available)(driver, &mut available, &mut availability_error);
                let availability_error = read_buffer(&api, availability_error);
                if availability_status != 0 || !available {
                    (api.destroy)(&mut driver);
                    let _ = FreeLibrary(api.module);
                    return Err(RuntimeError::new(format!(
                        "CUA runtime is unavailable with status {availability_status}: {availability_error}"
                    )));
                }
                Ok(Self {
                    api: Some(api),
                    driver,
                })
            }
        }

        pub(super) fn info(&self) -> Result<CuaDriverSdkRuntimeInfo, RuntimeError> {
            let api = self
                .api
                .as_ref()
                .ok_or_else(|| RuntimeError::new("CUA runtime library is unloaded"))?;
            unsafe {
                let mut version = AbiVersion {
                    struct_size: std::mem::size_of::<AbiVersion>() as u32,
                    major: 0,
                    minor: 0,
                    patch: 0,
                    reserved: 0,
                };
                let status = (api.abi_version)(&mut version);
                if status != 0 {
                    return Err(RuntimeError::new(format!(
                        "CUA ABI version call failed with status {status}"
                    )));
                }
                let metadata = call_json(api, self.driver, api.metadata)?;
                let driver_version = metadata
                    .get("driver_version")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                let embedded = metadata
                    .get("embedded")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                Ok(CuaDriverSdkRuntimeInfo {
                    abi_version: format!("{}.{}.{}", version.major, version.minor, version.patch),
                    driver_version,
                    embedded,
                    metadata,
                })
            }
        }

        pub(super) fn list_tools(&mut self) -> Result<Value, RuntimeError> {
            let api = self
                .api
                .as_ref()
                .ok_or_else(|| RuntimeError::new("CUA runtime library is unloaded"))?;
            unsafe { call_json(api, self.driver, api.list_tools) }
        }

        pub(super) fn invoke(
            &mut self,
            name: &str,
            arguments: &Value,
        ) -> Result<Value, RuntimeError> {
            let api = self
                .api
                .as_ref()
                .ok_or_else(|| RuntimeError::new("CUA runtime library is unloaded"))?;
            let name = name.as_bytes();
            let arguments = serde_json::to_vec(arguments).map_err(|error| {
                RuntimeError::new(format!("failed to encode CUA arguments: {error}"))
            })?;
            let (_, result, error) =
                unsafe { call_async(api, self.driver, Some(name), Some(&arguments), false)? };
            if !error.is_empty() {
                return Err(RuntimeError::new(format!(
                    "CUA tool {name:?} failed: {error}"
                )));
            }
            serde_json::from_str(&result).map_err(|parse_error| {
                RuntimeError::new(format!("CUA tool returned invalid JSON: {parse_error}"))
            })
        }

        pub(super) fn shutdown(&mut self) -> Result<(), RuntimeError> {
            let Some(api) = self.api.take() else {
                return Ok(());
            };
            unsafe {
                let result = call_async(&api, self.driver, None, None, true);
                (api.destroy)(&mut self.driver);
                let _ = FreeLibrary(api.module);
                result.map(|_| ())
            }
        }
    }

    impl Drop for Runtime {
        fn drop(&mut self) {
            let _ = self.shutdown();
        }
    }
}

#[cfg(target_os = "windows")]
use windows_impl::Runtime;

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::CuaDriverSdk;
    use std::{env, path::PathBuf};

    #[test]
    fn official_windows_release_runtime_smoke() {
        if env::var_os("PANES_CUA_SDK_SPIKE").is_none() {
            return;
        }
        let resource_dir = PathBuf::from("target/release");
        let sdk = CuaDriverSdk::new();
        sdk.set_resource_dir(Some(resource_dir));
        let info = sdk.initialize().expect("CUA runtime should initialize");
        assert_eq!(info.abi_version, "1.1.0");
        assert_eq!(info.driver_version.as_deref(), Some("0.19.3"));
        assert!(info.embedded);
        assert!(sdk
            .list_tools()
            .expect("tool inventory should load")
            .is_object());
        assert!(sdk
            .get_screen_size()
            .expect("screen size should be readable")
            .is_object());
        sdk.shutdown().expect("CUA runtime should shut down");
        assert!(!sdk.status().initialized);
    }

    #[test]
    fn tool_calls_require_explicit_startup_initialization() {
        let sdk = CuaDriverSdk::new();

        let error = sdk
            .get_screen_size()
            .expect_err("tool call must not initialize the CUA runtime on demand");

        assert!(error.contains("CUA SDK runtime is not initialized"));
        assert!(!sdk.status().initialized);
    }
}
