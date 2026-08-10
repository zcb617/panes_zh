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
    commands::chat::{cancel_turn_inner, send_message_inner},
    config::app_config::{AppConfig, RemoteAccessConfig},
    models::MessageWindowCursorDto,
    state::AppState,
};

const REMOTE_PROTOCOL_VERSION: u32 = 1;
const MESSAGE_WINDOW_DEFAULT_LIMIT: usize = 50;
const MESSAGE_WINDOW_MAX_LIMIT: usize = 100;

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
            paired: !runtime.config.device_credential.is_empty(),
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

        let generation = {
            let mut runtime = self.runtime.write().await;
            runtime.generation = runtime.generation.wrapping_add(1);
            runtime.config = config.clone();
            runtime.connected = false;
            runtime.peer_online = false;
            runtime.last_error = None;
            if config.enabled {
                runtime.pairing_token = Some(format!(
                    "{}{}",
                    uuid::Uuid::new_v4().simple(),
                    uuid::Uuid::new_v4().simple()
                ));
                runtime.pairing_expires_at = Some(Utc::now() + ChronoDuration::minutes(5));
            } else {
                runtime.pairing_token = None;
                runtime.pairing_expires_at = None;
            }
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
                                        !runtime.config.device_credential.is_empty()
                                            && request.auth == runtime.config.device_credential
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
                                            let credential_to_save = device_credential.clone();
                                            let _write_guard = state.config_write_lock.lock().await;
                                            let saved = tokio::task::spawn_blocking(move || {
                                                AppConfig::mutate(|app_config| {
                                                    app_config.remote_access.device_credential = credential_to_save;
                                                    Ok(())
                                                })
                                            })
                                            .await
                                            .map_err(|error| error.to_string())
                                            .and_then(|result| result.map_err(|error| error.to_string()));
                                            match saved {
                                                Ok(()) => {
                                                    let mut runtime = manager.runtime.write().await;
                                                    runtime.config.device_credential = device_credential.clone();
                                                    runtime.pairing_token = None;
                                                    runtime.pairing_expires_at = None;
                                                    drop(runtime);
                                                    let _ = app.emit("remote-access-updated", manager.status().await);
                                                    Ok(json!({ "device_credential": device_credential }))
                                                }
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
                                                .filter(|value| !value.is_empty())
                                                .map(str::to_string)
                                                .ok_or_else(|| "message is required".to_string());
                                            match (thread_id, message) {
                                                (Ok(thread_id), Ok(message)) if message.len() <= 100_000 => {
                                                    send_message_inner(
                                                        app.clone(),
                                                        &state,
                                                        thread_id,
                                                        message,
                                                        None,
                                                        None,
                                                        None,
                                                        None,
                                                        Some(false),
                                                        Some(request.id.clone()),
                                                        None,
                                                    )
                                                    .await
                                                    .map(|assistant_message_id| json!({ "assistant_message_id": assistant_message_id }))
                                                }
                                                (Ok(_), Ok(_)) => Err("message is too large".to_string()),
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
