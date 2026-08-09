use std::{
    collections::{HashMap, HashSet},
    env,
    ffi::OsString,
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::TcpStream,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Arc,
    thread,
    time::Duration,
};

use anyhow::{anyhow, Context};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::{oneshot, Mutex, Notify, RwLock},
};
use uuid::Uuid;

use crate::{config::app_config::AppConfig, process_utils, runtime_env, state::AppState};

const MANAGED_SERVER_NAME: &str = "panes-computer-control";
const COMPUTER_CONTROL_PROXY_SUBCOMMAND: &str = "--panes-computer-control-mcp";
const BROKER_FILE_ENV: &str = "PANES_COMPUTER_CONTROL_BROKER_FILE";
const DRIVER_PATH_ENV: &str = "PANES_CUA_DRIVER_PATH";
const AGENT_ID_ENV: &str = "PANES_COMPUTER_CONTROL_AGENT";
const INTERNAL_BROKER_WATCH_TOOL: &str = "__panes_watch_broker";
const APPROVAL_TIMEOUT: Duration = Duration::from_secs(300);
const ALLOWED_TOOLS: &[&str] = &[
    "start_session",
    "end_session",
    "launch_app",
    "list_apps",
    "list_windows",
    "bring_to_front",
    "get_window_state",
    "get_accessibility_tree",
    "verify_state",
    "get_screen_size",
    "get_cursor_position",
    "click",
    "double_click",
    "right_click",
    "drag",
    "type_text",
    "press_key",
    "hotkey",
    "set_value",
    "invoke_menu",
    "scroll",
    "move_cursor",
    "zoom",
    "clipboard_read",
    "clipboard_write",
    "check_permissions",
    "health_report",
    "get_session_state",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ComputerControlBrokerConfig {
    endpoint: String,
    token: String,
    host_pid: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ComputerControlBrokerRequest {
    token: String,
    session_id: String,
    agent: String,
    tool: String,
    #[serde(default)]
    arguments: Value,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ComputerControlBrokerResponse {
    allowed: bool,
    message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComputerControlApprovalRequestDto {
    request_id: String,
    agent: String,
    tool: String,
    application: String,
}

struct PendingComputerControlApproval {
    session_id: String,
    resource_key: String,
    response: oneshot::Sender<bool>,
}

#[derive(Default)]
pub struct ComputerControlApprovalManager {
    pending: Mutex<HashMap<String, PendingComputerControlApproval>>,
    granted_resources: RwLock<HashMap<String, HashSet<String>>>,
    proxy_shutdown: Notify,
}

impl ComputerControlApprovalManager {
    async fn authorize(
        &self,
        app: &AppHandle,
        request: &ComputerControlBrokerRequest,
        resource_key: String,
        application: String,
    ) -> bool {
        if self
            .granted_resources
            .read()
            .await
            .get(&request.session_id)
            .map(|resources| resources.contains(&resource_key))
            .unwrap_or(false)
        {
            return true;
        }

        let request_id = Uuid::new_v4().to_string();
        let (response_tx, response_rx) = oneshot::channel();
        self.pending.lock().await.insert(
            request_id.clone(),
            PendingComputerControlApproval {
                session_id: request.session_id.clone(),
                resource_key,
                response: response_tx,
            },
        );

        if let Some(window) = app.get_webview_window("main") {
            let _ = window.show();
            let _ = window.unminimize();
            let _ = window.set_focus();
        }
        if app
            .emit(
                "computer-control-approval-requested",
                ComputerControlApprovalRequestDto {
                    request_id: request_id.clone(),
                    agent: request.agent.clone(),
                    tool: request.tool.clone(),
                    application,
                },
            )
            .is_err()
        {
            self.pending.lock().await.remove(&request_id);
            return false;
        }

        match tokio::time::timeout(APPROVAL_TIMEOUT, response_rx).await {
            Ok(Ok(allowed)) => allowed,
            Ok(Err(_)) | Err(_) => {
                self.pending.lock().await.remove(&request_id);
                false
            }
        }
    }

    async fn respond(&self, request_id: &str, allowed: bool) -> Result<(), String> {
        let pending = self
            .pending
            .lock()
            .await
            .remove(request_id)
            .ok_or_else(|| "computer control approval request was not found".to_string())?;
        if allowed {
            self.granted_resources
                .write()
                .await
                .entry(pending.session_id)
                .or_default()
                .insert(pending.resource_key);
        }
        pending
            .response
            .send(allowed)
            .map_err(|_| "computer control approval request is no longer active".to_string())
    }

    pub async fn revoke_all(&self) {
        self.granted_resources.write().await.clear();
        let pending = std::mem::take(&mut *self.pending.lock().await);
        for approval in pending.into_values() {
            let _ = approval.response.send(false);
        }
        self.proxy_shutdown.notify_waiters();
    }

    async fn revoke_session(&self, session_id: &str) {
        self.granted_resources.write().await.remove(session_id);
        let mut pending = self.pending.lock().await;
        let request_ids = pending
            .iter()
            .filter_map(|(request_id, approval)| {
                (approval.session_id == session_id).then(|| request_id.clone())
            })
            .collect::<Vec<_>>();
        for request_id in request_ids {
            if let Some(approval) = pending.remove(&request_id) {
                let _ = approval.response.send(false);
            }
        }
    }
}

#[cfg(target_os = "windows")]
fn process_executable_path(pid: u32) -> Option<String> {
    use windows::{
        core::PWSTR,
        Win32::{
            Foundation::CloseHandle,
            System::Threading::{
                OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
                PROCESS_QUERY_LIMITED_INFORMATION,
            },
        },
    };

    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let mut buffer = vec![0_u16; 32_768];
    let mut length = buffer.len() as u32;
    let result = unsafe {
        QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut length,
        )
    };
    let _ = unsafe { CloseHandle(process) };
    result
        .ok()
        .map(|_| String::from_utf16_lossy(&buffer[..length as usize]))
}

#[cfg(not(target_os = "windows"))]
fn process_executable_path(_pid: u32) -> Option<String> {
    None
}

fn application_resource(raw: &str) -> (String, String) {
    let trimmed = raw.trim();
    let display = PathBuf::from(trimmed)
        .canonicalize()
        .ok()
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_else(|| trimmed.to_string());
    (format!("application:{}", display.to_lowercase()), display)
}

fn requested_resource(tool: &str, arguments: &Value) -> Result<(String, String), String> {
    let object = arguments.as_object();
    if object
        .and_then(|value| value.get("scope"))
        .and_then(Value::as_str)
        .map(|scope| scope.eq_ignore_ascii_case("desktop"))
        .unwrap_or(false)
    {
        return Err("Panes does not allow whole-desktop computer control".to_string());
    }
    if object
        .and_then(|value| value.get("capture_scope"))
        .and_then(Value::as_str)
        .map(|scope| scope.eq_ignore_ascii_case("desktop"))
        .unwrap_or(false)
    {
        return Err("Panes does not allow whole-desktop computer control".to_string());
    }

    for key in ["launch_path", "path", "aumid", "bundle_id", "name"] {
        if let Some(application) = object
            .and_then(|value| value.get(key))
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
        {
            return Ok(application_resource(application));
        }
    }

    if let Some(pid) = object
        .and_then(|value| value.get("pid"))
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
    {
        if let Some(path) = process_executable_path(pid) {
            return Ok(application_resource(&path));
        }
        return Ok((format!("pid:{pid}"), format!("PID {pid}")));
    }

    if tool == "launch_app"
        && object
            .and_then(|value| value.get("urls"))
            .and_then(Value::as_array)
            .map(|urls| !urls.is_empty())
            .unwrap_or(false)
    {
        return Ok((
            "application:default-browser".to_string(),
            "Windows 默认浏览器".to_string(),
        ));
    }

    match tool {
        "list_apps" | "list_windows" => Ok((
            "observation:applications".to_string(),
            "Windows 应用和窗口".to_string(),
        )),
        "clipboard_read" | "clipboard_write" => Ok((
            "resource:clipboard".to_string(),
            "Windows 剪贴板".to_string(),
        )),
        "start_session"
        | "end_session"
        | "check_permissions"
        | "health_report"
        | "get_session_state"
        | "get_screen_size"
        | "get_cursor_position"
        | "move_cursor" => Ok((
            "metadata:computer-control".to_string(),
            "电脑操作运行状态".to_string(),
        )),
        _ => Err(format!(
            "computer control tool `{tool}` did not identify a target application"
        )),
    }
}

fn targets_panes(application: &str) -> bool {
    let Some(current_path) = env::current_exe()
        .ok()
        .and_then(|path| path.canonicalize().ok())
    else {
        return false;
    };
    let candidate = Path::new(application);
    application.eq_ignore_ascii_case(&current_path.to_string_lossy())
        || candidate
            .file_name()
            .zip(current_path.file_name())
            .map(|(left, right)| left.eq_ignore_ascii_case(right))
            .unwrap_or(false)
        || candidate
            .file_stem()
            .zip(current_path.file_stem())
            .map(|(left, right)| left.eq_ignore_ascii_case(right))
            .unwrap_or(false)
}

fn filter_tool_list_response(line: &str) -> String {
    let Ok(mut value) = serde_json::from_str::<Value>(line) else {
        return line.to_string();
    };
    let Some(tools) = value
        .get_mut("result")
        .and_then(|result| result.get_mut("tools"))
        .and_then(Value::as_array_mut)
    else {
        return line.to_string();
    };
    tools.retain(|tool| {
        tool.get("name")
            .and_then(Value::as_str)
            .map(|name| ALLOWED_TOOLS.contains(&name))
            .unwrap_or(false)
    });
    serde_json::to_string(&value).unwrap_or_else(|_| line.to_string())
}

pub async fn start_approval_broker(
    app: AppHandle,
    approvals: Arc<ComputerControlApprovalManager>,
) -> anyhow::Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let config = ComputerControlBrokerConfig {
        endpoint: listener.local_addr()?.to_string(),
        token: Uuid::new_v4().to_string(),
        host_pid: std::process::id(),
    };
    let broker_path = runtime_env::app_data_dir()
        .join("computer-control")
        .join("broker.json");
    write_text_atomic(
        &broker_path,
        &format!("{}\n", serde_json::to_string_pretty(&config)?),
    )?;

    tauri::async_runtime::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let app = app.clone();
            let approvals = approvals.clone();
            let expected_token = config.token.clone();
            tauri::async_runtime::spawn(async move {
                let (reader, mut writer) = stream.into_split();
                let mut lines = tokio::io::BufReader::new(reader).lines();
                let mut keep_open_for_broker_lifetime = false;
                let response = match lines.next_line().await {
                    Ok(Some(line)) => {
                        match serde_json::from_str::<ComputerControlBrokerRequest>(&line) {
                            Ok(request) if request.token == expected_token => {
                                let enabled = AppConfig::load_or_create()
                                    .map(|config| config.computer_control.enabled)
                                    .unwrap_or(false);
                                if request.tool == INTERNAL_BROKER_WATCH_TOOL {
                                    keep_open_for_broker_lifetime = true;
                                    ComputerControlBrokerResponse {
                                        allowed: true,
                                        message: "Panes computer control broker is active"
                                            .to_string(),
                                    }
                                } else if !enabled {
                                    ComputerControlBrokerResponse {
                                        allowed: false,
                                        message: "Panes computer control is disabled".to_string(),
                                    }
                                } else if !ALLOWED_TOOLS.contains(&request.tool.as_str()) {
                                    ComputerControlBrokerResponse {
                                        allowed: false,
                                        message: format!(
                                            "Panes does not expose the computer control tool `{}`",
                                            request.tool
                                        ),
                                    }
                                } else {
                                    match requested_resource(&request.tool, &request.arguments) {
                                    Ok((resource_key, _application))
                                        if resource_key == "metadata:computer-control" =>
                                    {
                                        if matches!(request.tool.as_str(), "start_session" | "end_session") {
                                            approvals.revoke_session(&request.session_id).await;
                                        }
                                        ComputerControlBrokerResponse {
                                            allowed: true,
                                            message: "Computer control metadata access was allowed"
                                                .to_string(),
                                        }
                                    }
                                    Ok((_resource_key, application))
                                        if targets_panes(&application) =>
                                    {
                                        ComputerControlBrokerResponse {
                                            allowed: false,
                                            message: "Panes cannot authorize computer control of its own window"
                                                .to_string(),
                                        }
                                    }
                                    Ok((resource_key, application)) => {
                                        let allowed = approvals
                                            .authorize(
                                                &app,
                                                &request,
                                                resource_key,
                                                application,
                                            )
                                            .await;
                                        ComputerControlBrokerResponse {
                                            allowed,
                                            message: if allowed {
                                                "Computer control was authorized by the user"
                                                    .to_string()
                                            } else {
                                                "Computer control was denied by the user".to_string()
                                            },
                                        }
                                    }
                                    Err(message) => ComputerControlBrokerResponse {
                                        allowed: false,
                                        message,
                                    },
                                }
                                }
                            }
                            Ok(_) => ComputerControlBrokerResponse {
                                allowed: false,
                                message: "Computer control broker authentication failed"
                                    .to_string(),
                            },
                            Err(error) => ComputerControlBrokerResponse {
                                allowed: false,
                                message: format!("Invalid computer control request: {error}"),
                            },
                        }
                    }
                    Ok(None) => ComputerControlBrokerResponse {
                        allowed: false,
                        message: "Computer control request was empty".to_string(),
                    },
                    Err(error) => ComputerControlBrokerResponse {
                        allowed: false,
                        message: format!("Failed to read computer control request: {error}"),
                    },
                };
                if let Ok(raw) = serde_json::to_string(&response) {
                    let _ = writer.write_all(format!("{raw}\n").as_bytes()).await;
                }
                if keep_open_for_broker_lifetime {
                    approvals.proxy_shutdown.notified().await;
                }
            });
        }
    });
    Ok(())
}

