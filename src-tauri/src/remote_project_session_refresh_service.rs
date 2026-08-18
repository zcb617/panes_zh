//! SSH 远端项目会话同步服务。

use std::{
    collections::HashSet,
    sync::{Arc, LazyLock, Mutex},
    time::Duration,
};

use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tauri::{AppHandle, Manager};
use tokio_tungstenite::{connect_async, tungstenite::Message};

use crate::{
    cli_tools::{factory::CliToolFactory, CliExecutionContext, CliTool},
    db::{threads, workspaces, Database},
    message_notify_helper::{
        notify_ssh_remote_project_sessions_refreshed, SshRemoteProjectSessionsRefreshedEvent,
    },
    models::{ThreadStatusDto, WorkspaceDto},
    path_utils,
    ssh::{
        cli_service_lifecycle,
        cli_tunnel_registry::{self, SshCliTunnel},
    },
    state::AppState,
};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_PAGES: usize = 50;

/// 单个工作区的同步报告。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SshRemoteProjectSessionRefreshReport {
    /// 工作区唯一标识。
    pub workspace_id: String,
    /// 成功同步的 CLI。
    pub succeeded_cli_ids: Vec<String>,
    /// 失败的 CLI。
    pub failed_cli_ids: Vec<String>,
}

static REFRESHING_WORKSPACES: LazyLock<Mutex<HashSet<String>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

struct WorkspaceRefreshGuard {
    /// 当前互斥锁占用的工作区标识。
    workspace_id: String,
}

impl WorkspaceRefreshGuard {
    fn acquire(id: &str) -> Option<Self> {
        let mut set = REFRESHING_WORKSPACES
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        set.insert(id.to_string()).then(|| Self {
            workspace_id: id.to_string(),
        })
    }
}

impl Drop for WorkspaceRefreshGuard {
    fn drop(&mut self) {
        let mut set = REFRESHING_WORKSPACES
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        set.remove(&self.workspace_id);
    }
}

/// 按数据库顺序刷新全部已启用 SSH 工作区。
pub async fn refresh_all_ssh_remote_project_sessions(
    app: &AppHandle,
    db: Arc<Database>,
) -> Vec<SshRemoteProjectSessionRefreshReport> {
    let records = match tokio::task::spawn_blocking({
        let db = db.clone();
        move || workspaces::list_workspaces(db.as_ref())
    })
    .await
    {
        Ok(Ok(records)) => records,
        Ok(Err(error)) => {
            log::warn!("读取 SSH 工作区失败: {error:#}");
            return Vec::new();
        }
        Err(error) => {
            log::warn!("读取 SSH 工作区任务失败: {error:#}");
            return Vec::new();
        }
    };
    let mut reports = Vec::new();
    for workspace in records.into_iter().filter(is_enabled_ssh_workspace) {
        match refresh_ssh_remote_project_sessions(app, db.clone(), &workspace.id).await {
            Ok(report) => reports.push(report),
            Err(error) => log::warn!(
                "刷新 SSH 工作区失败: workspace_id={} error={error:#}",
                workspace.id
            ),
        }
    }
    reports
}

