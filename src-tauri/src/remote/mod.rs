use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use futures::{SinkExt, StreamExt};
use qrcode::{render::svg, QrCode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};
use tokio::sync::{Mutex, RwLock};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tokio_util::sync::CancellationToken;

use crate::{
    commands::{
        chat::{
            cancel_turn_inner, save_pasted_image_attachment, send_message_inner,
            ChatAttachmentPayload,
        },
        threads::create_thread_with_defaults,
    },
    config::app_config::{AppConfig, RemoteAccessConfig, RemoteDeviceConfig},
    models::MessageWindowCursorDto,
    state::AppState,
};

const REMOTE_PROTOCOL_VERSION: u32 = 1;
const MESSAGE_WINDOW_DEFAULT_LIMIT: usize = 50;
const MESSAGE_WINDOW_MAX_LIMIT: usize = 100;

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
    subscriptions: RwLock<HashSet<String>>,
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
        self.subscriptions.write().await.clear();
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

                let mut snapshots: HashMap<String, String> = HashMap::new();
                let mut snapshot_interval = tokio::time::interval(Duration::from_millis(650));
                snapshot_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
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
                                            manager.subscriptions.write().await.clear();
                                            snapshots.clear();
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
                                        "message.list" => {
                                            let thread_id = request.payload.get("thread_id")
                                                .and_then(Value::as_str)
                                                .map(str::trim)
                                                .filter(|value| !value.is_empty())
                                                .map(str::to_string)
                                                .ok_or_else(|| "thread_id is required".to_string());
                                            match thread_id {
                                                Ok(thread_id) => {
                                                    let cursor = request.payload.get("cursor")
                                                        .filter(|value| !value.is_null())
                                                        .cloned()
                                                        .map(serde_json::from_value::<MessageWindowCursorDto>)
                                                        .transpose()
                                                        .map_err(|error| error.to_string());
                                                    match cursor {
                                                        Ok(cursor) => {
                                                            let limit = request.payload.get("limit")
                                                                .and_then(Value::as_u64)
                                                                .map(|value| value as usize)
                                                                .unwrap_or(MESSAGE_WINDOW_DEFAULT_LIMIT)
                                                                .clamp(1, MESSAGE_WINDOW_MAX_LIMIT);
                                                            let db = state.db.clone();
                                                            tokio::task::spawn_blocking(move || {
                                                                crate::db::messages::get_thread_messages_window(&db, &thread_id, cursor.as_ref(), limit)
                                                            })
                                                            .await
                                                            .map_err(|error| error.to_string())
                                                            .and_then(|result| result.map_err(|error| error.to_string()))
                                                            .and_then(|window| serde_json::to_value(window).map_err(|error| error.to_string()))
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
                                                .filter(|value| value.starts_with("image/") && value.len() <= 128)
                                                .map(str::to_string)
                                                .ok_or_else(|| "image mime_type is required".to_string());
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
                                                        None,
                                                        None,
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
                                            let thread_id = request.payload.get("thread_id")
                                                .and_then(Value::as_str)
                                                .map(str::trim)
                                                .filter(|value| !value.is_empty())
                                                .map(str::to_string)
                                                .ok_or_else(|| "thread_id is required".to_string());
                                            match thread_id {
                                                Ok(thread_id) => {
                                                    manager.subscriptions.write().await.insert(thread_id);
                                                    Ok(json!({}))
                                                }
                                                Err(error) => Err(error),
                                            }
                                        }
                                        "thread.unsubscribe" => {
                                            let thread_id = request.payload.get("thread_id")
                                                .and_then(Value::as_str)
                                                .map(str::trim)
                                                .filter(|value| !value.is_empty())
                                                .map(str::to_string)
                                                .ok_or_else(|| "thread_id is required".to_string());
                                            match thread_id {
                                                Ok(thread_id) => {
                                                    manager.subscriptions.write().await.remove(&thread_id);
                                                    snapshots.remove(&thread_id);
                                                    Ok(json!({}))
                                                }
                                                Err(error) => Err(error),
                                            }
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
                    }
                }

                {
                    let mut runtime = manager.runtime.write().await;
                    if runtime.generation != generation {
                        break;
                    }
                    runtime.connected = false;
                    runtime.peer_online = false;
                    runtime.last_error = disconnected_error;
                }
                manager.subscriptions.write().await.clear();
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
    use crate::config::app_config::RemoteAccessConfig;

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
}