#[tauri::command]
pub async fn respond_computer_control_approval(
    state: State<'_, AppState>,
    request_id: String,
    allowed: bool,
) -> Result<(), String> {
    state
        .computer_control_approvals
        .respond(&request_id, allowed)
        .await
}

fn request_broker_authorization(
    session_id: &str,
    agent: &str,
    tool: &str,
    arguments: Value,
) -> ComputerControlBrokerResponse {
    let failure = |message: String| ComputerControlBrokerResponse {
        allowed: false,
        message,
    };
    let Some(broker_path) = env::var_os(BROKER_FILE_ENV).map(PathBuf::from) else {
        return failure("Panes computer control broker was not configured".to_string());
    };
    let config = match fs::read_to_string(&broker_path)
        .ok()
        .and_then(|raw| serde_json::from_str::<ComputerControlBrokerConfig>(&raw).ok())
    {
        Some(config) => config,
        None => return failure("Panes computer control broker is unavailable".to_string()),
    };
    let mut stream = match TcpStream::connect(&config.endpoint) {
        Ok(stream) => stream,
        Err(error) => {
            return failure(format!(
                "Panes computer control broker is unavailable: {error}"
            ))
        }
    };
    let _ = stream.set_read_timeout(Some(APPROVAL_TIMEOUT + Duration::from_secs(5)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
    let request = json!({
        "token": config.token,
        "sessionId": session_id,
        "agent": agent,
        "tool": tool,
        "arguments": arguments,
    });
    if writeln!(stream, "{request}").is_err() {
        return failure("Failed to send the computer control approval request".to_string());
    }
    let mut response = String::new();
    if BufReader::new(stream).read_line(&mut response).is_err() {
        return failure("Failed to read the computer control approval response".to_string());
    }
    serde_json::from_str::<ComputerControlBrokerResponse>(&response).unwrap_or_else(|error| {
        failure(format!(
            "Invalid computer control approval response: {error}"
        ))
    })
}

pub fn maybe_handle_cli_subcommand() -> anyhow::Result<bool> {
    if env::args().nth(1).as_deref() != Some(COMPUTER_CONTROL_PROXY_SUBCOMMAND) {
        return Ok(false);
    }

    let driver_path = env::var_os(DRIVER_PATH_ENV)
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("Cua Driver path was not configured"))?;
    let agent = env::var(AGENT_ID_ENV).unwrap_or_else(|_| "agent".to_string());
    let proxy_session_id = Uuid::new_v4().to_string();
    let mut active_session_id = proxy_session_id.clone();

    // The MCP process is owned by Codex/Claude/OpenCode, not by the Panes process.
    // Keep a dedicated broker connection open so the proxy can detect Panes exit even
    // when the agent host keeps its MCP stdin pipe open indefinitely.
    let broker_path = env::var_os(BROKER_FILE_ENV)
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("Panes computer control broker was not configured"))?;
    let broker_config = fs::read_to_string(&broker_path)
        .with_context(|| format!("failed to read {}", broker_path.display()))
        .and_then(|raw| {
            serde_json::from_str::<ComputerControlBrokerConfig>(&raw)
                .context("failed to parse Panes computer control broker configuration")
        })?;
    let mut broker_lifetime = TcpStream::connect(&broker_config.endpoint)
        .context("Panes computer control broker is unavailable")?;
    let broker_host_pid = broker_config.host_pid;
    let broker_host_executable = env::current_exe()
        .ok()
        .and_then(|path| path.canonicalize().ok());
    broker_lifetime.set_read_timeout(Some(Duration::from_secs(5)))?;
    broker_lifetime.set_write_timeout(Some(Duration::from_secs(5)))?;
    writeln!(
        broker_lifetime,
        "{}",
        json!({
            "token": broker_config.token,
            "sessionId": proxy_session_id.clone(),
            "agent": agent.clone(),
            "tool": INTERNAL_BROKER_WATCH_TOOL,
            "arguments": {}
        })
    )?;
    broker_lifetime.flush()?;
    let mut broker_acknowledgement = String::new();
    BufReader::new(&mut broker_lifetime).read_line(&mut broker_acknowledgement)?;
    let broker_acknowledgement =
        serde_json::from_str::<ComputerControlBrokerResponse>(&broker_acknowledgement)
            .context("invalid Panes computer control broker acknowledgement")?;
    if !broker_acknowledgement.allowed {
        return Err(anyhow!(broker_acknowledgement.message));
    }
    broker_lifetime.set_read_timeout(Some(Duration::from_millis(500)))?;

    let mut command = Command::new(&driver_path);
    command
        .arg("mcp")
        .env("CUA_DRIVER_PERMISSION_MODE", "standard")
        .env("CUA_DRIVER_RS_TELEMETRY_ENABLED", "false")
        .env("CUA_DRIVER_RS_UPDATE_CHECK", "false")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    process_utils::configure_std_command(&mut command);
    let mut child = command
        .spawn()
        .with_context(|| format!("failed to launch {}", driver_path.display()))?;
    let mut child_stdin = child
        .stdin
        .take()
        .context("Cua Driver stdin was unavailable")?;
    let child_stdout = child
        .stdout
        .take()
        .context("Cua Driver stdout was unavailable")?;
    let child_stderr = child
        .stderr
        .take()
        .context("Cua Driver stderr was unavailable")?;
    let child_pid = child.id();

    thread::spawn(move || {
        let mut buffer = [0_u8; 1];
        loop {
            match broker_lifetime.read(&mut buffer) {
                Ok(_) => break,
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                Err(_) => break,
            }
            let host_is_alive = process_executable_path(broker_host_pid)
                .and_then(|path| PathBuf::from(path).canonicalize().ok())
                .zip(broker_host_executable.as_ref())
                .map(|(actual, expected)| actual == *expected)
                .unwrap_or(false);
            if !host_is_alive {
                break;
            }
        }
        #[cfg(target_os = "windows")]
        unsafe {
            use windows::Win32::{
                Foundation::CloseHandle,
                System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE},
            };
            if let Ok(handle) = OpenProcess(PROCESS_TERMINATE, false, child_pid) {
                let _ = TerminateProcess(handle, 0);
                let _ = CloseHandle(handle);
            }
        }
        #[cfg(not(target_os = "windows"))]
        let _ = child_pid;
        std::process::exit(0);
    });

    let stdout_thread = thread::spawn(move || {
        let reader = BufReader::new(child_stdout);
        let stdout = std::io::stdout();
        for line in reader.lines().map_while(Result::ok) {
            let mut writer = stdout.lock();
            let _ = writeln!(writer, "{}", filter_tool_list_response(&line));
            let _ = writer.flush();
        }
    });
    let stderr_thread = thread::spawn(move || {
        let mut reader = BufReader::new(child_stderr);
        let mut stderr = std::io::stderr().lock();
        let _ = std::io::copy(&mut reader, &mut stderr);
    });

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = line?;
        let parsed = serde_json::from_str::<Value>(&line).ok();
        let tool_call = parsed.as_ref().and_then(|value| {
            (value.get("method").and_then(Value::as_str) == Some("tools/call")).then(|| {
                (
                    value.get("id").cloned(),
                    value
                        .pointer("/params/name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    value
                        .pointer("/params/arguments")
                        .cloned()
                        .unwrap_or_else(|| json!({})),
                )
            })
        });

        if let Some((request_id, tool, arguments)) = tool_call {
            if let Some(cua_session_id) = arguments
                .get("session")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
            {
                active_session_id = format!("{proxy_session_id}:{cua_session_id}");
            }
            let response = if ALLOWED_TOOLS.contains(&tool.as_str()) {
                request_broker_authorization(&active_session_id, &agent, &tool, arguments)
            } else {
                ComputerControlBrokerResponse {
                    allowed: false,
                    message: format!("Panes does not expose the computer control tool `{tool}`"),
                }
            };
            if !response.allowed {
                if let Some(request_id) = request_id {
                    let mut direct_output = stdout.lock();
                    let denial = json!({
                        "jsonrpc": "2.0",
                        "id": request_id,
                        "result": {
                            "content": [{ "type": "text", "text": response.message }],
                            "isError": true
                        }
                    });
                    writeln!(direct_output, "{denial}")?;
                    direct_output.flush()?;
                }
                continue;
            }
            if tool == "end_session" {
                active_session_id = proxy_session_id.clone();
            }
        }

        writeln!(child_stdin, "{line}")?;
        child_stdin.flush()?;
    }

    drop(child_stdin);
    let _ = child.wait();
    let _ = stdout_thread.join();
    let _ = stderr_thread.join();
    Ok(true)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComputerControlDriverStatusDto {
    installed: bool,
    path: Option<String>,
    version: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComputerControlAgentStatusDto {
    id: String,
    name: String,
    installed: bool,
    configured: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComputerControlStatusDto {
    supported: bool,
    enabled: bool,
    allowed_applications: Vec<String>,
    driver: ComputerControlDriverStatusDto,
    agents: Vec<ComputerControlAgentStatusDto>,
    warnings: Vec<String>,
}

fn run_command(executable: &Path, args: &[OsString]) -> anyhow::Result<String> {
    let mut command = Command::new(executable);
    command.args(args);
    // Panes 是 Windows GUI 程序；状态检测和配置命令必须隐藏控制台窗口，
    // 否则每次进入“电脑操作”设置页都会闪现黑色命令行窗口。
    process_utils::configure_std_command(&mut command);
    if let Some(path) = runtime_env::augmented_path() {
        command.env("PATH", path);
    }
    let output = command
        .output()
        .with_context(|| format!("failed to launch {}", executable.display()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        return Err(anyhow!(if stderr.is_empty() { stdout } else { stderr }));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn write_text_atomic(path: &Path, content: &str) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("invalid configuration path: {}", path.display()))?;
    fs::create_dir_all(parent)?;
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("config");
    let temp_path = path.with_extension(format!("{extension}.panes.tmp"));
    let backup_path = path.with_extension(format!("{extension}.panes.bak"));
    fs::write(&temp_path, content)?;

    if path.exists() {
        if backup_path.exists() {
            fs::remove_file(&backup_path)?;
        }
        fs::rename(path, &backup_path)?;
        if let Err(error) = fs::rename(&temp_path, path) {
            let _ = fs::rename(&backup_path, path);
            return Err(error.into());
        }
        fs::remove_file(&backup_path)?;
    } else {
        fs::rename(&temp_path, path)?;
    }
    Ok(())
}

fn opencode_config_path() -> Option<PathBuf> {
    if let Some(path) = env::var_os("OPENCODE_CONFIG").map(PathBuf::from) {
        return Some(path);
    }
    if let Some(directory) = env::var_os("OPENCODE_CONFIG_DIR").map(PathBuf::from) {
        return Some(directory.join("opencode.json"));
    }
    runtime_env::home_dir().map(|home| home.join(".config").join("opencode").join("opencode.json"))
}

fn collect_status(warnings: Vec<String>) -> anyhow::Result<ComputerControlStatusDto> {
    let config = AppConfig::load_or_create()?;
    let driver_path = runtime_env::resolve_executable("cua-driver");
    let driver_version = driver_path.as_deref().and_then(|path| {
        run_command(path, &[OsString::from("--version")])
            .ok()
            .and_then(|output| output.split_whitespace().last().map(str::to_string))
    });

    let home = runtime_env::home_dir();
    let codex_path = runtime_env::resolve_executable("codex");
    let codex_configured = home
        .as_ref()
        .map(|directory| directory.join(".codex").join("config.toml"))
        .and_then(|path| fs::read_to_string(path).ok())
        .and_then(|raw| toml::from_str::<toml::Value>(&raw).ok())
        .and_then(|value| {
            value
                .get("mcp_servers")
                .and_then(|servers| servers.get(MANAGED_SERVER_NAME))
                .cloned()
        })
        .is_some();

    let claude_path = runtime_env::resolve_executable("claude");
    let claude_configured =
        config.computer_control.enabled && driver_path.is_some() && claude_path.is_some();

    let opencode_path = runtime_env::resolve_executable("opencode");
    let opencode_configured = opencode_config_path()
        .and_then(|path| fs::read_to_string(path).ok())
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .and_then(|value| {
            value
                .get("mcp")
                .and_then(|servers| servers.get(MANAGED_SERVER_NAME))
                .cloned()
        })
        .is_some();

    Ok(ComputerControlStatusDto {
        supported: cfg!(target_os = "windows"),
        enabled: config.computer_control.enabled,
        allowed_applications: config.computer_control.allowed_applications,
        driver: ComputerControlDriverStatusDto {
            installed: driver_path.is_some(),
            path: driver_path.map(|path| path.to_string_lossy().to_string()),
            version: driver_version,
        },
        agents: vec![
            ComputerControlAgentStatusDto {
                id: "codex".to_string(),
                name: "Codex".to_string(),
                installed: codex_path.is_some(),
                configured: codex_configured,
            },
            ComputerControlAgentStatusDto {
                id: "claude".to_string(),
                name: "Claude Code".to_string(),
                installed: claude_path.is_some(),
                configured: claude_configured,
            },
            ComputerControlAgentStatusDto {
                id: "opencode".to_string(),
                name: "OpenCode".to_string(),
                installed: opencode_path.is_some(),
                configured: opencode_configured,
            },
        ],
        warnings,
    })
}

#[tauri::command]
pub async fn get_computer_control_status() -> Result<ComputerControlStatusDto, String> {
    tokio::task::spawn_blocking(|| collect_status(Vec::new()).map_err(|error| error.to_string()))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn set_computer_control(
    state: State<'_, AppState>,
    enabled: bool,
    allowed_applications: Vec<String>,
) -> Result<ComputerControlStatusDto, String> {
    let config_write_lock = state.config_write_lock.clone();
    let _guard = config_write_lock.lock_owned().await;

    let approvals = state.computer_control_approvals.clone();
    let result = tokio::task::spawn_blocking(move || -> Result<ComputerControlStatusDto, String> {
        if !cfg!(target_os = "windows") {
            return Err("computer control is currently supported on Windows only".to_string());
        }

        /*
        旧实现要求用户在开启功能前预先选择已经存在的 .exe，并据此生成 Cua Driver
        bounded manifest。这个模型无法覆盖“应用尚未开发完成、AI 首次运行后才出现 exe”
        的场景，因此保留代码作为历史参考，不再执行。现在由 Panes MCP 代理在每次真实
        工具调用时识别目标应用并发起授权。
        let mut canonical_applications = Vec::new();
        let panes_executable = env::current_exe()
            .ok()
            .and_then(|path| path.canonicalize().ok())
            .map(|path| path.to_string_lossy().to_string());
        for raw_path in allowed_applications {
            let path = PathBuf::from(raw_path.trim());
            if !path.is_absolute() || !path.is_file() {
                return Err(format!("application does not exist: {}", path.display()));
            }
            if path
                .extension()
                .and_then(|extension| extension.to_str())
                .map(|extension| !extension.eq_ignore_ascii_case("exe"))
                .unwrap_or(true)
            {
                return Err(format!("application must be an .exe file: {}", path.display()));
            }
            let canonical = path
                .canonicalize()
                .map_err(|error| format!("failed to resolve {}: {error}", path.display()))?
                .to_string_lossy()
                .to_string();
            if panes_executable
                .as_ref()
                .map(|current| current.eq_ignore_ascii_case(&canonical))
                .unwrap_or(false)
            {
                return Err("Panes cannot be added to its own computer control allowlist".to_string());
            }
            if !canonical_applications
                .iter()
                .any(|existing: &String| existing.eq_ignore_ascii_case(&canonical))
            {
                canonical_applications.push(canonical);
            }
        }

        if enabled && canonical_applications.is_empty() {
            return Err("select at least one application before enabling computer control".to_string());
        }

        let driver_path = runtime_env::resolve_executable("cua-driver");
        if enabled && driver_path.is_none() {
            return Err("Cua Driver is not installed or cannot be found".to_string());
        }

        let policy_path = runtime_env::app_data_dir()
            .join("computer-control")
            .join("session-policy.yaml");
        if enabled {
            let mut policy = String::from(
                "version: 2\nmode: bounded\nexpires_after: 8h\nidle_timeout: 30m\n\nallow:\n  tools:\n",
            );
            for tool in [
                "start_session",
                "end_session",
                "launch_app",
                "list_apps",
                "list_windows",
                "bring_to_front",
                "get_window_state",
                "get_accessibility_tree",
                "verify_state",
                "get_screen_size",
                "get_cursor_position",
                "click",
                "double_click",
                "right_click",
                "drag",
                "type_text",
                "press_key",
                "hotkey",
                "set_value",
                "invoke_menu",
                "scroll",
                "move_cursor",
                "zoom",
                "clipboard_read",
                "clipboard_write",
                "check_permissions",
                "health_report",
                "get_session_state",
            ] {
                policy.push_str(&format!("    - {tool}\n"));
            }
            policy.push_str("\nresources:\n  apps:\n");
            for executable in &canonical_applications {
                let quoted = serde_json::to_string(executable).map_err(|error| error.to_string())?;
                policy.push_str(&format!(
                    "    - executable: {quoted}\n      launch: true\n      windows: all\n      terminate: driver_launched\n"
                ));
            }
            policy.push_str("  desktop:\n    display: false\n");
            write_text_atomic(&policy_path, &policy).map_err(|error| error.to_string())?;
        }

        let mut warnings = Vec::new();
        let environment = [
            ("CUA_DRIVER_PERMISSION_MODE", "bounded".to_string()),
            (
                "CUA_DRIVER_SESSION_POLICY_FILE",
                policy_path.to_string_lossy().to_string(),
            ),
            ("CUA_DRIVER_SESSION_POLICY_APPROVED", "1".to_string()),
            ("CUA_DRIVER_RS_TELEMETRY_ENABLED", "false".to_string()),
            ("CUA_DRIVER_RS_UPDATE_CHECK", "false".to_string()),
        ];

        let claude_runtime_path = runtime_env::app_data_dir()
            .join("computer-control")
            .join("claude-runtime.json");
        let claude_runtime = if enabled {
            json!({
                "enabled": true,
                "server": {
                    "command": driver_path.as_ref().expect("driver path checked").to_string_lossy(),
                    "args": ["mcp"],
                    "env": environment
                        .iter()
                        .map(|(key, value)| ((*key).to_string(), Value::String(value.clone())))
                        .collect::<serde_json::Map<String, Value>>()
                }
            })
        } else {
            json!({ "enabled": false })
        };
        let claude_runtime_raw = serde_json::to_string_pretty(&claude_runtime)
            .map_err(|error| error.to_string())?;
        write_text_atomic(&claude_runtime_path, &format!("{claude_runtime_raw}\n"))
            .map_err(|error| error.to_string())?;

        if let Some(codex_path) = runtime_env::resolve_executable("codex") {
            let codex_configured = runtime_env::home_dir()
                .map(|directory| directory.join(".codex").join("config.toml"))
                .and_then(|path| fs::read_to_string(path).ok())
                .and_then(|raw| toml::from_str::<toml::Value>(&raw).ok())
                .and_then(|value| {
                    value
                        .get("mcp_servers")
                        .and_then(|servers| servers.get(MANAGED_SERVER_NAME))
                        .cloned()
                })
                .is_some();
            if codex_configured {
                if let Err(error) = run_command(
                    &codex_path,
                    &[OsString::from("mcp"), OsString::from("remove"), OsString::from(MANAGED_SERVER_NAME)],
                ) {
                    warnings.push(format!("Codex: {error}"));
                }
            }
            if enabled {
                let mut args = vec![OsString::from("mcp"), OsString::from("add")];
                for (key, value) in &environment {
                    args.push(OsString::from("--env"));
                    args.push(OsString::from(format!("{key}={value}")));
                }
                args.push(OsString::from(MANAGED_SERVER_NAME));
                args.push(OsString::from("--"));
                args.push(driver_path.as_ref().expect("driver path checked").as_os_str().to_os_string());
                args.push(OsString::from("mcp"));
                if let Err(error) = run_command(&codex_path, &args) {
                    warnings.push(format!("Codex: {error}"));
                }
            }
        }

        /*
        Panes 的 Claude 会话由 Agent SDK sidecar 托管，并且有意不读取用户级 Claude
        设置。保留以下 CLI 注册实现作为历史参考，但不再执行，避免把 Panes 功能写入
        用户独立使用的 Claude CLI 配置。sidecar 会读取上面的 claude-runtime.json。
        if let Some(claude_path) = runtime_env::resolve_executable("claude") {
            let claude_configured = runtime_env::home_dir()
                .map(|directory| directory.join(".claude.json"))
                .and_then(|path| fs::read_to_string(path).ok())
                .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
                .and_then(|value| {
                    value
                        .get("mcpServers")
                        .and_then(|servers| servers.get(MANAGED_SERVER_NAME))
                        .cloned()
                })
                .is_some();
            if claude_configured {
                if let Err(error) = run_command(
                    &claude_path,
                    &[
                        OsString::from("mcp"),
                        OsString::from("remove"),
                        OsString::from("--scope"),
                        OsString::from("user"),
                        OsString::from(MANAGED_SERVER_NAME),
                    ],
                ) {
                    warnings.push(format!("Claude Code: {error}"));
                }
            }
            if enabled {
                let env_value: serde_json::Map<String, Value> = environment
                    .iter()
                    .map(|(key, value)| ((*key).to_string(), Value::String(value.clone())))
                    .collect();
                let definition = json!({
                    "command": driver_path.as_ref().expect("driver path checked").to_string_lossy(),
                    "args": ["mcp"],
                    "env": env_value,
                });
                if let Err(error) = run_command(
                    &claude_path,
                    &[
                        OsString::from("mcp"),
                        OsString::from("add-json"),
                        OsString::from("--scope"),
                        OsString::from("user"),
                        OsString::from(MANAGED_SERVER_NAME),
                        OsString::from(definition.to_string()),
                    ],
                ) {
                    warnings.push(format!("Claude Code: {error}"));
                }
            }
        }
        */

        if runtime_env::resolve_executable("opencode").is_some() {
            match opencode_config_path() {
                Some(config_path) => {
                    let mut value = if config_path.exists() {
                        match fs::read_to_string(&config_path)
                            .map_err(anyhow::Error::from)
                            .and_then(|raw| serde_json::from_str::<Value>(&raw).map_err(anyhow::Error::from))
                        {
                            Ok(value) => value,
                            Err(error) => {
                                warnings.push(format!("OpenCode: {error}"));
                                Value::Null
                            }
                        }
                    } else {
                        json!({ "$schema": "https://opencode.ai/config.json" })
                    };

                    if let Some(root) = value.as_object_mut() {
                        let servers = root
                            .entry("mcp".to_string())
                            .or_insert_with(|| Value::Object(serde_json::Map::new()));
                        if let Some(servers) = servers.as_object_mut() {
                            let managed_server_existed = servers.contains_key(MANAGED_SERVER_NAME);
                            if enabled {
                                let env_value: serde_json::Map<String, Value> = environment
                                    .iter()
                                    .map(|(key, value)| ((*key).to_string(), Value::String(value.clone())))
                                    .collect();
                                servers.insert(
                                    MANAGED_SERVER_NAME.to_string(),
                                    json!({
                                        "type": "local",
                                        "command": [
                                            driver_path.as_ref().expect("driver path checked").to_string_lossy(),
                                            "mcp"
                                        ],
                                        "environment": env_value,
                                        "enabled": true
                                    }),
                                );
                            } else {
                                servers.remove(MANAGED_SERVER_NAME);
                            }
                            if enabled || managed_server_existed {
                                match serde_json::to_string_pretty(&value)
                                    .map_err(anyhow::Error::from)
                                    .and_then(|raw| {
                                        write_text_atomic(&config_path, &format!("{raw}\n"))
                                    }) {
                                    Ok(()) => {}
                                    Err(error) => warnings.push(format!("OpenCode: {error}")),
                                }
                            }
                        } else {
                            warnings.push("OpenCode: the mcp setting is not an object".to_string());
                        }
                    }
                }
                None => warnings.push("OpenCode: configuration directory was not found".to_string()),
            }
        }

        AppConfig::mutate(|config| {
            config.computer_control.enabled = enabled;
            config.computer_control.allowed_applications = canonical_applications;
            Ok(())
        })
        .map_err(|error| error.to_string())?;
        */

        let _ = allowed_applications;
        let canonical_applications: Vec<String> = Vec::new();
        let driver_path = runtime_env::resolve_executable("cua-driver");
        if enabled && driver_path.is_none() {
            return Err("Cua Driver is not installed or cannot be found".to_string());
        }

        let proxy_path = env::current_exe().map_err(|error| error.to_string())?;
        let broker_path = runtime_env::app_data_dir()
            .join("computer-control")
            .join("broker.json");
        let mut warnings = Vec::new();

        let claude_runtime_path = runtime_env::app_data_dir()
            .join("computer-control")
            .join("claude-runtime.json");
        let claude_runtime = if enabled {
            json!({
                "enabled": true,
                "server": {
                    "command": proxy_path.to_string_lossy(),
                    "args": [COMPUTER_CONTROL_PROXY_SUBCOMMAND],
                    "env": {
                        "PANES_COMPUTER_CONTROL_BROKER_FILE": broker_path.to_string_lossy(),
                        "PANES_CUA_DRIVER_PATH": driver_path.as_ref().expect("driver path checked").to_string_lossy(),
                        "PANES_COMPUTER_CONTROL_AGENT": "claude"
                    }
                }
            })
        } else {
            json!({ "enabled": false })
        };
        let claude_runtime_raw = serde_json::to_string_pretty(&claude_runtime)
            .map_err(|error| error.to_string())?;
        write_text_atomic(&claude_runtime_path, &format!("{claude_runtime_raw}\n"))
            .map_err(|error| error.to_string())?;

        if let Some(codex_path) = runtime_env::resolve_executable("codex") {
            let codex_config_path = runtime_env::home_dir()
                .map(|directory| directory.join(".codex").join("config.toml"));
            let codex_configured = codex_config_path
                .as_ref()
                .and_then(|path| fs::read_to_string(path).ok())
                .and_then(|raw| toml::from_str::<toml::Value>(&raw).ok())
                .and_then(|value| {
                    value
                        .get("mcp_servers")
                        .and_then(|servers| servers.get(MANAGED_SERVER_NAME))
                        .cloned()
                })
                .is_some();
            if codex_configured {
                if let Err(error) = run_command(
                    &codex_path,
                    &[
                        OsString::from("mcp"),
                        OsString::from("remove"),
                        OsString::from(MANAGED_SERVER_NAME),
                    ],
                ) {
                    warnings.push(format!("Codex: {error}"));
                }
            }
            if enabled {
                let args = vec![
                    OsString::from("mcp"),
                    OsString::from("add"),
                    OsString::from("--env"),
                    OsString::from(format!(
                        "{BROKER_FILE_ENV}={}",
                        broker_path.to_string_lossy()
                    )),
                    OsString::from("--env"),
                    OsString::from(format!(
                        "{DRIVER_PATH_ENV}={}",
                        driver_path
                            .as_ref()
                            .expect("driver path checked")
                            .to_string_lossy()
                    )),
                    OsString::from("--env"),
                    OsString::from(format!("{AGENT_ID_ENV}=codex")),
                    OsString::from(MANAGED_SERVER_NAME),
                    OsString::from("--"),
                    proxy_path.as_os_str().to_os_string(),
                    OsString::from(COMPUTER_CONTROL_PROXY_SUBCOMMAND),
                ];
                match run_command(&codex_path, &args) {
                    Ok(_) => {
                        if let Some(config_path) = codex_config_path.as_ref() {
                            if let Ok(mut raw) = fs::read_to_string(config_path) {
                                let headers = [
                                    format!("[mcp_servers.{MANAGED_SERVER_NAME}]"),
                                    format!("[mcp_servers.\"{MANAGED_SERVER_NAME}\"]"),
                                ];
                                if let Some((header_index, header)) = headers.iter().find_map(|header| {
                                    raw.find(header).map(|index| (index, header))
                                }) {
                                    let body_start = header_index + header.len();
                                    let body_end = raw[body_start..]
                                        .find("\n[")
                                        .map(|offset| body_start + offset)
                                        .unwrap_or(raw.len());
                                    if !raw[body_start..body_end]
                                        .contains("default_tools_approval_mode")
                                    {
                                        raw.insert_str(
                                            body_start,
                                            "\ndefault_tools_approval_mode = \"approve\"",
                                        );
                                        if let Err(error) = write_text_atomic(config_path, &raw) {
                                            warnings.push(format!("Codex: {error}"));
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Err(error) => warnings.push(format!("Codex: {error}")),
                }
            }
        }

        if runtime_env::resolve_executable("opencode").is_some() {
            match opencode_config_path() {
                Some(config_path) => {
                    let mut value = if config_path.exists() {
                        match fs::read_to_string(&config_path)
                            .map_err(anyhow::Error::from)
                            .and_then(|raw| {
                                serde_json::from_str::<Value>(&raw).map_err(anyhow::Error::from)
                            }) {
                            Ok(value) => value,
                            Err(error) => {
                                warnings.push(format!("OpenCode: {error}"));
                                Value::Null
                            }
                        }
                    } else {
                        json!({ "$schema": "https://opencode.ai/config.json" })
                    };

                    if let Some(root) = value.as_object_mut() {
                        let servers = root
                            .entry("mcp".to_string())
                            .or_insert_with(|| Value::Object(serde_json::Map::new()));
                        let mut should_write = false;
                        if let Some(servers) = servers.as_object_mut() {
                            let managed_server_existed = servers.contains_key(MANAGED_SERVER_NAME);
                            if enabled {
                                servers.insert(
                                    MANAGED_SERVER_NAME.to_string(),
                                    json!({
                                        "type": "local",
                                        "command": [
                                            proxy_path.to_string_lossy(),
                                            COMPUTER_CONTROL_PROXY_SUBCOMMAND
                                        ],
                                        "environment": {
                                            "PANES_COMPUTER_CONTROL_BROKER_FILE": broker_path.to_string_lossy(),
                                            "PANES_CUA_DRIVER_PATH": driver_path.as_ref().expect("driver path checked").to_string_lossy(),
                                            "PANES_COMPUTER_CONTROL_AGENT": "opencode"
                                        },
                                        "enabled": true
                                    }),
                                );
                            } else {
                                servers.remove(MANAGED_SERVER_NAME);
                            }
                            should_write = enabled || managed_server_existed;
                        } else {
                            warnings.push("OpenCode: the mcp setting is not an object".to_string());
                        }

                        let permission_key = format!("{MANAGED_SERVER_NAME}_*");
                        if enabled {
                            let permission = root
                                .entry("permission".to_string())
                                .or_insert_with(|| Value::Object(serde_json::Map::new()));
                            if let Some(permission) = permission.as_object_mut() {
                                permission.insert(permission_key, Value::String("allow".to_string()));
                            } else if let Some(existing) = permission.as_str().map(str::to_string) {
                                *permission = json!({
                                    "*": existing,
                                    permission_key: "allow"
                                });
                            } else {
                                warnings.push("OpenCode: the permission setting is not supported".to_string());
                            }
                        } else if let Some(permission) = root
                            .get_mut("permission")
                            .and_then(Value::as_object_mut)
                        {
                            permission.remove(&permission_key);
                        }

                        if should_write {
                            match serde_json::to_string_pretty(&value)
                                .map_err(anyhow::Error::from)
                                .and_then(|raw| write_text_atomic(&config_path, &format!("{raw}\n")))
                            {
                                Ok(()) => {}
                                Err(error) => warnings.push(format!("OpenCode: {error}")),
                            }
                        }
                    }
                }
                None => warnings.push("OpenCode: configuration directory was not found".to_string()),
            }
        }

        AppConfig::mutate(|config| {
            config.computer_control.enabled = enabled;
            config.computer_control.allowed_applications = canonical_applications;
            Ok(())
        })
        .map_err(|error| error.to_string())?;

        collect_status(warnings).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?;

    if !enabled {
        approvals.revoke_all().await;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::{filter_tool_list_response, requested_resource};
    use serde_json::{json, Value};

    #[test]
    fn tool_list_exposes_only_the_reviewed_computer_control_surface() {
        let filtered = filter_tool_list_response(
            &json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": {
                    "tools": [
                        { "name": "click" },
                        { "name": "kill_app" },
                        { "name": "get_desktop_state" }
                    ]
                }
            })
            .to_string(),
        );
        let value: Value = serde_json::from_str(&filtered).expect("valid JSON response");
        let tools = value["result"]["tools"]
            .as_array()
            .expect("tools should remain an array");
        assert_eq!(tools, &[json!({ "name": "click" })]);
    }

    #[test]
    fn desktop_scoped_input_is_denied_before_authorization() {
        let result = requested_resource(
            "click",
            &json!({
                "scope": "desktop",
                "x": 10,
                "y": 20
            }),
        );
        assert!(result.is_err());

        let result = requested_resource(
            "start_session",
            &json!({
                "session": "test-session",
                "capture_scope": "desktop"
            }),
        );
        assert!(result.is_err());
    }

    #[test]
    fn launch_target_is_resolved_at_call_time() {
        let (resource, application) =
            requested_resource("launch_app", &json!({ "path": "C:\\work\\future-app.exe" }))
                .expect("launch path should identify an application");
        assert_eq!(resource, "application:c:\\work\\future-app.exe");
        assert_eq!(application, "C:\\work\\future-app.exe");
    }
}
