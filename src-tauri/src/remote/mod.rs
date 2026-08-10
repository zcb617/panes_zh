use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::Arc,
    time::Duration,
};

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use futures::{SinkExt, StreamExt};
use qrcode::{render::svg, QrCode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Listener};
use tokio::{
    fs as tokio_fs,
    sync::{Mutex, RwLock},
};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    commands::{
        chat::{
            cancel_turn_inner, save_pasted_image_attachment, send_message_inner,
            ChatAttachmentPayload,
        },
        threads::create_thread_with_defaults,
    },
    config::app_config::{AppConfig, RemoteAccessConfig, RemoteDeviceConfig},
    // 历史分页使用的 MessageWindowCursorDto 已停用，保留原引用记录。
    // models::MessageWindowCursorDto,
    state::AppState,
};

const REMOTE_PROTOCOL_VERSION: u32 = 1;
// 历史分页窗口限制已停用：手机进入会话时一次读取完整消息。
// const MESSAGE_WINDOW_DEFAULT_LIMIT: usize = 50;
// const MESSAGE_WINDOW_MAX_LIMIT: usize = 100;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteDeviceDto {
    pub id: String,
    pub name: String,
    pub paired_at: Option<String>,
    pub last_connected_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteAccessStatusDto {
    pub enabled: bool,
    pub endpoint: String,
    pub tunnel_id: String,
    pub connected: bool,
    pub peer_online: bool,
    pub last_error: Option<String>,
    pub pairing_payload: Option<String>,
    pub pairing_qr_svg: Option<String>,
    pub pairing_expires_at: Option<String>,
    pub paired: bool,
    pub devices: Vec<RemoteDeviceDto>,
}

#[derive(Debug, Clone)]
struct RemoteRuntimeState {
    generation: u64,
    config: RemoteAccessConfig,
    connected: bool,
    peer_online: bool,
    last_error: Option<String>,
    pairing_token: Option<String>,
    pairing_expires_at: Option<DateTime<Utc>>,
}

impl Default for RemoteRuntimeState {
    fn default() -> Self {
        Self {
            generation: 0,
            config: RemoteAccessConfig::default(),
            connected: false,
            peer_online: false,
            last_error: None,
            pairing_token: None,
            pairing_expires_at: None,
        }
    }
}

#[derive(Default)]
pub struct RemoteTunnelManager {
    runtime: RwLock<RemoteRuntimeState>,
    /// 当前隧道内已经完成 `device.identify` 的在线设备集合。
    online_devices: RwLock<HashSet<String>>,
    uploads: Mutex<HashMap<String, RemoteUploadState>>,
    cancellation: Mutex<Option<CancellationToken>>,
}

#[derive(Debug, Deserialize)]
struct RemoteRequest {
    version: u32,
    kind: String,
    id: String,
    method: String,
    auth: String,
    #[serde(default)]
    payload: Value,
}

struct RemoteUploadState {
    file_name: String,
    mime_type: String,
    chunk_count: u32,
    next_chunk: u32,
    data_base64: String,
}

