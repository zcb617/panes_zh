use anyhow::Context;
use serde::Serialize;
use tauri::{AppHandle, Emitter};

/// 前端用于刷新 SSH 远端项目会话列表的事件名称。
pub const SSH_REMOTE_PROJECT_SESSIONS_REFRESHED_EVENT: &str =
    "ssh-remote-project-sessions-refreshed";

/// SSH 远端项目会话刷新完成后发送给前端的事件载荷。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SshRemoteProjectSessionsRefreshedEvent {
    /// 本次刷新所属的工作区标识。
    pub workspace_id: String,
    /// 刷新成功的远端 CLI 标识列表。
    pub succeeded_cli_ids: Vec<String>,
    /// 刷新失败的远端 CLI 标识列表。
    pub failed_cli_ids: Vec<String>,
}

/// 向前端发送 SSH 远端项目会话刷新完成事件。
///
/// 调用方应当只在数据库事务成功提交后调用此函数；本函数不承担任何
/// 数据库、会话刷新或通知展示职责，只负责把固定契约发送给 Tauri 前端。
pub fn notify_ssh_remote_project_sessions_refreshed(
    app: &AppHandle,
    event: SshRemoteProjectSessionsRefreshedEvent,
) -> anyhow::Result<()> {
    app.emit(SSH_REMOTE_PROJECT_SESSIONS_REFRESHED_EVENT, event)
        .context("failed to emit SSH remote project sessions refreshed event")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_contract_has_fixed_name_and_camel_case_payload() {
        assert_eq!(
            SSH_REMOTE_PROJECT_SESSIONS_REFRESHED_EVENT,
            "ssh-remote-project-sessions-refreshed"
        );

        let payload = serde_json::to_value(SshRemoteProjectSessionsRefreshedEvent {
            workspace_id: "workspace-1".to_owned(),
            succeeded_cli_ids: vec!["codex".to_owned()],
            failed_cli_ids: vec!["claude".to_owned()],
        })
        .expect("event DTO should serialize");

        assert_eq!(payload["workspaceId"], "workspace-1");
        assert_eq!(payload["succeededCliIds"][0], "codex");
        assert_eq!(payload["failedCliIds"][0], "claude");
        assert!(payload.get("workspace_id").is_none());
    }
}
