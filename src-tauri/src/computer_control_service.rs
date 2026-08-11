use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::{Arc, Mutex as StdMutex},
    time::Duration,
};

use serde::Serialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};
use tokio::{
    sync::{oneshot, Mutex},
    time::timeout,
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{computer_control_sdk::CuaDriverSdk, config::app_config::AppConfig};

const APPROVAL_TIMEOUT: Duration = Duration::from_secs(300);
const COMPUTER_CONTROL_APPROVAL_EVENT: &str = "computer-control-approval-requested";
const COMPUTER_CONTROL_NAMESPACE: &str = "panes_computer_control";

const REVIEWED_TOOLS: &[(&str, &str)] = &[
    ("start_session", "创建当前任务的电脑操作会话"),
    ("end_session", "结束当前任务的电脑操作会话"),
    ("launch_app", "启动指定应用程序"),
    ("list_apps", "列出可见应用程序"),
    ("list_windows", "列出可见应用窗口"),
    ("bring_to_front", "将指定应用窗口置于前台"),
    ("get_window_state", "读取指定窗口状态"),
    ("get_accessibility_tree", "读取指定应用的可访问性树"),
    ("verify_state", "验证指定应用当前状态"),
    ("get_screen_size", "读取屏幕尺寸元数据"),
    ("get_cursor_position", "读取当前光标位置"),
    ("click", "点击指定应用窗口"),
    ("double_click", "双击指定应用窗口"),
    ("right_click", "右键点击指定应用窗口"),
    ("drag", "在指定应用窗口内拖动"),
    ("type_text", "向指定应用输入文本"),
    ("press_key", "向指定应用发送按键"),
    ("hotkey", "向指定应用发送组合键"),
    ("set_value", "设置指定应用控件值"),
    ("invoke_menu", "调用指定应用菜单"),
    ("scroll", "滚动指定应用窗口"),
    ("move_cursor", "移动指定应用窗口内的光标"),
    ("zoom", "调整指定应用窗口缩放"),
    ("clipboard_read", "读取当前任务剪贴板"),
    ("clipboard_write", "写入当前任务剪贴板"),
    ("health_report", "读取电脑操作运行状态"),
    ("get_session_state", "读取当前电脑操作会话状态"),
];

#[derive(Debug)]
struct PendingAuthorization {
    grant_key: String,
    response: oneshot::Sender<bool>,
}

impl Default for ComputerControlService {
    fn default() -> Self {
        Self::new(Arc::new(CuaDriverSdk::new()))
    }
}

pub fn dynamic_tool_success(value: Value) -> Value {
    json!({
        "contentItems": content_items(value),
        "success": true
    })
}

pub fn dynamic_tool_failure(error: impl Into<String>) -> Value {
    json!({
        "contentItems": [{
            "type": "inputText",
            "text": error.into()
        }],
        "success": false
    })
}

fn content_items(value: Value) -> Vec<Value> {
    if let Some(items) = value.get("contentItems").and_then(Value::as_array) {
        return items.clone();
    }
    if let Some(items) = value.get("content").and_then(Value::as_array) {
        let mapped = items
            .iter()
            .filter_map(|item| match item.get("type").and_then(Value::as_str) {
                Some("text") => item
                    .get("text")
                    .and_then(Value::as_str)
                    .map(|text| json!({"type": "inputText", "text": text})),
                Some("image") => {
                    let data = item.get("data").and_then(Value::as_str)?;
                    let mime = item
                        .get("mimeType")
                        .and_then(Value::as_str)
                        .unwrap_or("image/png");
                    Some(json!({
                        "type": "inputImage",
                        "imageUrl": format!("data:{mime};base64,{data}")
                    }))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        if !mapped.is_empty() {
            return mapped;
        }
    }
    if let Some(image_url) = value
        .get("imageUrl")
        .or_else(|| value.get("dataUrl"))
        .and_then(Value::as_str)
        .filter(|value| value.starts_with("data:image/"))
    {
        return vec![json!({
            "type": "inputImage",
            "imageUrl": image_url
        })];
    }

    let text = match value {
        Value::String(text) => text,
        value => serde_json::to_string(&value).unwrap_or_else(|_| "null".to_string()),
    };
    vec![json!({
        "type": "inputText",
        "text": text
    })]
}

fn normalize_arguments(arguments: Value) -> Result<Value, String> {
    match arguments {
        Value::Object(_) => Ok(arguments),
        Value::String(raw) => serde_json::from_str(&raw).map_err(|error| {
            service_error(
                "invalid_request",
                &format!("电脑操作 arguments 不是有效 JSON：{error}"),
            )
        }),
        _ => Err(service_error(
            "invalid_request",
            "电脑操作 arguments 必须是 JSON 对象",
        )),
    }
}

fn target_resource(tool: &str, arguments: &Value) -> Result<TargetResource, String> {
    let object = arguments.as_object();
    let desktop_scope = object
        .and_then(|value| value.get("scope"))
        .and_then(Value::as_str)
        .map(|scope| scope.eq_ignore_ascii_case("desktop"))
        .unwrap_or(false)
        || object
            .and_then(|value| value.get("capture_scope"))
            .and_then(Value::as_str)
            .map(|scope| scope.eq_ignore_ascii_case("desktop"))
            .unwrap_or(false);
    if desktop_scope {
        return Err(service_error(
            "target_scope_mismatch",
            "Panes 不允许全桌面范围的电脑操作",
        ));
    }

    for key in ["launch_path", "path", "aumid", "application", "name"] {
        if let Some(value) = object
            .and_then(|value| value.get(key))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return Ok(application_resource(value));
        }
    }
    if let Some(window_id) = object.and_then(|value| value.get("window_id")) {
        if let Some(value) = window_id.as_str().filter(|value| !value.trim().is_empty()) {
            return Ok(TargetResource {
                key: format!("window:{}", value.to_lowercase()),
                display: format!("窗口 {value}"),
                scope: "window",
            });
        }
        if let Some(value) = window_id.as_u64() {
            return Ok(TargetResource {
                key: format!("window:{value}"),
                display: format!("窗口 {value}"),
                scope: "window",
            });
        }
    }
    if let Some(pid) = object
        .and_then(|value| value.get("pid"))
        .and_then(Value::as_u64)
    {
        return Ok(TargetResource {
            key: format!("pid:{pid}"),
            display: format!("PID {pid}"),
            scope: "application",
        });
    }

    match tool {
        "list_apps" | "list_windows" => Ok(TargetResource {
            key: "observation:applications".to_string(),
            display: "Windows 应用和窗口".to_string(),
            scope: "observation",
        }),
        "clipboard_read" | "clipboard_write" => Ok(TargetResource {
            key: "resource:clipboard".to_string(),
            display: "当前任务剪贴板".to_string(),
            scope: "clipboard",
        }),
        "start_session"
        | "end_session"
        | "health_report"
        | "get_screen_size"
        | "get_cursor_position"
        | "get_session_state" => Ok(TargetResource {
            key: "metadata:computer-control".to_string(),
            display: "电脑操作运行状态".to_string(),
            scope: "metadata",
        }),
        _ => Err(service_error(
            "target_not_found",
            &format!("电脑操作工具 `{tool}` 缺少应用或窗口目标"),
        )),
    }
}

fn application_resource(application: &str) -> TargetResource {
    let display = Path::new(application)
        .canonicalize()
        .ok()
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_else(|| application.to_string());
    TargetResource {
        key: format!("application:{}", display.to_lowercase()),
        display,
        scope: "application",
    }
}

fn target_is_panes(application: &str) -> bool {
    let Some(current_path) = std::env::current_exe()
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

fn operation_kind(tool: &str) -> &'static str {
    match tool {
        "click" | "double_click" | "right_click" | "drag" | "type_text" | "press_key"
        | "hotkey" | "set_value" | "invoke_menu" | "scroll" | "move_cursor" | "zoom"
        | "bring_to_front" | "launch_app" => "input",
        "clipboard_read" | "clipboard_write" => "clipboard",
        "start_session" | "end_session" => "session",
        _ => "observe",
    }
}

fn service_error(code: &str, message: &str) -> String {
    format!("{code}: {message}")
}

#[cfg(test)]
mod tests {
    use super::{
        dynamic_tool_failure, dynamic_tool_success, target_resource, ComputerControlService,
    };
    use serde_json::json;
    use std::sync::Arc;

    #[test]
    fn dynamic_tools_are_namespaced_and_do_not_start_runtime() {
        let value = ComputerControlService::dynamic_tools_spec();
        assert_eq!(value[0]["type"], "namespace");
        assert_eq!(value[0]["name"], "panes_computer_control");
        assert!(!value[0]["tools"].as_array().unwrap().is_empty());
        let service = ComputerControlService::default();
        assert!(!service.sdk().status().initialized);
        let _ = Arc::new(service);
    }

    #[test]
    fn desktop_scope_and_unscoped_input_are_rejected() {
        assert!(target_resource("click", &json!({"scope": "desktop"})).is_err());
        assert!(target_resource("click", &json!({"x": 10, "y": 20})).is_err());
        assert!(target_resource("click", &json!({"path": "notepad.exe"})).is_ok());
    }

    #[test]
    fn dynamic_tool_result_has_codex_content_items() {
        assert_eq!(dynamic_tool_success(json!({"ok": true}))["success"], true);
        assert_eq!(
            dynamic_tool_failure("permission_denied: no")["success"],
            false
        );
        let image = dynamic_tool_success(json!({
            "content": [{"type": "image", "data": "AQ==", "mimeType": "image/png"}]
        }));
        assert_eq!(image["contentItems"][0]["type"], "inputImage");
    }
}

#[derive(Default)]
struct AuthorizationState {
    pending: HashMap<String, PendingAuthorization>,
    grants: HashSet<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ComputerControlAuthorizationRequest {
    request_id: String,
    agent: String,
    tool: String,
    call_id: String,
    application: String,
    operation: String,
    scope: String,
    thread_id: String,
    turn_id: String,
}

#[derive(Debug, Clone)]
struct TargetResource {
    key: String,
    display: String,
    scope: &'static str,
}

pub struct ComputerControlService {
    sdk: Arc<CuaDriverSdk>,
    state: Mutex<AuthorizationState>,
    app_handle: StdMutex<Option<AppHandle>>,
}

impl ComputerControlService {
    pub fn new(sdk: Arc<CuaDriverSdk>) -> Self {
        Self {
            sdk,
            state: Mutex::new(AuthorizationState::default()),
            app_handle: StdMutex::new(None),
        }
    }

    pub fn bind_app_handle(&self, handle: AppHandle) {
        if let Ok(mut current) = self.app_handle.lock() {
            *current = Some(handle);
        }
    }

    pub fn dynamic_tools_spec() -> Value {
        let tools = REVIEWED_TOOLS
            .iter()
            .map(|(name, description)| {
                json!({
                    "type": "function",
                    "name": name,
                    "description": description,
                    "inputSchema": {
                        "type": "object",
                        "additionalProperties": true
                    }
                })
            })
            .collect::<Vec<_>>();

        json!([
            {
                "type": "namespace",
                "name": COMPUTER_CONTROL_NAMESPACE,
                "description": "Panes 的电脑操作能力。每次实际调用都由 Panes 在执行前申请授权。",
                "tools": tools
            }
        ])
    }

    pub fn is_reviewed_tool(tool: &str) -> bool {
        REVIEWED_TOOLS.iter().any(|(name, _)| *name == tool)
    }

    #[cfg(test)]
    pub fn sdk(&self) -> Arc<CuaDriverSdk> {
        self.sdk.clone()
    }

    async fn has_grant(&self, grant_key: &str) -> bool {
        self.state.lock().await.grants.contains(grant_key)
    }

    pub async fn respond(&self, request_id: &str, allowed: bool) -> Result<bool, String> {
        let pending = {
            let mut state = self.state.lock().await;
            state.pending.remove(request_id)
        };
        let Some(pending) = pending else {
            return Ok(false);
        };

        if allowed {
            self.state.lock().await.grants.insert(pending.grant_key);
        }
        let _ = pending.response.send(allowed);
        Ok(true)
    }

    pub async fn revoke_all(&self) {
        let pending = {
            let mut state = self.state.lock().await;
            state.grants.clear();
            state
                .pending
                .drain()
                .map(|(_, pending)| pending.response)
                .collect::<Vec<_>>()
        };
        for response in pending {
            let _ = response.send(false);
        }
    }

    pub async fn revoke_turn(&self, thread_id: &str, turn_id: Option<&str>) {
        let prefix = match turn_id.map(str::trim).filter(|value| !value.is_empty()) {
            Some(turn_id) => format!("{}\n{}\n", thread_id.trim(), turn_id),
            None => format!("{}\n", thread_id.trim()),
        };
        let pending = {
            let mut state = self.state.lock().await;
            state.grants.retain(|key| !key.starts_with(&prefix));
            let request_ids = state
                .pending
                .iter()
                .filter(|(_, pending)| pending.grant_key.starts_with(&prefix))
                .map(|(request_id, _)| request_id.clone())
                .collect::<Vec<_>>();
            request_ids
                .into_iter()
                .filter_map(|request_id| state.pending.remove(&request_id))
                .map(|pending| pending.response)
                .collect::<Vec<_>>()
        };
        for response in pending {
            let _ = response.send(false);
        }
    }

    pub async fn invoke_for_codex(
        &self,
        thread_id: &str,
        turn_id: &str,
        tool: &str,
        call_id: &str,
        arguments: Value,
        cancellation: CancellationToken,
    ) -> Result<Value, String> {
        self.invoke_for_engine(
            "codex",
            thread_id,
            turn_id,
            tool,
            call_id,
            arguments,
            cancellation,
        )
        .await
    }

    pub async fn invoke_for_engine(
        &self,
        agent: &str,
        thread_id: &str,
        turn_id: &str,
        tool: &str,
        call_id: &str,
        arguments: Value,
        cancellation: CancellationToken,
    ) -> Result<Value, String> {
        let agent = agent.trim();
        let thread_id = thread_id.trim();
        let turn_id = turn_id.trim();
        let tool = tool.trim();
        if agent.is_empty() || thread_id.is_empty() || turn_id.is_empty() {
            return Err(service_error(
                "invalid_request",
                "电脑操作请求缺少 agent、threadId 或 turnId",
            ));
        }
        if !Self::is_reviewed_tool(tool) {
            return Err(service_error(
                "tool_not_allowed",
                &format!("未审核的电脑操作工具：{tool}"),
            ));
        }
        let enabled = AppConfig::load_or_create()
            .map(|config| config.computer_control.enabled)
            .unwrap_or(false);
        if !enabled {
            return Err(service_error(
                "computer_control_disabled",
                "Panes 的电脑操作能力开关未开启",
            ));
        }
        let arguments = normalize_arguments(arguments)?;
        let target = target_resource(tool, &arguments)?;
        if target_is_panes(&target.display) {
            return Err(service_error(
                "target_scope_mismatch",
                "Panes 不允许把自身窗口作为电脑操作目标",
            ));
        }
        let operation = operation_kind(tool);
        let grant_key = format!(
            "{thread_id}\n{turn_id}\n{agent}\n{}\n{operation}",
            target.key
        );
        if !self.has_grant(&grant_key).await {
            self.request_authorization(
                agent,
                thread_id,
                turn_id,
                tool,
                call_id,
                &target,
                operation,
                grant_key,
                cancellation.clone(),
            )
            .await?;
        }
        self.sdk
            .invoke(tool, &arguments)
            .map_err(|error| service_error("sdk_unavailable", &error))
    }

    async fn request_authorization(
        &self,
        agent: &str,
        thread_id: &str,
        turn_id: &str,
        tool: &str,
        call_id: &str,
        target: &TargetResource,
        operation: &str,
        grant_key: String,
        cancellation: CancellationToken,
    ) -> Result<(), String> {
        let request_id = Uuid::new_v4().to_string();
        let (response_tx, response_rx) = oneshot::channel();
        {
            let mut state = self.state.lock().await;
            state.pending.insert(
                request_id.clone(),
                PendingAuthorization {
                    grant_key,
                    response: response_tx,
                },
            );
        }

        let request = ComputerControlAuthorizationRequest {
            request_id: request_id.clone(),
            agent: agent.to_string(),
            tool: tool.to_string(),
            call_id: call_id.to_string(),
            application: target.display.clone(),
            operation: operation.to_string(),
            scope: target.scope.to_string(),
            thread_id: thread_id.to_string(),
            turn_id: turn_id.to_string(),
        };
        let emit_result = self
            .app_handle
            .lock()
            .map_err(|_| "电脑操作授权窗口状态已损坏".to_string())?
            .as_ref()
            .ok_or_else(|| "Panes 窗口尚未就绪，无法发起电脑操作授权".to_string())?
            .emit(COMPUTER_CONTROL_APPROVAL_EVENT, request);
        if let Err(error) = emit_result {
            self.state.lock().await.pending.remove(&request_id);
            return Err(service_error(
                "authorization_required",
                &format!("无法显示电脑操作授权窗口：{error}"),
            ));
        }

        let result = tokio::select! {
            _ = cancellation.cancelled() => Err(service_error("request_timeout", "电脑操作任务已取消")),
            result = timeout(APPROVAL_TIMEOUT, response_rx) => match result {
                Ok(Ok(true)) => Ok(()),
                Ok(Ok(false)) => Err(service_error("permission_denied", "用户拒绝了电脑操作授权")),
                Ok(Err(_)) => Err(service_error("authorization_required", "电脑操作授权请求已失效")),
                Err(_) => Err(service_error("request_timeout", "电脑操作授权等待超时")),
            },
        };

        self.state.lock().await.pending.remove(&request_id);
        result
    }
}