/// 刷新一个 SSH 工作区；远端缺失的本地线程不会被删除。
pub async fn refresh_ssh_remote_project_sessions(
    app: &AppHandle,
    db: Arc<Database>,
    workspace_id: &str,
) -> Result<SshRemoteProjectSessionRefreshReport> {
    let _guard = WorkspaceRefreshGuard::acquire(workspace_id)
        .ok_or_else(|| anyhow::anyhow!("workspace refresh already in progress: {workspace_id}"))?;
    let workspace = load_workspace(db.clone(), workspace_id).await?;
    let connection_id = workspace
        .ssh_connection_id
        .as_deref()
        .context("SSH workspace has no connection id")?
        .to_string();
    let tunnels = cli_tunnel_registry::list_by_host(&connection_id).await;
    let mut cli_ids = tunnels.keys().cloned().collect::<Vec<_>>();
    cli_ids.sort();
    let mut report = SshRemoteProjectSessionRefreshReport {
        workspace_id: workspace_id.to_string(),
        succeeded_cli_ids: Vec::new(),
        failed_cli_ids: Vec::new(),
    };
    for cli_id in cli_ids {
        match cli_id.as_str() {
            "codex" | "opencode" | "claude" => {
                match sync_cli(app, &workspace, &connection_id, &cli_id, db.clone()).await {
                    Ok(()) => report.succeeded_cli_ids.push(cli_id),
                    Err(error) => {
                        log::warn!(
                            "同步 SSH 远端 CLI 失败: workspace_id={} cli_id={} error={error:#}",
                            workspace_id,
                            cli_id
                        );
                        report.failed_cli_ids.push(cli_id);
                    }
                }
            }
            other => log::debug!("跳过未支持 SSH 远端 CLI: {other}"),
        }
    }
    if let Err(error) = notify_ssh_remote_project_sessions_refreshed(
        app,
        SshRemoteProjectSessionsRefreshedEvent {
            workspace_id: report.workspace_id.clone(),
            succeeded_cli_ids: report.succeeded_cli_ids.clone(),
            failed_cli_ids: report.failed_cli_ids.clone(),
        },
    ) {
        log::warn!("SSH 会话刷新通知失败（数据已提交）: {error:#}");
    }
    Ok(report)
}

fn is_enabled_ssh_workspace(workspace: &WorkspaceDto) -> bool {
    workspace.location_kind == "ssh"
        && workspace.ssh_connection_id.is_some()
        && workspace.connection_enabled == Some(true)
        && workspace.connection_deleted != Some(true)
}

async fn load_workspace(db: Arc<Database>, workspace_id: &str) -> Result<WorkspaceDto> {
    let id = workspace_id.to_string();
    let workspace =
        tokio::task::spawn_blocking(move || workspaces::find_workspace_by_id(db.as_ref(), &id))
            .await
            .context("load workspace task failed")??;
    workspace
        .filter(is_enabled_ssh_workspace)
        .ok_or_else(|| anyhow::anyhow!("enabled SSH workspace not found: {workspace_id}"))
}