fn device_name_from_payload(payload: &Value) -> String {
    payload
        .get("device_name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.chars().take(80).collect())
        .unwrap_or_else(|| "Panes Mobile".to_string())
}

/// 将聊天完成通知包装成移动端约定的远程事件。
///
/// 事件只携带刚刚完成持久化的单条助手消息，避免按线程扫描或发送流式差异。
fn build_completed_message_event(target_device_id: &str, payload: &Value) -> Option<Value> {
    let thread_id = payload
        .get("threadId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let message_id = payload
        .get("messageId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let message = payload.get("message")?.clone();
    Some(json!({
        "version": REMOTE_PROTOCOL_VERSION,
        "kind": "event",
        "event": "thread.message.completed",
        "targetDeviceId": target_device_id,
        "payload": {
            "threadId": thread_id,
            "messageId": message_id,
            "message": message,
        },
    }))
}

impl RemoteTunnelManager {
    pub async fn status(&self) -> RemoteAccessStatusDto {
        let runtime = self.runtime.read().await;
        let pairing_token = runtime.pairing_token.as_ref().filter(|_| {
            runtime
                .pairing_expires_at
                .map(|expires_at| expires_at > Utc::now())
                .unwrap_or(false)
        });
        let pairing_payload = if runtime.config.enabled
            && !runtime.config.tunnel_id.is_empty()
            && !runtime.config.credential.is_empty()
            && pairing_token.is_some()
        {
            serde_json::to_string(&json!({
                "version": REMOTE_PROTOCOL_VERSION,
                "endpoint": runtime.config.endpoint,
                "tunnel_id": runtime.config.tunnel_id,
                "relay_credential": runtime.config.credential,
                "pairing_token": pairing_token,
                "expires_at": runtime.pairing_expires_at.map(|value| value.to_rfc3339()),
            }))
            .ok()
        } else {
            None
        };
        let pairing_qr_svg = pairing_payload.as_ref().and_then(|payload| {
            QrCode::new(payload.as_bytes()).ok().map(|code| {
                code.render::<svg::Color>()
                    .min_dimensions(256, 256)
                    .dark_color(svg::Color("#111827"))
                    .light_color(svg::Color("#ffffff"))
                    .build()
            })
        });
        let mut devices = runtime
            .config
            .devices
            .iter()
            .map(|device| RemoteDeviceDto {
                id: device.id.clone(),
                name: if device.name.trim().is_empty() {
                    "Panes Mobile".to_string()
                } else {
                    device.name.clone()
                },
                paired_at: (!device.paired_at.is_empty()).then(|| device.paired_at.clone()),
                last_connected_at: (!device.last_connected_at.is_empty())
                    .then(|| device.last_connected_at.clone()),
            })
            .collect::<Vec<_>>();
        if !runtime.config.device_credential.is_empty() {
            devices.push(RemoteDeviceDto {
                id: "legacy".to_string(),
                name: "Panes Mobile".to_string(),
                paired_at: None,
                last_connected_at: None,
            });
        }
        RemoteAccessStatusDto {
            enabled: runtime.config.enabled,
            endpoint: runtime.config.endpoint.clone(),
            tunnel_id: runtime.config.tunnel_id.clone(),
            connected: runtime.connected,
            peer_online: runtime.peer_online,
            last_error: runtime.last_error.clone(),
            pairing_payload,
            pairing_qr_svg,
            pairing_expires_at: runtime.pairing_expires_at.map(|value| value.to_rfc3339()),
            paired: !devices.is_empty(),
            devices,
        }
    }

    pub async fn refresh_pairing_token(&self) -> RemoteAccessStatusDto {
        let mut runtime = self.runtime.write().await;
        if runtime.config.enabled {
            runtime.pairing_token = Some(format!(
                "{}{}",
                uuid::Uuid::new_v4().simple(),
                uuid::Uuid::new_v4().simple()
            ));
            runtime.pairing_expires_at = Some(Utc::now() + ChronoDuration::minutes(5));
        }
        drop(runtime);
        self.status().await
    }

    pub async fn configure(
        self: &Arc<Self>,
        app: AppHandle,
        state: AppState,
        config: RemoteAccessConfig,
    ) {
        if let Some(cancellation) = self.cancellation.lock().await.take() {
            cancellation.cancel();
        }
        self.online_devices.write().await.clear();
        self.uploads.lock().await.clear();

        let generation = {
            let mut runtime = self.runtime.write().await;
            runtime.generation = runtime.generation.wrapping_add(1);
            runtime.config = config.clone();
            runtime.connected = false;
            runtime.peer_online = false;
            runtime.last_error = None;
            // 配对凭据只在用户点击“添加设备”时生成，打开设置或重连服务时不生成二维码。
            runtime.pairing_token = None;
            runtime.pairing_expires_at = None;
            runtime.generation
        };
        let _ = app.emit("remote-access-updated", self.status().await);

        if !config.enabled {
            return;
        }

        let cancellation = CancellationToken::new();
        *self.cancellation.lock().await = Some(cancellation.clone());
        let manager = self.clone();
        tauri::async_runtime::spawn(async move {
            let retry_delays = [1_u64, 2, 5, 10, 30];
            let mut retry_index = 0_usize;

            loop {
                if cancellation.is_cancelled() {
                    break;
                }

                let connection = tokio::select! {
                    _ = cancellation.cancelled() => break,
                    result = connect_async(config.endpoint.as_str()) => result,
                };

                let socket = match connection {
                    Ok((socket, _)) => socket,
                    Err(error) => {
                        {
                            let mut runtime = manager.runtime.write().await;
                            if runtime.generation != generation {
                                break;
                            }
                            runtime.connected = false;
                            runtime.peer_online = false;
                            runtime.last_error = Some(error.to_string());
                        }
                        let _ = app.emit("remote-access-updated", manager.status().await);
                        let delay = retry_delays[retry_index.min(retry_delays.len() - 1)];
                        retry_index = (retry_index + 1).min(retry_delays.len() - 1);
                        tokio::select! {
                            _ = cancellation.cancelled() => break,
                            _ = tokio::time::sleep(Duration::from_secs(delay)) => {}
                        }
                        continue;
                    }
                };

                retry_index = 0;
                let (mut sink, mut stream) = socket.split();
                let hello = json!({
                    "version": REMOTE_PROTOCOL_VERSION,
                    "type": "tunnel.hello",
                    "role": "desktop",
                    "tunnel_id": config.tunnel_id,
                    "credential": config.credential,
                });
                if let Err(error) = sink.send(Message::Text(hello.to_string().into())).await {
                    let mut runtime = manager.runtime.write().await;
                    if runtime.generation == generation {
                        runtime.last_error = Some(error.to_string());
                    }
                    continue;
                }

                /*
                // 旧版按 thread-updated 做全线程差异扫描的实现已废弃。
                let mut snapshots: HashMap<String, String> = HashMap::new();
                let mut snapshot_interval = tokio::time::interval(Duration::from_millis(650));
                snapshot_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                let (thread_updated_tx, mut thread_updated_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
                let thread_updated_listener = app.listen("thread-updated", move |event| {
                    let Ok(payload) = serde_json::from_str::<Value>(event.payload()) else { return; };
                    let Some(thread_id) = payload.get("threadId").and_then(Value::as_str) else { return; };
                    let _ = thread_updated_tx.send(thread_id.to_string());
                });
                let mut message_versions: HashMap<String, HashMap<String, String>> = HashMap::new();
                */
                // 只监听助手消息完成事件；流式块和线程状态更新不会进入远程推送通道。
                let (completed_message_tx, mut completed_message_rx) =
                    tokio::sync::mpsc::unbounded_channel::<Value>();
                let completed_message_listener = app.listen("assistant-message-completed", move |event| {
                    let Ok(payload) = serde_json::from_str::<Value>(event.payload()) else {
                        return;
                    };
                    let _ = completed_message_tx.send(payload);
                });
                let mut disconnected_error: Option<String> = None;

                loop {
                    tokio::select! {
                        _ = cancellation.cancelled() => {
                            let _ = sink.close().await;
                            break;
                        }
                        incoming = stream.next() => {
                            let Some(incoming) = incoming else {
                                disconnected_error = Some("WSS connection closed".to_string());
                                break;
                            };
                            let incoming = match incoming {
                                Ok(message) => message,
                                Err(error) => {
                                    disconnected_error = Some(error.to_string());
                                    break;
                                }
                            };
                            match incoming {
                                Message::Ping(payload) => {
                                    if let Err(error) = sink.send(Message::Pong(payload)).await {
                                        disconnected_error = Some(error.to_string());
                                        break;
                                    }
                                }
                                Message::Close(_) => {
                                    disconnected_error = Some("WSS connection closed".to_string());
                                    break;
                                }
                                Message::Text(text) => {
                                    let value: Value = match serde_json::from_str(text.as_str()) {
                                        Ok(value) => value,
                                        Err(_) => continue,
                                    };
                                    match value.get("type").and_then(Value::as_str) {
                                        Some("tunnel.ready") => {
                                            let peer_online = value.get("peer_online").and_then(Value::as_bool).unwrap_or(false);
                                            {
                                                let mut runtime = manager.runtime.write().await;
                                                if runtime.generation != generation { break; }
                                                runtime.connected = true;
                                                runtime.peer_online = peer_online;
                                                runtime.last_error = None;
                                            }
                                            let _ = app.emit("remote-access-updated", manager.status().await);
                                            continue;
                                        }
                                        Some("tunnel.peer_online") => {
                                            {
                                                let mut runtime = manager.runtime.write().await;
                                                if runtime.generation != generation { break; }
                                                runtime.peer_online = true;
                                            }
                                            let _ = app.emit("remote-access-updated", manager.status().await);
                                            continue;
                                        }
                                        Some("tunnel.peer_offline") => {
                                            {
                                                let mut runtime = manager.runtime.write().await;
                                                if runtime.generation != generation { break; }
                                                runtime.peer_online = false;
                                            }
                                            // 隧道确认全部移动对端离线后，清理在线设备；切换会话或离开页面不会触发此处。
                                            manager.online_devices.write().await.clear();
                                            let _ = app.emit("remote-access-updated", manager.status().await);
                                            continue;
                                        }
                                        _ => {}
                                    }

                                    let request: RemoteRequest = match serde_json::from_value(value) {
                                        Ok(request) => request,
                                        Err(error) => {
                                            let response = json!({
                                                "version": REMOTE_PROTOCOL_VERSION,
                                                "kind": "response",
                                                "id": "invalid",
                                                "ok": false,
                                                "error": { "code": "invalid_request", "message": error.to_string() }
                                            });
                                            let _ = sink.send(Message::Text(response.to_string().into())).await;
                                            continue;
                                        }
                                    };

                                    if request.version != REMOTE_PROTOCOL_VERSION
                                        || request.kind != "request"
                                        || request.id.trim().is_empty()
                                        || request.id.len() > 128
                                    {
                                        let response = json!({
                                            "version": REMOTE_PROTOCOL_VERSION,
                                            "kind": "response",
                                            "id": request.id,
                                            "ok": false,
                                            "error": { "code": "unauthorized", "message": "Invalid remote request or credential" }
                                        });
                                        let _ = sink.send(Message::Text(response.to_string().into())).await;
                                        continue;
                                    }

                                    let auth_valid = if request.method == "device.pair" {
                                        let runtime = manager.runtime.read().await;
                                        runtime.pairing_token.as_deref() == Some(request.auth.as_str())
                                            && runtime.pairing_expires_at
                                                .map(|expires_at| expires_at > Utc::now())
                                                .unwrap_or(false)
                                    } else {
                                        let runtime = manager.runtime.read().await;
                                        runtime.config.devices.iter().any(|device| {
                                            !device.credential.is_empty()
                                                && request.auth == device.credential
                                        }) || (!runtime.config.device_credential.is_empty()
                                            && request.auth == runtime.config.device_credential)
                                    };
                                    if !auth_valid {
                                        let response = json!({
                                            "version": REMOTE_PROTOCOL_VERSION,
                                            "kind": "response",
                                            "id": request.id,
                                            "ok": false,
                                            "error": { "code": "unauthorized", "message": "Remote credential is invalid or expired" }
                                        });
                                        let _ = sink.send(Message::Text(response.to_string().into())).await;
                                        continue;
                                    }

                                    /*
                                    // 旧版按请求即时推导 deviceId，仅供 thread.subscribe 使用；
                                    // 现在设备在线集合只在 device.identify 成功后注册。
                                    let device_id = {
                                        let runtime = manager.runtime.read().await;
                                        runtime.config.devices.iter()
                                            .find(|device| device.credential == request.auth)
                                            .map(|device| device.id.clone())
                                            .unwrap_or_else(|| "legacy".to_string())
                                    };
                                    */

                                    let result: Result<Value, String> = match request.method.as_str() {
                                        "device.pair" => {
                                            let device_credential = format!(
                                                "{}{}",
                                                uuid::Uuid::new_v4().simple(),
                                                uuid::Uuid::new_v4().simple()
                                            );
                                            let now = Utc::now().to_rfc3339();
                                            let device = RemoteDeviceConfig {
                                                id: format!("mobile_{}", uuid::Uuid::new_v4().simple()),
                                                name: device_name_from_payload(&request.payload),
                                                credential: device_credential.clone(),
                                                paired_at: now.clone(),
                                                last_connected_at: now,
                                            };
                                            let device_to_save = device.clone();
                                            let _write_guard = state.config_write_lock.lock().await;
                                            let saved = tokio::task::spawn_blocking(move || {
                                                AppConfig::mutate(|app_config| {
                                                    app_config.remote_access.devices.push(device_to_save);
                                                    Ok(())
                                                })
                                            })
                                            .await
                                            .map_err(|error| error.to_string())
                                            .and_then(|result| result.map_err(|error| error.to_string()));
                                            match saved {
                                                Ok(()) => {
                                                    let mut runtime = manager.runtime.write().await;
                                                    runtime.config.devices.push(device.clone());
                                                    runtime.pairing_token = None;
                                                    runtime.pairing_expires_at = None;
                                                    drop(runtime);
                                                    // 配对成功即视为该设备在线，避免首次配对后等待额外 identify 才能收到完成事件。
                                                    manager.online_devices.write().await.insert(device.id.clone());
                                                    let _ = app.emit("remote-access-updated", manager.status().await);
                                                    Ok(json!({
                                                        "device_credential": device_credential,
                                                        "device_id": device.id,
                                                    }))
                                                }
                                                Err(error) => Err(error),
                                            }
                                        }
                                        "device.identify" => {
                                            let credential = request.auth.clone();
                                            let device_name = device_name_from_payload(&request.payload);
                                            let connected_at = Utc::now().to_rfc3339();
                                            let credential_to_save = credential.clone();
                                            let name_to_save = device_name.clone();
                                            let connected_at_to_save = connected_at.clone();
                                            let _write_guard = state.config_write_lock.lock().await;
                                            let saved = tokio::task::spawn_blocking(move || {
                                                AppConfig::mutate(|app_config| {
                                                    if let Some(device) = app_config
                                                        .remote_access
                                                        .devices
                                                        .iter_mut()
                                                        .find(|device| device.credential == credential_to_save)
                                                    {
                                                        device.name = name_to_save;
                                                        device.last_connected_at = connected_at_to_save;
                                                        return Ok(Some(device.clone()));
                                                    }
                                                    if app_config.remote_access.device_credential == credential_to_save {
                                                        let device = RemoteDeviceConfig {
                                                            id: format!("mobile_{}", uuid::Uuid::new_v4().simple()),
                                                            name: name_to_save,
                                                            credential: credential_to_save,
                                                            paired_at: connected_at_to_save.clone(),
                                                            last_connected_at: connected_at_to_save,
                                                        };
                                                        app_config.remote_access.device_credential.clear();
                                                        app_config.remote_access.devices.push(device.clone());
                                                        return Ok(Some(device));
                                                    }
                                                    Ok(None)
                                                })
                                            })
                                            .await
                                            .map_err(|error| error.to_string())
                                            .and_then(|result| result.map_err(|error| error.to_string()));
                                            match saved {
                                                Ok(Some(device)) => {
                                                    let mut runtime = manager.runtime.write().await;
                                                    runtime.config.device_credential.clear();
                                                    if let Some(current) = runtime
                                                        .config
                                                        .devices
                                                        .iter_mut()
                                                        .find(|current| current.id == device.id)
                                                    {
                                                        *current = device.clone();
                                                    } else {
                                                        runtime.config.devices.push(device.clone());
                                                    }
                                                    drop(runtime);
                                                    // `device.identify` 是设备在线注册点，重连时会覆盖同一 deviceId。
                                                    manager.online_devices.write().await.insert(device.id.clone());
                                                    let _ = app.emit("remote-access-updated", manager.status().await);
                                                    Ok(json!({ "device_id": device.id }))
                                                }
                                                Ok(None) => Err("Remote device is no longer authorized".to_string()),
                                                Err(error) => Err(error),
                                            }
                                        }
                                        "desktop.get_status" => Ok(json!({
                                            "version": app.package_info().version.to_string(),
                                            "online": true,
                                        })),
                                        "engine.list" => state.engines.list_engines()
                                            .await
                                            .map_err(|error| error.to_string())
                                            .and_then(|items| serde_json::to_value(items).map_err(|error| error.to_string())),
                                        "workspace.list" => {
                                            let db = state.db.clone();
                                            tokio::task::spawn_blocking(move || crate::db::workspaces::list_workspaces(&db))
                                                .await
                                                .map_err(|error| error.to_string())
                                                .and_then(|result| result.map_err(|error| error.to_string()))
                                                .and_then(|items| serde_json::to_value(items).map_err(|error| error.to_string()))
                                        }
                                        "thread.list" => {
                                            let workspace_id = request.payload.get("workspace_id")
                                                .and_then(Value::as_str)
                                                .map(str::trim)
                                                .filter(|value| !value.is_empty())
                                                .map(str::to_string)
                                                .ok_or_else(|| "workspace_id is required".to_string());
                                            match workspace_id {
                                                Ok(workspace_id) => {
                                                    let db = state.db.clone();
                                                    tokio::task::spawn_blocking(move || crate::db::threads::list_threads_for_workspace(&db, &workspace_id))
                                                        .await
                                                        .map_err(|error| error.to_string())
                                                        .and_then(|result| result.map_err(|error| error.to_string()))
                                                        .and_then(|items| serde_json::to_value(items).map_err(|error| error.to_string()))
                                                }
                                                Err(error) => Err(error),
                                            }
                                        }
                                        "thread.create" => {
                                            let workspace_id = request.payload.get("workspace_id")
                                                .and_then(Value::as_str)
                                                .map(str::trim)
                                                .filter(|value| !value.is_empty())
                                                .map(str::to_string)
                                                .ok_or_else(|| "workspace_id is required".to_string());
                                            let engine_id = request.payload.get("engine_id")
                                                .and_then(Value::as_str)
                                                .map(str::trim)
                                                .filter(|value| !value.is_empty())
                                                .unwrap_or("codex")
                                                .to_string();
                                            let model_id = request.payload.get("model_id")
                                                .and_then(Value::as_str)
                                                .map(str::trim)
                                                .filter(|value| !value.is_empty())
                                                .unwrap_or("gpt-5.4")
                                                .to_string();
                                            let reasoning_effort = request.payload.get("reasoning_effort")
                                                .and_then(Value::as_str)
                                                .map(str::trim)
                                                .filter(|value| !value.is_empty())
                                                .map(str::to_string);
                                            let service_tier = request.payload.get("service_tier")
                                                .and_then(Value::as_str)
                                                .map(str::trim)
                                                .filter(|value| !value.is_empty())
                                                .map(str::to_string);
                                            match workspace_id {
                                                Ok(workspace_id) => create_thread_with_defaults(
                                                    &state,
                                                    workspace_id,
                                                    None,
                                                    engine_id,
                                                    model_id,
                                                    "新会话".to_string(),
                                                    reasoning_effort,
                                                    service_tier,
                                                )
                                                .await
                                                .and_then(|thread| serde_json::to_value(thread).map_err(|error| error.to_string())),
                                                Err(error) => Err(error),
                                            }
                                        }
                                        "thread.set_autonomy_preset" => {
                                            let thread_id = request.payload.get("thread_id")
                                                .and_then(Value::as_str)
                                                .map(str::trim)
                                                .filter(|value| !value.is_empty())
                                                .map(str::to_string)
                                                .ok_or_else(|| "thread_id is required".to_string());
                                            let preset = request.payload.get("preset")
                                                .and_then(Value::as_str)
                                                .map(str::trim)
                                                .filter(|value| matches!(*value, "inherit" | "read-only" | "ask" | "auto" | "full"))
                                                .map(str::to_string)
                                                .ok_or_else(|| "preset is invalid".to_string());
                                            match (thread_id, preset) {
                                                (Ok(thread_id), Ok(preset)) => {
                                                    let db = state.db.clone();
                                                    let codex_uses_external_sandbox = state.engines.codex_uses_external_sandbox().await;
                                                    tokio::task::spawn_blocking(move || {
                                                        let thread = crate::db::threads::get_thread(&db, &thread_id)?
                                                            .ok_or_else(|| anyhow::anyhow!("thread not found"))?;
                                                        let mut metadata = thread.engine_metadata.unwrap_or_else(|| json!({}));
                                                        if !metadata.is_object() { metadata = json!({}); }
                                                        let values = metadata.as_object_mut().expect("metadata must be an object");
                                                        values.remove("permissionProfile");
                                                        values.remove("sandboxApprovalPolicy");
                                                        values.remove("sandboxMode");
                                                        values.remove("sandboxAllowNetwork");
                                                        values.remove("claudePermissionMode");
                                                        values.remove("opencodePermissionMode");

                                                        if preset != "inherit" {
                                                            match thread.engine_id.as_str() {
                                                                "codex" => {
                                                                    let (approval, sandbox, allow_network) = match preset.as_str() {
                                                                        "read-only" => ("untrusted", (!codex_uses_external_sandbox).then_some("read-only"), false),
                                                                        "ask" => ("on-request", (!codex_uses_external_sandbox).then_some("workspace-write"), false),
                                                                        "auto" => ("on-request", (!codex_uses_external_sandbox).then_some("workspace-write"), true),
                                                                        "full" => ("never", Some("danger-full-access"), true),
                                                                        _ => unreachable!(),
                                                                    };
                                                                    values.insert("sandboxApprovalPolicy".to_string(), json!(approval));
                                                                    if let Some(sandbox) = sandbox {
                                                                        values.insert("sandboxMode".to_string(), json!(sandbox));
                                                                    }
                                                                    values.insert("sandboxAllowNetwork".to_string(), json!(allow_network));
                                                                }
                                                                "claude" => {
                                                                    let (permission_mode, sandbox, allow_network) = match preset.as_str() {
                                                                        "read-only" => ("default", "read-only", false),
                                                                        "ask" => ("default", "workspace-write", false),
                                                                        "auto" => ("acceptEdits", "workspace-write", true),
                                                                        "full" => ("bypassPermissions", "danger-full-access", true),
                                                                        _ => unreachable!(),
                                                                    };
                                                                    values.insert("claudePermissionMode".to_string(), json!(permission_mode));
                                                                    values.insert("sandboxMode".to_string(), json!(sandbox));
                                                                    values.insert("sandboxAllowNetwork".to_string(), json!(allow_network));
                                                                }
                                                                "opencode" => {
                                                                    let permission_mode = match preset.as_str() {
                                                                        "read-only" | "ask" => "default",
                                                                        "auto" | "full" => "bypassPermissions",
                                                                        _ => unreachable!(),
                                                                    };
                                                                    values.insert("opencodePermissionMode".to_string(), json!(permission_mode));
                                                                }
                                                                _ => return Err(anyhow::anyhow!("unsupported engine")),
                                                            }
                                                        }

                                                        crate::db::threads::update_engine_metadata(&db, &thread_id, &metadata)?;
                                                        crate::db::threads::get_thread(&db, &thread_id)?
                                                            .ok_or_else(|| anyhow::anyhow!("thread not found after update"))
                                                    })
                                                    .await
                                                    .map_err(|error| error.to_string())
                                                    .and_then(|result| result.map_err(|error| error.to_string()))
                                                    .and_then(|thread| serde_json::to_value(thread).map_err(|error| error.to_string()))
                                                }
                                                (Err(error), _) | (_, Err(error)) => Err(error),
                                            }
                                        }
                                        "message.list" => {
                                            let thread_id = request.payload.get("thread_id")
                                                .and_then(Value::as_str)
                                                .map(str::trim)
                                                .filter(|value| !value.is_empty())
                                                .map(str::to_string)
                                                .ok_or_else(|| "thread_id is required".to_string());
                                            match thread_id {
                                                Ok(thread_id) => {
                                                    // 历史分页实现已停用：手机首次打开会话时直接取得全部消息。
                                                    /*
                                                    let cursor = request.payload.get("cursor")
                                                        .filter(|value| !value.is_null())
                                                        .cloned()
                                                        .map(serde_json::from_value::<MessageWindowCursorDto>)
                                                        .transpose()
                                                        .map_err(|error| error.to_string());
                                                    */
                                                    let db = state.db.clone();
                                                    let query_thread_id = thread_id.clone();
                                                    let messages = tokio::task::spawn_blocking(move || {
                                                        crate::db::messages::get_thread_messages(&db, &query_thread_id)
                                                    })
                                                    .await
                                                    .map_err(|error| error.to_string())
                                                    .and_then(|result| result.map_err(|error| error.to_string()));
                                                    match messages {
                                                        Ok(messages) => {
                                                            // 历史查询不参与实时消息投递，直接返回消息列表。
                                                            /*
                                                            let versions = messages.iter()
                                                                .map(|message| serde_json::to_string(message)
                                                                    .map(|value| (message.id.clone(), value)))
                                                                .collect::<Result<HashMap<String, String>, _>>()
                                                                .map_err(|error| error.to_string());
                                                            match versions {
                                                                Ok(versions) => {
                                                                    message_versions.insert(thread_id, versions);
                                                                    Ok(json!({ "messages": messages }))
                                                                }
                                                                Err(error) => Err(error),
                                                            }
                                                            */
                                                            Ok(json!({ "messages": messages }))
                                                        }
                                                        Err(error) => Err(error),
                                                    }
                                                }
                                                Err(error) => Err(error),
                                            }
                                        }
                                        "attachment.upload" => {
                                            let upload_id = request.payload.get("upload_id")
                                                .and_then(Value::as_str)
                                                .map(str::trim)
                                                .filter(|value| !value.is_empty() && value.len() <= 128)
                                                .map(str::to_string)
                                                .ok_or_else(|| "upload_id is required".to_string());
                                            let file_name = request.payload.get("file_name")
                                                .and_then(Value::as_str)
                                                .map(str::trim)
                                                .filter(|value| !value.is_empty() && value.len() <= 255)
                                                .map(str::to_string)
                                                .ok_or_else(|| "file_name is required".to_string());
                                            let mime_type = request.payload.get("mime_type")
                                                .and_then(Value::as_str)
                                                .map(str::trim)
                                                .filter(|value| {
                                                    value.len() <= 128
                                                        && value.split_once('/').is_some_and(|(kind, subtype)| !kind.is_empty() && !subtype.is_empty())
                                                        && value.chars().all(|character| character.is_ascii_alphanumeric() || matches!(character, '/' | '.' | '+' | '-'))
                                                })
                                                .map(str::to_string)
                                                .ok_or_else(|| "attachment mime_type is invalid".to_string());
                                            let chunk_index = request.payload.get("chunk_index")
                                                .and_then(Value::as_u64)
                                                .filter(|value| *value <= u32::MAX as u64)
                                                .map(|value| value as u32)
                                                .ok_or_else(|| "chunk_index is required".to_string());
                                            let chunk_count = request.payload.get("chunk_count")
                                                .and_then(Value::as_u64)
                                                .filter(|value| *value > 0 && *value <= 64)
                                                .map(|value| value as u32)
                                                .ok_or_else(|| "chunk_count is invalid".to_string());
                                            let data_base64 = request.payload.get("data_base64")
                                                .and_then(Value::as_str)
                                                .filter(|value| !value.is_empty() && value.len() <= 300_000)
                                                .map(str::to_string)
                                                .ok_or_else(|| "attachment chunk is invalid".to_string());
                                            match (upload_id, file_name, mime_type, chunk_index, chunk_count, data_base64) {
                                                (Ok(upload_id), Ok(file_name), Ok(mime_type), Ok(chunk_index), Ok(chunk_count), Ok(data_base64)) => {
                                                    let mut uploads = manager.uploads.lock().await;
                                                    let completed = (|| -> Result<Option<RemoteUploadState>, String> {
                                                        if chunk_index == 0 {
                                                            uploads.insert(upload_id.clone(), RemoteUploadState {
                                                                file_name: file_name.clone(),
                                                                mime_type: mime_type.clone(),
                                                                chunk_count,
                                                                next_chunk: 0,
                                                                data_base64: String::new(),
                                                            });
                                                        }
                                                        let upload = uploads.get_mut(&upload_id)
                                                            .ok_or_else(|| "attachment upload was not initialized".to_string())?;
                                                        if upload.file_name != file_name
                                                            || upload.mime_type != mime_type
                                                            || upload.chunk_count != chunk_count
                                                            || upload.next_chunk != chunk_index
                                                        {
                                                            return Err("attachment chunks are out of sequence".to_string());
                                                        }
                                                        upload.data_base64.push_str(&data_base64);
                                                        if upload.data_base64.len() > 14_000_000 {
                                                            uploads.remove(&upload_id);
                                                            return Err("attachment exceeds the 10 MB limit".to_string());
                                                        }
                                                        upload.next_chunk += 1;
                                                        if upload.next_chunk == upload.chunk_count {
                                                            Ok(uploads.remove(&upload_id))
                                                        } else {
                                                            Ok(None)
                                                        }
                                                    })();
                                                    drop(uploads);
                                                    match completed {
                                                        Ok(Some(upload)) => {
                                                            let original_file_name = upload.file_name.clone();
                                                            if upload.mime_type.starts_with("image/") {
                                                                save_pasted_image_attachment(
                                                                    upload.file_name,
                                                                    upload.mime_type,
                                                                    upload.data_base64,
                                                                )
                                                                .await
                                                                .and_then(|mut attachment| {
                                                                    attachment.file_name = original_file_name;
                                                                    serde_json::to_value(attachment).map_err(|error| error.to_string())
                                                                })
                                                            } else {
                                                                let decoded = BASE64
                                                                    .decode(upload.data_base64.trim())
                                                                    .map_err(|_| "attachment data is not valid base64".to_string());
                                                                match decoded {
                                                                    Ok(bytes) if bytes.is_empty() => Err("attachment data is empty".to_string()),
                                                                    Ok(bytes) if bytes.len() > 10 * 1024 * 1024 => Err("attachment exceeds the 10 MB limit".to_string()),
                                                                    Ok(bytes) => {
                                                                        let extension = Path::new(&original_file_name)
                                                                            .extension()
                                                                            .and_then(|value| value.to_str())
                                                                            .filter(|value| !value.is_empty() && value.len() <= 24 && value.chars().all(|character| character.is_ascii_alphanumeric()))
                                                                            .map(|value| format!(".{value}"))
                                                                            .unwrap_or_default();
                                                                        let stored_file_name = format!("mobile-file-{}{}", Uuid::new_v4().simple(), extension);
                                                                        let attachment_dir = crate::runtime_env::app_data_dir()
                                                                            .join("attachments")
                                                                            .join("mobile-files");
                                                                        match tokio_fs::create_dir_all(&attachment_dir).await {
                                                                            Ok(()) => {
                                                                                let file_path = attachment_dir.join(stored_file_name);
                                                                                match tokio_fs::write(&file_path, &bytes).await {
                                                                                    Ok(()) => serde_json::to_value(ChatAttachmentPayload {
                                                                                        file_name: original_file_name,
                                                                                        file_path: file_path.display().to_string(),
                                                                                        size_bytes: bytes.len() as u64,
                                                                                        mime_type: Some(upload.mime_type),
                                                                                        browser_annotation: None,
                                                                                    }).map_err(|error| error.to_string()),
                                                                                    Err(error) => Err(format!("failed to save mobile attachment: {error}")),
                                                                                }
                                                                            }
                                                                            Err(error) => Err(format!("failed to create mobile attachment directory: {error}")),
                                                                        }
                                                                    }
                                                                    Err(error) => Err(error),
                                                                }
                                                            }
                                                        }
                                                        Ok(None) => Ok(json!({ "complete": false })),
                                                        Err(error) => Err(error),
                                                    }
                                                }
                                                (Err(error), _, _, _, _, _)
                                                | (_, Err(error), _, _, _, _)
                                                | (_, _, Err(error), _, _, _)
                                                | (_, _, _, Err(error), _, _)
                                                | (_, _, _, _, Err(error), _)
                                                | (_, _, _, _, _, Err(error)) => Err(error),
                                            }
                                        }
                                        "message.send" => {
                                            let thread_id = request.payload.get("thread_id")
                                                .and_then(Value::as_str)
                                                .map(str::trim)
                                                .filter(|value| !value.is_empty())
                                                .map(str::to_string)
                                                .ok_or_else(|| "thread_id is required".to_string());
                                            let message = request.payload.get("message")
                                                .and_then(Value::as_str)
                                                .map(str::trim)
                                                .unwrap_or_default()
                                                .to_string();
                                            let model_id = request.payload.get("model_id")
                                                .and_then(Value::as_str)
                                                .map(str::trim)
                                                .filter(|value| !value.is_empty())
                                                .map(str::to_string);
                                            let reasoning_effort = request.payload.get("reasoning_effort")
                                                .and_then(Value::as_str)
                                                .map(str::trim)
                                                .filter(|value| !value.is_empty())
                                                .map(str::to_string);
                                            let attachments = request.payload.get("attachments")
                                                .filter(|value| !value.is_null())
                                                .cloned()
                                                .map(serde_json::from_value::<Vec<ChatAttachmentPayload>>)
                                                .transpose()
                                                .map_err(|error| error.to_string());
                                            match (thread_id, attachments) {
                                                (Ok(thread_id), Ok(attachments))
                                                    if message.len() <= 100_000
                                                        && (!message.is_empty()
                                                            || !attachments.as_ref().map(|items| items.is_empty()).unwrap_or(true)) => {
                                                    send_message_inner(
                                                        app.clone(),
                                                        &state,
                                                        thread_id,
                                                        message,
                                                        model_id,
                                                        reasoning_effort,
                                                        attachments,
                                                        None,
                                                        Some(false),
                                                        Some(request.id.clone()),
                                                        None,
                                                    )
                                                    .await
                                                    .map(|assistant_message_id| json!({ "assistant_message_id": assistant_message_id }))
                                                }
                                                (Ok(_), Ok(_)) if message.len() > 100_000 => Err("message is too large".to_string()),
                                                (Ok(_), Ok(_)) => Err("message or attachment is required".to_string()),
                                                (Err(error), _) | (_, Err(error)) => Err(error),
                                            }
                                        }
                                        "turn.stop" => {
                                            let thread_id = request.payload.get("thread_id")
                                                .and_then(Value::as_str)
                                                .map(str::trim)
                                                .filter(|value| !value.is_empty())
                                                .map(str::to_string)
                                                .ok_or_else(|| "thread_id is required".to_string());
                                            match thread_id {
                                                Ok(thread_id) => cancel_turn_inner(&state, thread_id).await.map(|_| json!({})),
                                                Err(error) => Err(error),
                                            }
                                        }
                                        "thread.subscribe" => {
                                            // 兼容旧客户端，但不再按 threadId 控制完成消息投递。
                                            /*
                                            let thread_id = request.payload.get("thread_id")
                                                .and_then(Value::as_str)
                                                .map(str::trim)
                                                .filter(|value| !value.is_empty())
                                                .map(str::to_string)
                                                .ok_or_else(|| "thread_id is required".to_string());
                                            match thread_id {
                                                Ok(thread_id) => {
                                                    // 先记录当前完整会话的版本。这样订阅与首次取数之间若有新消息，
                                                    // 后续只会推送那一条新消息，不会把整段历史再次当作实时消息发送。
                                                    let db = state.db.clone();
                                                    let query_thread_id = thread_id.clone();
                                                    let messages = tokio::task::spawn_blocking(move || {
                                                        crate::db::messages::get_thread_messages(&db, &query_thread_id)
                                                    })
                                                    .await
                                                    .map_err(|error| error.to_string())
                                                    .and_then(|result| result.map_err(|error| error.to_string()));
                                                    match messages {
                                                        Ok(messages) => {
                                                            let versions = messages.into_iter()
                                                                .map(|message| serde_json::to_string(&message)
                                                                    .map(|value| (message.id, value)))
                                                                .collect::<Result<HashMap<String, String>, _>>()
                                                                .map_err(|error| error.to_string());
                                                            match versions {
                                                                Ok(versions) => {
                                                                    manager.subscriptions.write().await
                                                                        .entry(thread_id.clone())
                                                                        .or_default()
                                                                        .insert(device_id.clone());
                                                                    message_versions.insert(thread_id, versions);
                                                                    Ok(json!({}))
                                                                }
                                                                Err(error) => Err(error),
                                                            }
                                                        }
                                                        Err(error) => Err(error),
                                                    }
                                                }
                                                Err(error) => Err(error),
                                            }
                                            */
                                            Ok(json!({ "deprecated": true }))
                                        }
                                        "thread.unsubscribe" => {
                                            // 离开会话不能取消设备注册；设备在线状态只由 identify/隧道离线维护。
                                            /*
                                            let thread_id = request.payload.get("thread_id")
                                                .and_then(Value::as_str)
                                                .map(str::trim)
                                                .filter(|value| !value.is_empty())
                                                .map(str::to_string)
                                                .ok_or_else(|| "thread_id is required".to_string());
                                            match thread_id {
                                                Ok(thread_id) => {
                                                    let mut subscriptions = manager.subscriptions.write().await;
                                                    if let Some(device_ids) = subscriptions.get_mut(&thread_id) {
                                                        device_ids.remove(&device_id);
                                                        if device_ids.is_empty() {
                                                            subscriptions.remove(&thread_id);
                                                            message_versions.remove(&thread_id);
                                                        }
                                                    }
                                                    Ok(json!({}))
                                                }
                                                Err(error) => Err(error),
                                            }
                                            */
                                            Ok(json!({ "deprecated": true }))
                                        }
                                        _ => Err(format!("unknown remote method: {}", request.method)),
                                    };

                                    let response = match result {
                                        Ok(payload) => json!({
                                            "version": REMOTE_PROTOCOL_VERSION,
                                            "kind": "response",
                                            "id": request.id,
                                            "ok": true,
                                            "payload": payload,
                                        }),
                                        Err(error) => json!({
                                            "version": REMOTE_PROTOCOL_VERSION,
                                            "kind": "response",
                                            "id": request.id,
                                            "ok": false,
                                            "error": { "code": "request_failed", "message": error },
                                        }),
                                    };
                                    if let Err(error) = sink.send(Message::Text(response.to_string().into())).await {
                                        disconnected_error = Some(error.to_string());
                                        break;
                                    }
                                }
                                _ => {}
                            }
                        }
                        /*
                        Some(changed_thread_id) = thread_updated_rx.recv() => {
                            let device_ids = manager.subscriptions.read().await
                                .get(&changed_thread_id)
                                .cloned()
                                .unwrap_or_default();
                            if device_ids.is_empty() {
                                continue;
                            }
                            let db = state.db.clone();
                            let query_thread_id = changed_thread_id.clone();
                            let messages = tokio::task::spawn_blocking(move || {
                                crate::db::messages::get_thread_messages(&db, &query_thread_id)
                            })
                            .await
                            .ok()
                            .and_then(Result::ok);
                            let Some(messages) = messages else { continue; };
                            let changed_messages = {
                                let versions = message_versions.entry(changed_thread_id.clone()).or_default();
                                messages.into_iter().filter_map(|message| {
                                    let serialized = serde_json::to_string(&message).ok()?;
                                    if versions.get(&message.id) == Some(&serialized) {
                                        return None;
                                    }
                                    versions.insert(message.id.clone(), serialized);
                                    serde_json::to_value(message).ok()
                                }).collect::<Vec<_>>()
                            };
                            for message in changed_messages {
                                for device_id in &device_ids {
                                    let event = json!({
                                        "version": REMOTE_PROTOCOL_VERSION,
                                        "kind": "event",
                                        "event": "thread.message",
                                        "targetDeviceId": device_id,
                                        "payload": { "message": message },
                                    });
                                    if let Err(error) = sink.send(Message::Text(event.to_string().into())).await {
                                        disconnected_error = Some(error.to_string());
                                        break;
                                    }
                                }
                                if disconnected_error.is_some() { break; }
                            }
                            if disconnected_error.is_some() { break; }
                        }
                        */
                        Some(completed_payload) = completed_message_rx.recv() => {
                            let device_ids = manager.online_devices.read().await.clone();
                            if device_ids.is_empty() {
                                continue;
                            }
                            for device_id in device_ids {
                                let Some(event) = build_completed_message_event(
                                    &device_id,
                                    &completed_payload,
                                ) else {
                                    continue;
                                };
                                if let Err(error) = sink.send(Message::Text(event.to_string().into())).await {
                                    disconnected_error = Some(error.to_string());
                                    break;
                                }
                            }
                            if disconnected_error.is_some() { break; }
                        }
                        /*
                        _ = snapshot_interval.tick() => {
                            let thread_ids: Vec<String> = manager.subscriptions.read().await.iter().cloned().collect();
                            for thread_id in thread_ids {
                                let db = state.db.clone();
                                let query_thread_id = thread_id.clone();
                                let snapshot = tokio::task::spawn_blocking(move || {
                                    let thread = crate::db::threads::get_thread(&db, &query_thread_id)?;
                                    let messages = crate::db::messages::get_thread_messages_window(
                                        &db,
                                        &query_thread_id,
                                        None,
                                        MESSAGE_WINDOW_MAX_LIMIT,
                                    )?;
                                    Ok::<Value, anyhow::Error>(json!({ "thread": thread, "messages": messages }))
                                })
                                .await
                                .ok()
                                .and_then(Result::ok);
                                let Some(snapshot) = snapshot else { continue; };
                                let serialized = snapshot.to_string();
                                if snapshots.get(&thread_id) == Some(&serialized) {
                                    continue;
                                }
                                snapshots.insert(thread_id.clone(), serialized);
                                let event = json!({
                                    "version": REMOTE_PROTOCOL_VERSION,
                                    "kind": "event",
                                    "event": "thread.snapshot",
                                    "payload": snapshot,
                                });
                                if let Err(error) = sink.send(Message::Text(event.to_string().into())).await {
                                    disconnected_error = Some(error.to_string());
                                    break;
                                }
                            }
                            if disconnected_error.is_some() { break; }
                        }
                        */
                    }
                }

                app.unlisten(completed_message_listener);

                {
                    let mut runtime = manager.runtime.write().await;
                    if runtime.generation != generation {
                        break;
                    }
                    runtime.connected = false;
                    runtime.peer_online = false;
                    runtime.last_error = disconnected_error;
                }
                manager.online_devices.write().await.clear();
                let _ = app.emit("remote-access-updated", manager.status().await);

                if cancellation.is_cancelled() {
                    break;
                }
                tokio::select! {
                    _ = cancellation.cancelled() => break,
                    _ = tokio::time::sleep(Duration::from_secs(1)) => {}
                }
            }
        });
    }

    pub async fn shutdown(&self) {
        if let Some(cancellation) = self.cancellation.lock().await.take() {
            cancellation.cancel();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::build_completed_message_event;
    use crate::config::app_config::RemoteAccessConfig;
    use serde_json::json;

    #[test]
    fn remote_identity_is_generated_and_rotates() {
        let mut config = RemoteAccessConfig::default();
        assert!(config.ensure_identity());
        assert!(config.tunnel_id.starts_with("panes_"));
        assert!(config.credential.len() >= 64);
        let first_id = config.tunnel_id.clone();
        assert!(!config.ensure_identity());
        config.device_credential = "paired-device".to_string();
        config.regenerate_identity();
        assert_ne!(config.tunnel_id, first_id);
        assert!(config.device_credential.is_empty());
    }

    #[test]
    fn completed_message_event_targets_device_and_preserves_exact_message() {
        let payload = json!({
            "threadId": "thread-1",
            "messageId": "message-1",
            "message": {
                "id": "message-1",
                "threadId": "thread-1",
                "role": "assistant",
                "status": "completed",
                "content": "done",
            },
        });

        let event = build_completed_message_event("mobile-1", &payload)
            .expect("valid completed message payload");
        assert_eq!(event["event"], "thread.message.completed");
        assert_eq!(event["targetDeviceId"], "mobile-1");
        assert_eq!(event["payload"]["threadId"], "thread-1");
        assert_eq!(event["payload"]["messageId"], "message-1");
        assert_eq!(event["payload"]["message"]["id"], "message-1");
    }

    #[test]
    fn completed_message_event_rejects_missing_exact_ids() {
        let payload = json!({
            "threadId": "thread-1",
            "message": { "id": "message-1" },
        });
        assert!(build_completed_message_event("mobile-1", &payload).is_none());
    }
}