async fn sync_cli(
    app: &AppHandle,
    workspace: &WorkspaceDto,
    connection_id: &str,
    cli_id: &str,
    db: Arc<Database>,
) -> Result<()> {
    // 会话扫描前先登记常驻服务。应用启动时首次建立服务并写入 Map，后续刷新复用
    // 已登记的服务；CLI 实现仍走各自的读取逻辑，但服务不会因一次扫描结束而关闭。
    cli_service_lifecycle::set(connection_id, cli_id)
        .await
        .with_context(|| {
            format!(
                "启动并登记 SSH 远端 CLI 服务失败: connection_id={connection_id} cli_id={cli_id}"
            )
        })?;

    if cli_id == "codex" {
        let state = app.state::<AppState>();
        let context = CliExecutionContext::from_workspace(workspace)?;
        let codex = CliToolFactory::new(state.inner().clone())
            .create("codex")
            .expect("Codex CLI factory mapping must exist");
        let cli: &dyn CliTool = codex.as_ref();
        let sessions = cli.list_sessions(&context, None, Some(false)).await?;
        let sessions = sessions
            .into_iter()
            .filter(|session| path_utils::paths_equal(&session.cwd, &workspace.root_path))
            .map(|session| RemoteSessionSnapshot {
                engine_thread_id: session.engine_thread_id,
                title: session.title,
                cwd: session.cwd,
                model_id: session.model_id,
                updated_at: session.updated_at,
                status: session.status,
                metadata: session.metadata,
            })
            .collect();
        return persist_sessions(db, workspace, "codex", sessions).await;
    }

    if cli_id == "opencode" {
        let state = app.state::<AppState>();
        let context = CliExecutionContext::from_workspace(workspace)?;
        let opencode = CliToolFactory::new(state.inner().clone())
            .create("opencode")
            .expect("OpenCode CLI factory mapping must exist");
        let cli: &dyn CliTool = opencode.as_ref();
        let sessions = cli.list_sessions(&context, None, Some(false)).await?;
        let sessions = sessions
            .into_iter()
            .filter(|session| path_utils::paths_equal(&session.cwd, &workspace.root_path))
            .map(|session| RemoteSessionSnapshot {
                engine_thread_id: session.engine_thread_id,
                title: session.title,
                cwd: session.cwd,
                model_id: session.model_id,
                updated_at: session.updated_at,
                status: session.status,
                metadata: session.metadata,
            })
            .collect();
        return persist_sessions(db, workspace, "opencode", sessions).await;
    }

    if cli_id == "claude" {
        let state = app.state::<AppState>();
        let context = CliExecutionContext::from_workspace(workspace)?;
        let claude = CliToolFactory::new(state.inner().clone())
            .create("claude")
            .expect("Claude CLI factory mapping must exist");
        let cli: &dyn CliTool = claude.as_ref();
        let sessions = cli.list_sessions(&context, None, Some(false)).await?;
        let sessions = sessions
            .into_iter()
            .filter(|session| path_utils::paths_equal(&session.cwd, &workspace.root_path))
            .map(|session| RemoteSessionSnapshot {
                engine_thread_id: session.engine_thread_id,
                title: session.title,
                cwd: session.cwd,
                model_id: session.model_id,
                updated_at: session.updated_at,
                status: session.status,
                metadata: session.metadata,
            })
            .collect();
        return persist_sessions(db, workspace, "claude", sessions).await;
    }

    let tunnel = cli_tunnel_registry::acquire_temporary_service_use(connection_id, cli_id).await?;
    let result = match cli_id {
        "opencode" => unreachable!("OpenCode session sync already uses OpenCodeCli"),
        "claude" => unreachable!("Claude session sync already uses ClaudeCodeCli"),
        _ => Ok(()),
    };
    let release_result =
        cli_tunnel_registry::release_temporary_service_use(connection_id, cli_id).await;
    match (result, release_result) {
        (Err(error), _) => Err(error),
        (Ok(()), Ok(_)) => Ok(()),
        (Ok(()), Err(error)) => Err(error).context(format!(
            "failed release SSH CLI temporary use: connection_id={connection_id} cli_id={cli_id}"
        )),
    }
}

#[derive(Debug, Clone)]
pub(crate) struct RemoteSessionSnapshot {
    /// 远端会话 ID。
    pub(crate) engine_thread_id: String,
    /// 远端标题。
    pub(crate) title: String,
    /// 远端 cwd。
    pub(crate) cwd: String,
    /// 远端模型；不可靠时为 unknown。
    pub(crate) model_id: String,
    /// 远端最后活动时间。
    pub(crate) updated_at: Option<String>,
    /// 映射后的本地状态。
    pub(crate) status: ThreadStatusDto,
    /// 远端元数据。
    pub(crate) metadata: Value,
}

async fn persist_sessions(
    db: Arc<Database>,
    workspace: &WorkspaceDto,
    engine_id: &str,
    sessions: Vec<RemoteSessionSnapshot>,
) -> Result<()> {
    for session in sessions {
        let workspace_id = workspace.id.clone();
        let engine_id = engine_id.to_string();
        tokio::task::spawn_blocking({
            let db = db.clone();
            move || {
                threads::upsert_ssh_remote_thread_snapshot(
                    db.as_ref(),
                    &workspace_id,
                    &engine_id,
                    &session.engine_thread_id,
                    &session.model_id,
                    &session.title,
                    session.status,
                    &session.metadata,
                    session.updated_at.as_deref(),
                )
            }
        })
        .await
        .context("persist remote session task failed")??;
    }
    Ok(())
}

pub(crate) async fn list_codex_sessions(
    tunnel: &SshCliTunnel,
    cwd: &str,
) -> Result<Vec<RemoteSessionSnapshot>> {
    let (mut socket, _) = connect_async(format!("ws://127.0.0.1:{}/", tunnel.local_port()))
        .await
        .context("connect Codex SSH tunnel")?;
    let mut request_id = 1_u64;
    send_ws_request(&mut socket, &mut request_id, "initialize", json!({"clientInfo":{"name":"panes-ssh-sync","title":"Panes SSH Sync","version":"0"},"capabilities":{"experimentalApi":true}})).await?;
    socket
        .send(Message::Text(
            json!({"method":"initialized","params":{}})
                .to_string()
                .into(),
        ))
        .await
        .map_err(|error| anyhow::anyhow!("send Codex initialized notification failed: {error}"))?;
    let mut cursor: Option<String> = None;
    let mut out = Vec::new();
    for _ in 0..MAX_PAGES {
        let response = send_ws_request(
            &mut socket,
            &mut request_id,
            "thread/list",
            json!({
                "cursor": cursor,
                "limit": 100,
                "archived": false,
                "sortKey": "updated_at",
                "cwd": cwd,
            }),
        )
        .await?;
        for value in response
            .get("data")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            // if let Some(session) = parse_codex_session(value, cwd) {
            //     out.push(session);
            // }
            if let Some((mut session, title_needs_read)) = parse_codex_session(value, cwd) {
                if title_needs_read {
                    match send_ws_request(
                        &mut socket,
                        &mut request_id,
                        "thread/read",
                        json!({
                            "threadId": session.engine_thread_id.clone(),
                            "includeTurns": false,
                        }),
                    )
                    .await
                    {
                        Ok(response) => {
                            if let Some((resolved, false)) = parse_codex_session(&response, cwd) {
                                session = resolved;
                            }
                        }
                        Err(error) => log::warn!(
                            "读取 SSH 远端 Codex 会话标题失败: thread_id={} error={error:#}",
                            session.engine_thread_id
                        ),
                    }
                }
                out.push(session);
            }
        }
        cursor = response
            .get("nextCursor")
            .or_else(|| response.get("next_cursor"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        if cursor.is_none() {
            break;
        }
    }
    Ok(out)
}

async fn send_ws_request<S>(
    socket: &mut S,
    next_id: &mut u64,
    method: &str,
    params: Value,
) -> Result<Value>
where
    S: futures::Sink<Message>
        + futures::Stream<Item = std::result::Result<Message, tokio_tungstenite::tungstenite::Error>>
        + Unpin,
    S::Error: std::fmt::Display,
{
    let id = *next_id;
    *next_id += 1;
    socket
        .send(Message::Text(
            json!({"id":id,"method":method,"params":params})
                .to_string()
                .into(),
        ))
        .await
        .map_err(|error| anyhow::anyhow!("send Codex RPC failed: {error}"))?;
    loop {
        let message = tokio::time::timeout(REQUEST_TIMEOUT, socket.next())
            .await
            .context("Codex RPC timeout")?
            .ok_or_else(|| anyhow::anyhow!("Codex WebSocket closed"))??;
        let Message::Text(text) = message else {
            continue;
        };
        let value: Value = serde_json::from_str(&text).context("parse Codex RPC")?;
        if value.get("id").and_then(Value::as_u64) != Some(id) {
            continue;
        }
        if let Some(error) = value.get("error") {
            anyhow::bail!("Codex RPC error: {error}");
        }
        return Ok(value.get("result").cloned().unwrap_or(Value::Null));
    }
}

fn parse_codex_session(value: &Value, expected_cwd: &str) -> Option<(RemoteSessionSnapshot, bool)> {
    let thread = value.get("thread").unwrap_or(value);
    let cwd = string_field(thread, &["cwd"])?;
    if cwd != expected_cwd {
        return None;
    }
    let id = string_field(thread, &["id"])?;
    let updated = number_field(thread, &["updatedAt", "updated_at"])
        .or_else(|| number_field(thread, &["createdAt", "created_at"]));
    // let title = string_field(value, &["name", "threadName", "title"])
    //     .or_else(|| string_field(thread, &["name", "threadName", "title"]))
    //     .unwrap_or_else(|| id.clone());
    let explicit_title = string_field(value, &["name", "threadName", "title"])
        .or_else(|| string_field(thread, &["name", "threadName", "title"]))
        .map(|title| title.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|title| !title.is_empty());
    let preview = string_field(value, &["preview"])
        .or_else(|| string_field(thread, &["preview"]))
        .map(|preview| preview.trim().to_string())
        .filter(|preview| !preview.is_empty());
    let title_needs_read = explicit_title.is_none()
        && preview
            .as_deref()
            .is_none_or(|preview| preview.starts_with("<<ccr:") && preview.ends_with(">>"));
    let title = explicit_title
        .or_else(|| {
            preview
                .as_deref()
                .filter(|_| !title_needs_read)
                .and_then(|preview| preview.lines().next())
                .map(|title| title.split_whitespace().collect::<Vec<_>>().join(" "))
                .filter(|title| !title.is_empty())
        })
        .map(|title| title.chars().take(120).collect())
        .unwrap_or_else(|| id.clone());
    let model_id = string_field(
        thread,
        &[
            "model",
            "modelId",
            "model_id",
            "modelProvider",
            "model_provider",
        ],
    )
    .unwrap_or_else(|| "unknown".to_string());
    Some((
        RemoteSessionSnapshot {
            engine_thread_id: id,
            title,
            cwd: cwd.clone(),
            model_id,
            updated_at: updated.map(unix_to_rfc3339),
            status: status_from_value(value.get("status").or_else(|| thread.get("status"))),
            metadata: json!({"sshRemote":true,"codexRemoteCwd":cwd,"codexRemote":value}),
        },
        title_needs_read,
    ))
}

async fn list_opencode_sessions(
    tunnel: &SshCliTunnel,
    cwd: &str,
) -> Result<Vec<RemoteSessionSnapshot>> {
    let secret = tunnel
        .remote_service_secret()
        .context("OpenCode tunnel has no service secret")?;
    let auth = format!("Basic {}", BASE64.encode(format!("opencode:{secret}")));
    let values = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{}/session", tunnel.local_port()))
        .header(reqwest::header::AUTHORIZATION, auth)
        .query(&[("directory", cwd), ("roots", "true"), ("limit", "200")])
        .send()
        .await
        .context("request OpenCode sessions")?
        .error_for_status()
        .context("OpenCode session request failed")?
        .json::<Vec<Value>>()
        .await
        .context("parse OpenCode sessions")?;
    Ok(values
        .iter()
        .filter_map(|value| parse_opencode_session(value, cwd))
        .collect())
}

fn parse_opencode_session(value: &Value, expected_cwd: &str) -> Option<RemoteSessionSnapshot> {
    let cwd = string_field(value, &["directory", "cwd"])?;
    if cwd != expected_cwd {
        return None;
    }
    let id = string_field(value, &["id"])?;
    let title = string_field(value, &["title", "name"]).unwrap_or_else(|| id.clone());
    let updated = value
        .get("time")
        .and_then(|time| number_field(time, &["updated"]));
    Some(RemoteSessionSnapshot {
        engine_thread_id: id,
        title,
        cwd: cwd.clone(),
        model_id: "unknown".to_string(),
        updated_at: updated.map(unix_to_rfc3339),
        status: ThreadStatusDto::Idle,
        metadata: json!({"sshRemote":true,"opencodeRemoteCwd":cwd,"opencodeRemote":value}),
    })
}

// Claude 会话查询已经归入 ClaudeCodeCli::list_sessions。公共刷新服务只调用统一
// CLI 接口，不再直接通过 tunnel 调用 Claude /sessions 协议。
// pub(crate) async fn list_claude_sessions(
//     tunnel: &SshCliTunnel,
//     cwd: &str,
// ) -> Result<Vec<RemoteSessionSnapshot>> {
//     let values = reqwest::Client::new()
//         .get(format!("http://127.0.0.1:{}/sessions", tunnel.local_port()))
//         .query(&[("cwd", cwd)])
//         .send()
//         .await
//         .context("request Claude SSH remote sessions")?
//         .error_for_status()
//         .context("Claude SSH remote session request failed")?
//         .json::<Vec<Value>>()
//         .await
//         .context("parse Claude SSH remote sessions")?;
//     Ok(values
//         .iter()
//         .filter_map(|value| parse_claude_session(value, cwd))
//         .collect())
// }

fn parse_claude_session(value: &Value, expected_cwd: &str) -> Option<RemoteSessionSnapshot> {
    let cwd = string_field(value, &["cwd"])?;
    if cwd != expected_cwd {
        return None;
    }
    let id = string_field(value, &["id"])?;
    let title = string_field(value, &["title", "name"]).unwrap_or_else(|| id.clone());
    let updated_at = string_field(value, &["updatedAt", "updated_at"])
        .or_else(|| number_field(value, &["updatedAt", "updated_at"]).map(unix_to_rfc3339));
    Some(RemoteSessionSnapshot {
        engine_thread_id: id,
        title,
        cwd: cwd.clone(),
        model_id: "unknown".to_string(),
        updated_at,
        status: ThreadStatusDto::Idle,
        metadata: json!({"sshRemote":true,"claudeRemoteCwd":cwd,"claudeRemote":value}),
    })
}

fn string_field(value: &Value, names: &[&str]) -> Option<String> {
    names
        .iter()
        .find_map(|name| value.get(*name).and_then(Value::as_str).map(str::to_owned))
}
fn number_field(value: &Value, names: &[&str]) -> Option<i64> {
    names
        .iter()
        .find_map(|name| value.get(*name).and_then(Value::as_i64))
}
fn unix_to_rfc3339(value: i64) -> String {
    chrono::DateTime::from_timestamp(
        if value > 10_000_000_000 {
            value / 1000
        } else {
            value
        },
        0,
    )
    .map(|date| date.to_rfc3339())
    .unwrap_or_default()
}
fn status_from_value(value: Option<&Value>) -> ThreadStatusDto {
    match value
        .and_then(|item| item.get("type").or(Some(item)))
        .and_then(Value::as_str)
    {
        Some("inProgress") | Some("streaming") | Some("running") => ThreadStatusDto::Streaming,
        Some("error") | Some("failed") => ThreadStatusDto::Error,
        Some("completed") | Some("success") => ThreadStatusDto::Completed,
        _ => ThreadStatusDto::Idle,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn codex_parser_filters_by_cwd() {
        let value = json!({"thread":{"id":"same","cwd":"/a","createdAt":1}});
        assert!(parse_codex_session(&value, "/b").is_none());
        assert_eq!(
            parse_codex_session(&value, "/a")
                .unwrap()
                .0
                .engine_thread_id,
            "same"
        );
    }

    #[test]
    fn codex_parser_uses_preview_when_explicit_title_is_missing() {
        let value = json!({
            "thread": {
                "id": "thread-a",
                "cwd": "/a",
                "preview": "第一行标题\n后续内容",
                "createdAt": 1
            }
        });

        let (session, title_needs_read) = parse_codex_session(&value, "/a").unwrap();
        assert_eq!(session.title, "第一行标题");
        assert!(!title_needs_read);
    }

    #[test]
    fn codex_parser_marks_compacted_preview_for_thread_read() {
        let value = json!({
            "thread": {
                "id": "thread-a",
                "cwd": "/a",
                "preview": "<<ccr:8859c5dfe071,string,279B>>",
                "createdAt": 1
            }
        });

        let (session, title_needs_read) = parse_codex_session(&value, "/a").unwrap();
        assert_eq!(session.title, "thread-a");
        assert!(title_needs_read);
    }
    #[test]
    fn opencode_parser_filters_by_cwd() {
        let value = json!({"id":"same","directory":"/a","time":{"updated":2}});
        assert!(parse_opencode_session(&value, "/b").is_none());
        assert_eq!(parse_opencode_session(&value, "/a").unwrap().cwd, "/a");
    }

    #[test]
    fn claude_parser_filters_by_cwd_and_keeps_remote_timestamp() {
        let value = json!({
            "id":"same",
            "cwd":"/a",
            "title":"remote claude",
            "updatedAt":"2026-08-14T09:00:00.000Z"
        });
        assert!(parse_claude_session(&value, "/b").is_none());
        let session = parse_claude_session(&value, "/a").unwrap();
        assert_eq!(session.engine_thread_id, "same");
        assert_eq!(
            session.updated_at.as_deref(),
            Some("2026-08-14T09:00:00.000Z")
        );
    }
}
