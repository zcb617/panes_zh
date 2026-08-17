use std::{collections::HashMap, path::Path, sync::Arc};

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::{broadcast, mpsc, oneshot, Mutex};
use tokio_util::sync::CancellationToken;

use super::{
    claude_code_session_lifecycle::{
        shared_claude_code_session_handles, ClaudeCodeSessionHandleRegistry,
    },
    CliExecutionContext, CliForkedThread, CliLocationKind, CliReviewStarted, CliSessionSnapshot,
    CliTool,
};
use crate::{
    config::app_config::ClaudeCodeSessionMode,
    db,
    engines::{
        capabilities_for_engine, ApprovalRequestRoute, CodexRuntimeEvent, Engine,
        EngineCapabilities, EngineEvent, EngineSteerReceipt, EngineThread, ModelInfo,
        SandboxPolicy, ThreadScope, ThreadSyncSnapshot, TurnInput,
    },
    extensions,
    models::{
        CachedExtensionCatalogDto, ChatProviderUsageDto, CodexAppDto, CodexPluginDto,
        CodexSkillDto, EngineHealthDto, EngineInfoDto, ExtensionActionResultDto,
        ExtensionCatalogKindRefreshDto, ExtensionCatalogRefreshErrorDto, ExtensionItemDto,
        OpenCodeRuntimeCatalogDto, ThreadDto, ThreadStatusDto, WorkspaceDto,
    },
    path_utils, remote_project_claude_runtime_service, ssh,
    state::AppState,
};

/// Claude Code 对统一 CLI 操作接口的实现。
pub struct ClaudeCodeCli {
    state: AppState,
    remote_turn_use:
        Arc<Mutex<Option<remote_project_claude_runtime_service::RemoteClaudeServiceUse>>>,
    session_handles: Arc<ClaudeCodeSessionHandleRegistry>,
}

impl Clone for ClaudeCodeCli {
    fn clone(&self) -> Self {
        Self {
            state: self.state.clone(),
            remote_turn_use: self.remote_turn_use.clone(),
            session_handles: self.session_handles.clone(),
        }
    }
}

impl ClaudeCodeCli {
    pub fn new(state: AppState) -> Self {
        Self {
            state,
            remote_turn_use: Arc::new(Mutex::new(None)),
            session_handles: shared_claude_code_session_handles(),
        }
    }

    /// 用户进入某个 workspace 后，建立 Claude Code 的本机或 SSH 执行目标；未传 workspace 时只使用默认本机 workspace。
    pub async fn execution_context(
        &self,
        workspace_id: Option<&str>,
    ) -> Result<CliExecutionContext> {
        let db = self.state.db.clone();
        let workspace_id = workspace_id.map(str::to_string);
        let workspace = tokio::task::spawn_blocking(move || match workspace_id {
            Some(workspace_id) => db::workspaces::find_workspace_by_id(&db, &workspace_id)?
                .ok_or_else(|| anyhow::anyhow!("workspace 不存在: {workspace_id}")),
            None => db::workspaces::ensure_default_workspace(&db),
        })
        .await
        .context("读取 Claude Code workspace 任务失败")??;
        CliExecutionContext::from_workspace(&workspace)
    }

    /// 用户刷新某个项目目录的 Claude Code 扩展时，找到该目录所属的 workspace，保证 SSH 项目不会误用本机 Claude Code。
    pub async fn execution_context_for_cwd(
        &self,
        cwd: Option<&str>,
    ) -> Result<CliExecutionContext> {
        let Some(cwd) = cwd.map(str::trim).filter(|value| !value.is_empty()) else {
            return self.execution_context(None).await;
        };
        let db = self.state.db.clone();
        let cwd = cwd.to_string();
        let workspace = tokio::task::spawn_blocking(move || {
            let workspaces = db::workspaces::list_workspaces(&db)?;
            for workspace in workspaces {
                if path_utils::paths_equal(&workspace.root_path, &cwd) {
                    return Ok::<_, anyhow::Error>(Some(workspace));
                }
                let repos = db::repos::get_repos(&db, &workspace.id)?;
                if repos
                    .iter()
                    .any(|repo| path_utils::paths_equal(&repo.path, &cwd))
                {
                    return Ok(Some(workspace));
                }
            }
            Ok(None)
        })
        .await
        .context("按项目目录读取 Claude Code workspace 任务失败")??;
        match workspace {
            Some(workspace) => CliExecutionContext::from_workspace(&workspace),
            None => self.execution_context(None).await,
        }
    }

    async fn load_workspace(&self, context: &CliExecutionContext) -> Result<WorkspaceDto> {
        let db = self.state.db.clone();
        let workspace_id = context.workspace_id.clone();
        let workspace = tokio::task::spawn_blocking(move || {
            db::workspaces::find_workspace_by_id(&db, &workspace_id)
        })
        .await
        .context("读取当前 workspace 任务失败")??
        .ok_or_else(|| anyhow::anyhow!("workspace 不存在: {}", context.workspace_id))?;

        anyhow::ensure!(
            path_utils::paths_equal(&workspace.root_path, &context.root_path),
            "当前 workspace 的项目目录与 Claude Code 操作目标不一致"
        );

        match context.location_kind {
            CliLocationKind::Local => {
                anyhow::ensure!(
                    workspace.location_kind != "ssh",
                    "当前 workspace 是 SSH 远端项目，不能使用本机 Claude Code"
                );
            }
            CliLocationKind::Ssh => {
                anyhow::ensure!(
                    workspace.location_kind == "ssh",
                    "当前 workspace 不是 SSH 远端项目"
                );
                anyhow::ensure!(
                    workspace.ssh_connection_id == context.ssh_connection_id,
                    "当前 workspace 的 SSH 连接与 Claude Code 操作目标不一致"
                );
                remote_project_claude_runtime_service::validate_remote_claude_workspace(
                    &workspace,
                )?;
            }
        }
        Ok(workspace)
    }

    async fn remote_connection(
        &self,
        workspace: &WorkspaceDto,
    ) -> Result<db::ssh_connections::SshConnectionRecord> {
        let connection_id =
            remote_project_claude_runtime_service::validate_remote_claude_workspace(workspace)?
                .to_string();
        let db = self.state.db.clone();
        let lookup_connection_id = connection_id.clone();
        tokio::task::spawn_blocking(move || db::ssh_connections::find(&db, &lookup_connection_id))
            .await
            .context("读取 SSH 连接任务失败")??
            .ok_or_else(|| anyhow::anyhow!("SSH 连接不存在: {connection_id}"))
    }

    async fn remote_extension_catalog(
        &self,
        workspace: &WorkspaceDto,
    ) -> Result<CachedExtensionCatalogDto> {
        let connection = self.remote_connection(workspace).await?;
        let project_root = ssh::runtime::quote_posix(&workspace.root_path);
        let plugin_command = ssh::runtime::wrap_remote_login_shell_command(&format!(
            "cd -- {project_root} && env claude plugin list --available --json"
        ));
        let mut refresh_errors = Vec::new();
        let mut plugins = match ssh::gateway::run_command(&connection, &plugin_command).await {
            Ok(plugin_output) => match serde_json::from_str::<Value>(&plugin_output) {
                Ok(plugin_value) => {
                    extensions::claude::parse_plugins(&plugin_value, &HashMap::new())
                }
                Err(error) => {
                    log::warn!("解析 SSH 远端 Claude Plugin 目录失败: {error:#}");
                    refresh_errors.push(ExtensionCatalogRefreshErrorDto {
                        kind: "plugin".to_string(),
                        code: "parse_failed".to_string(),
                    });
                    Vec::new()
                }
            },
            Err(error) => {
                log::warn!("读取 SSH 远端 Claude Plugin 目录失败: {error:#}");
                refresh_errors.push(ExtensionCatalogRefreshErrorDto {
                    kind: "plugin".to_string(),
                    code: "read_failed".to_string(),
                });
                Vec::new()
            }
        };

        let project_skill_root = format!(
            "{}/.claude/skills",
            workspace.root_path.trim_end_matches('/')
        );
        let plugin_skill_roots = plugins
            .iter()
            .filter(|item| item.installed == Some(true))
            .filter_map(|item| {
                item.path.as_ref().map(|path| {
                    (
                        format!("{}/skills", path.trim_end_matches('/')),
                        item.id.clone(),
                        item.enabled.unwrap_or(true),
                    )
                })
            })
            .collect::<Vec<_>>();
        let quoted_plugin_skill_roots = plugin_skill_roots
            .iter()
            .map(|(path, _, _)| ssh::runtime::quote_posix(path))
            .collect::<Vec<_>>()
            .join(" ");
        let skill_command = ssh::runtime::wrap_remote_login_shell_command(&format!(
            "find \"$HOME/.claude/skills\" {project_root}/.claude/skills {quoted_plugin_skill_roots} -type f -name SKILL.md -print 2>/dev/null || true"
        ));
        let mut skills = match ssh::gateway::run_command(&connection, &skill_command).await {
            Ok(skill_output) => skill_output
                .lines()
                .map(str::trim)
                .filter(|path| !path.is_empty())
                .map(|path| {
                    let plugin = plugin_skill_roots
                        .iter()
                        .find(|(root, _, _)| path == root || path.starts_with(&format!("{root}/")));
                    let (scope, parent_plugin_id, enabled) = if path == project_skill_root
                        || path.starts_with(&format!("{project_skill_root}/"))
                    {
                        ("project", None, true)
                    } else if let Some((_, plugin_id, plugin_enabled)) = plugin {
                        ("plugin", Some(plugin_id.clone()), *plugin_enabled)
                    } else {
                        ("user", None, true)
                    };
                    let name = Path::new(path)
                        .parent()
                        .and_then(Path::file_name)
                        .and_then(|value| value.to_str())
                        .unwrap_or("Skill")
                        .to_string();
                    ExtensionItemDto {
                        id: path.to_string(),
                        provider_id: "claude".to_string(),
                        kind: "skill".to_string(),
                        name,
                        description: None,
                        version: None,
                        scope: scope.to_string(),
                        source: None,
                        marketplace: None,
                        path: Some(path.to_string()),
                        parent_plugin_id,
                        category: None,
                        officially_available: false,
                        catalog_authority: None,
                        installed: Some(true),
                        configured: None,
                        enabled: Some(enabled),
                        health: if enabled { "healthy" } else { "unknown" }.to_string(),
                        auth_state: None,
                        available_actions: Vec::new(),
                        requires_new_session: false,
                        read_only_reason: Some("ssh_remote_claude_extension_action".to_string()),
                        warning: None,

                        ..Default::default()}
                })
                .collect::<Vec<_>>(),
            Err(error) => {
                log::warn!("读取 SSH 远端 Claude Skill 目录失败: {error:#}");
                refresh_errors.push(ExtensionCatalogRefreshErrorDto {
                    kind: "skill".to_string(),
                    code: "read_failed".to_string(),
                });
                Vec::new()
            }
        };

        let mcp_command = ssh::runtime::wrap_remote_login_shell_command(&format!(
            "cd -- {project_root} && env claude mcp list"
        ));
        let mut mcp_servers = match ssh::gateway::run_command(&connection, &mcp_command).await {
            Ok(mcp_output) => extensions::claude::parse_mcp_servers(&mcp_output),
            Err(error) => {
                log::warn!("读取 SSH 远端 Claude MCP 目录失败: {error:#}");
                refresh_errors.push(ExtensionCatalogRefreshErrorDto {
                    kind: "mcp".to_string(),
                    code: "read_failed".to_string(),
                });
                Vec::new()
            }
        };
        for item in plugins
            .iter_mut()
            .chain(skills.iter_mut())
            .chain(mcp_servers.iter_mut())
        {
            item.available_actions.clear();
            item.read_only_reason = Some("ssh_remote_claude_extension_action".to_string());
        }
        let mut items = Vec::new();
        items.append(&mut skills);
        items.append(&mut plugins);
        items.append(&mut mcp_servers);
        let fetched_at = chrono::Utc::now().to_rfc3339();
        let kind_fetched_at = ["skill", "plugin", "mcp"]
            .into_iter()
            .map(|kind| (kind.to_string(), Some(fetched_at.clone())))
            .collect();

        Ok(CachedExtensionCatalogDto {
            provider_id: "claude".to_string(),
            cwd: Some(workspace.root_path.clone()),
            items,
            sources: Vec::new(),
            capabilities: extensions::provider_capabilities("claude"),
            fetched_at: Some(fetched_at.clone()),
            kind_fetched_at,
            last_attempt_at: Some(fetched_at.clone()),
            next_refresh_at: None,
            refreshing: false,
            refresh_completed_at: Some(fetched_at),
            has_snapshot: true,
            refresh_errors,
        })
    }

    // 旧实现把远端 Claude 会话快照交给公共会话刷新服务转换。会话查询和解析属于
    // Claude Code 业务，现已内联到 CliTool::list_sessions，不再经由外部服务转换。
    // fn map_session(
    //     session: remote_project_session_refresh_service::RemoteSessionSnapshot,
    // ) -> CliSessionSnapshot {
    //     let raw_status = Some(session.status.as_str().to_string());
    //     CliSessionSnapshot {
    //         engine_thread_id: session.engine_thread_id,
    //         title: session.title,
    //         preview: None,
    //         cwd: session.cwd,
    //         model_id: session.model_id,
    //         created_at: None,
    //         updated_at: session.updated_at,
    //         source_kind: Some("claude".to_string()),
    //         raw_status,
    //         active_flags: Vec::new(),
    //         status: session.status,
    //         archived: false,
    //         metadata: session.metadata,
    //     }
    // }

    fn uses_reuse_session(&self, context: &CliExecutionContext) -> bool {
        context.location_kind == CliLocationKind::Ssh
            && self.state.config.claude_code.session_mode() == ClaudeCodeSessionMode::ReuseSession
    }

    fn unsupported(action: &str) -> anyhow::Error {
        anyhow::anyhow!("Claude Code 当前不支持{action}，不会调用 Codex、OpenCode 或本机替代实现")
    }
}

#[async_trait]
impl CliTool for ClaudeCodeCli {
    fn id(&self) -> &str {
        "claude"
    }

    fn name(&self) -> &str {
        "Claude"
    }

    fn capabilities(&self) -> EngineCapabilities {
        capabilities_for_engine("claude")
    }

    async fn execution_context(&self, workspace_id: Option<&str>) -> Result<CliExecutionContext> {
        ClaudeCodeCli::execution_context(self, workspace_id).await
    }

    async fn execution_context_for_cwd(&self, cwd: Option<&str>) -> Result<CliExecutionContext> {
        ClaudeCodeCli::execution_context_for_cwd(self, cwd).await
    }

    async fn get_engine_info(&self, context: &CliExecutionContext) -> Result<EngineInfoDto> {
        let workspace = self.load_workspace(context).await?;
        if context.location_kind == CliLocationKind::Ssh {
            let remote_turn_use = self.remote_turn_use.lock().await;
            return remote_project_claude_runtime_service::engine_info(
                &workspace,
                remote_turn_use.as_ref(),
            )
            .await;
        }

        self.state
            .engines
            .list_engines()
            .await?
            .into_iter()
            .find(|engine| engine.id == "claude")
            .ok_or_else(|| anyhow::anyhow!("Claude Code 不在当前可用 CLI 列表中"))
    }

    async fn models_for_validation(
        &self,
        context: &CliExecutionContext,
        requested_model_id: &str,
    ) -> Result<Vec<ModelInfo>> {
        let workspace = self.load_workspace(context).await?;
        if context.location_kind == CliLocationKind::Ssh {
            let remote_turn_use = self.remote_turn_use.lock().await;
            return remote_project_claude_runtime_service::model_infos(
                &workspace,
                remote_turn_use.as_ref(),
            )
            .await;
        }
        self.state
            .engines
            .models_for_validation("claude", requested_model_id)
            .await
    }

    async fn get_chat_provider_usage(
        &self,
        context: &CliExecutionContext,
    ) -> Result<Option<ChatProviderUsageDto>> {
        self.load_workspace(context).await?;
        if context.location_kind == CliLocationKind::Ssh {
            return Ok(Some(ChatProviderUsageDto {
                engine_id: "claude".to_string(),
                name: "Claude".to_string(),
                available: false,
                windows: Vec::new(),
            }));
        }
        Ok(self
            .state
            .engines
            .chat_provider_usage()
            .await
            .into_iter()
            .find(|usage| usage.engine_id == "claude"))
    }

    async fn engine_health(&self, context: &CliExecutionContext) -> Result<EngineHealthDto> {
        let workspace = self.load_workspace(context).await?;
        if context.location_kind == CliLocationKind::Local {
            return self.state.engines.health("claude").await;
        }

        let connection = self.remote_connection(&workspace).await?;

        let availability = remote_project_claude_runtime_service::prewarm(&workspace).await;
        let version = if availability.is_ok() {
            let command = ssh::runtime::wrap_remote_login_shell_command("claude --version");
            ssh::gateway::run_command(&connection, &command)
                .await
                .ok()
                .and_then(|output| String::from_utf8(output.into()).ok())
                .map(|output| output.trim().to_string())
                .filter(|output| !output.is_empty())
        } else {
            None
        };
        let connection_name = workspace
            .connection_display_name
            .clone()
            .unwrap_or_else(|| "未命名 SSH 连接".to_string());

        Ok(EngineHealthDto {
            id: "claude".to_string(),
            available: availability.is_ok(),
            version,
            details: Some(match availability {
                Ok(()) => format!("SSH 远端 Claude：{connection_name}"),
                Err(error) => format!("SSH 远端 Claude 不可用：{error:#}"),
            }),
            warnings: Vec::new(),
            checks: Vec::new(),
            fixes: Vec::new(),
            protocol_diagnostics: None,
        })
    }

    fn subscribe_codex_runtime_events(&self) -> broadcast::Receiver<CodexRuntimeEvent> {
        let (sender, receiver) = broadcast::channel(1);
        drop(sender);
        receiver
    }

    async fn prewarm_engine(&self, context: &CliExecutionContext) -> Result<()> {
        let workspace = self.load_workspace(context).await?;
        if context.location_kind == CliLocationKind::Ssh {
            remote_project_claude_runtime_service::prewarm(&workspace).await
        } else {
            self.state.engines.prewarm("claude").await
        }
    }

    async fn uses_external_sandbox(&self, context: &CliExecutionContext) -> Result<bool> {
        self.load_workspace(context).await?;
        Ok(false)
    }

    async fn list_sessions(
        &self,
        context: &CliExecutionContext,
        search_term: Option<&str>,
        archived: Option<bool>,
    ) -> Result<Vec<CliSessionSnapshot>> {
        let workspace = self.load_workspace(context).await?;
        if context.location_kind == CliLocationKind::Local || archived == Some(true) {
            return Ok(Vec::new());
        }

        let connection_id =
            remote_project_claude_runtime_service::validate_remote_claude_workspace(&workspace)?;
        let service_use =
            remote_project_claude_runtime_service::acquire_temporary(&workspace).await?;
        let tunnel = ssh::cli_tunnel_registry::get(connection_id, "claude")
            .await
            .context("当前 SSH 远端 Claude tunnel 不存在")?;
        let result = async {
            reqwest::Client::new()
                .get(format!("http://127.0.0.1:{}/sessions", tunnel.local_port()))
                .query(&[("cwd", workspace.root_path.as_str())])
                .send()
                .await
                .context("读取 SSH 远端 Claude 会话失败")?
                .error_for_status()
                .context("SSH 远端 Claude 会话读取被拒绝")?
                .json::<Vec<Value>>()
                .await
                .context("解析 SSH 远端 Claude 会话失败")
        }
        .await;
        service_use.release().await;
        let query = search_term.map(str::trim).filter(|value| !value.is_empty());
        result.map(|values| {
            values
                .into_iter()
                .filter_map(|value| {
                    let cwd = value.get("cwd")?.as_str()?.to_string();
                    if !path_utils::paths_equal(&cwd, &workspace.root_path) {
                        return None;
                    }
                    let engine_thread_id = value.get("id")?.as_str()?.to_string();
                    let title = value
                        .get("title")
                        .or_else(|| value.get("name"))
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .unwrap_or_else(|| engine_thread_id.clone());
                    if query.is_some_and(|query| {
                        !title.to_lowercase().contains(&query.to_lowercase())
                            && !engine_thread_id.contains(query)
                    }) {
                        return None;
                    }
                    let updated_at = value
                        .get("updatedAt")
                        .or_else(|| value.get("updated_at"))
                        .and_then(|value| {
                            value.as_str().map(str::to_string).or_else(|| {
                                value.as_i64().and_then(|timestamp| {
                                    chrono::DateTime::from_timestamp(
                                        if timestamp > 10_000_000_000 {
                                            timestamp / 1000
                                        } else {
                                            timestamp
                                        },
                                        0,
                                    )
                                    .map(|date| date.to_rfc3339())
                                })
                            })
                        });
                    Some(CliSessionSnapshot {
                        engine_thread_id,
                        title,
                        preview: None,
                        cwd: cwd.clone(),
                        model_id: "unknown".to_string(),
                        created_at: None,
                        updated_at,
                        source_kind: Some("claude".to_string()),
                        raw_status: Some("idle".to_string()),
                        active_flags: Vec::new(),
                        status: ThreadStatusDto::Idle,
                        archived: false,
                        metadata: json!({
                            "sshRemote": true,
                            "claudeRemoteCwd": cwd,
                            "claudeRemote": value,
                        }),
                    })
                })
                .collect()
        })
    }

    async fn read_session(
        &self,
        context: &CliExecutionContext,
        engine_thread_id: &str,
    ) -> Result<CliSessionSnapshot> {
        self.list_sessions(context, None, Some(false))
            .await?
            .into_iter()
            .find(|session| session.engine_thread_id == engine_thread_id)
            .ok_or_else(|| {
                anyhow::anyhow!("Claude Code 会话不存在或目录不匹配: session_id={engine_thread_id}")
            })
    }

    async fn acquire_turn(&self, context: &CliExecutionContext, thread: &ThreadDto) -> Result<()> {
        let workspace = self.load_workspace(context).await?;
        if context.location_kind == CliLocationKind::Ssh {
            if self.uses_reuse_session(context)
                && self.session_handles.prepare_turn(&thread.id).await
            {
                return Ok(());
            }
            let mut remote_turn_use = self.remote_turn_use.lock().await;
            anyhow::ensure!(
                remote_turn_use.is_none(),
                "当前 Claude Code 工具实例已经持有其他整轮使用权"
            );
            *remote_turn_use = Some(
                remote_project_claude_runtime_service::acquire_turn(&workspace, &thread.id).await?,
            );
        }
        Ok(())
    }

    async fn start_thread(
        &self,
        context: &CliExecutionContext,
        thread: &ThreadDto,
        scope: ThreadScope,
        resume_engine_thread_id: Option<&str>,
        model: &str,
        sandbox: SandboxPolicy,
    ) -> Result<EngineThread> {
        self.load_workspace(context).await?;
        if context.location_kind == CliLocationKind::Ssh {
            if self.uses_reuse_session(context) && self.session_handles.contains(&thread.id).await {
                let (engine, _) = self.session_handles.session_runtime(&thread.id).await?;
                return Engine::start_thread(
                    engine.as_ref(),
                    scope,
                    resume_engine_thread_id,
                    model,
                    sandbox,
                )
                .await;
            }
            let remote_turn_use = self.remote_turn_use.lock().await;
            let service_use = remote_turn_use
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("当前 SSH 远端 Claude 会话尚未建立持续使用关系"))?;
            return Engine::start_thread(
                service_use.engine().as_ref(),
                scope,
                resume_engine_thread_id,
                model,
                sandbox,
            )
            .await;
        }
        let engine_thread_id = self
            .state
            .engines
            .ensure_engine_thread(thread, Some(model), scope, sandbox)
            .await?;
        Ok(EngineThread { engine_thread_id })
    }

    async fn send_message(
        &self,
        context: &CliExecutionContext,
        thread: &ThreadDto,
        engine_thread_id: &str,
        input: TurnInput,
        event_tx: mpsc::Sender<EngineEvent>,
        cancellation: CancellationToken,
    ) -> Result<()> {
        self.load_workspace(context).await?;
        if context.location_kind == CliLocationKind::Ssh {
            if self.uses_reuse_session(context) {
                let session_exists = self.session_handles.contains(&thread.id).await;
                let (engine, remote_base_url, service_use) = if session_exists {
                    let (engine, remote_base_url) =
                        self.session_handles.session_runtime(&thread.id).await?;
                    (engine, remote_base_url, None)
                } else {
                    let service_use =
                        self.remote_turn_use.lock().await.take().ok_or_else(|| {
                            anyhow::anyhow!("当前 SSH 远端 Claude 会话尚未建立持续使用关系")
                        })?;
                    let engine = service_use.engine().clone();
                    let remote_base_url = reqwest::Url::parse(engine.base_url())
                        .context("解析 SSH 远端 Claude 服务地址失败")?;
                    (engine, remote_base_url, Some(service_use))
                };
                let persistent_turn = engine
                    .prepare_persistent_turn(engine_thread_id, input)
                    .await?;
                let handle_id = if let Some(service_use) = service_use {
                    self.session_handles
                        .create_or_get(
                            &thread.id,
                            remote_base_url,
                            Some(service_use),
                            persistent_turn.params.clone(),
                        )
                        .await?
                        .handle_id
                } else {
                    self.session_handles
                        .send_message(&thread.id, persistent_turn.params.clone())
                        .await?
                        .handle_id
                };

                let session_handles = self.session_handles.clone();
                let cancel_thread_id = thread.id.clone();
                let cancel_token = cancellation.clone();
                let cancel_task = tokio::spawn(async move {
                    cancel_token.cancelled().await;
                    if let Err(error) = session_handles.interrupt(&cancel_thread_id).await {
                        log::warn!(
                            "中断 SSH 远端 Claude 复用会话失败: thread_id={} error={error:#}",
                            cancel_thread_id
                        );
                    }
                });
                let result = engine
                    .relay_persistent_turn(engine_thread_id, &handle_id, persistent_turn, event_tx)
                    .await;
                cancel_task.abort();
                let idle_result = self.session_handles.mark_turn_completed(&thread.id).await;
                if let Err(error) = result {
                    let _ = idle_result;
                    return Err(error);
                }
                return idle_result;
            } else {
                let service_use = self.remote_turn_use.lock().await.take().ok_or_else(|| {
                    anyhow::anyhow!("当前 SSH 远端 Claude 会话尚未建立持续使用关系")
                })?;
                let result = Engine::send_message(
                    service_use.engine().as_ref(),
                    engine_thread_id,
                    input,
                    event_tx,
                    cancellation,
                )
                .await;
                service_use.release().await;
                return result;
            }
        }
        self.state
            .engines
            .send_message(thread, engine_thread_id, input, event_tx, cancellation)
            .await
    }

    async fn steer_message(
        &self,
        context: &CliExecutionContext,
        thread: &ThreadDto,
        engine_thread_id: &str,
        client_steer_id: &str,
        content: &str,
        input: TurnInput,
    ) -> Result<EngineSteerReceipt> {
        let workspace = self.load_workspace(context).await?;
        if context.location_kind == CliLocationKind::Ssh {
            let service_use =
                remote_project_claude_runtime_service::acquire_temporary(&workspace).await?;
            let result = Engine::steer_message(
                service_use.engine().as_ref(),
                engine_thread_id,
                client_steer_id,
                content,
                input,
            )
            .await;
            service_use.release().await;
            return result;
        }
        self.state
            .engines
            .steer_message(thread, engine_thread_id, client_steer_id, content, input)
            .await
    }

    async fn respond_to_approval(
        &self,
        context: &CliExecutionContext,
        thread: &ThreadDto,
        approval_id: &str,
        response: Value,
        route: Option<ApprovalRequestRoute>,
    ) -> Result<()> {
        let workspace = self.load_workspace(context).await?;
        if context.location_kind == CliLocationKind::Ssh {
            remote_project_claude_runtime_service::respond_to_approval(
                &workspace,
                thread,
                approval_id,
                response,
                route,
            )
            .await
        } else {
            self.state
                .engines
                .respond_to_approval(thread, approval_id, response, route)
                .await
        }
    }

    async fn interrupt(
        &self,
        context: &CliExecutionContext,
        thread: &ThreadDto,
        _engine_thread_id: &str,
    ) -> Result<()> {
        let workspace = self.load_workspace(context).await?;
        if context.location_kind == CliLocationKind::Ssh {
            if self.uses_reuse_session(context) {
                if self.session_handles.contains(&thread.id).await {
                    self.session_handles.interrupt(&thread.id).await?;
                }
                Ok(())
            } else {
                remote_project_claude_runtime_service::interrupt(&workspace, thread).await
            }
        } else {
            self.state.engines.interrupt(thread).await
        }
    }

    async fn archive_thread(
        &self,
        context: &CliExecutionContext,
        thread: &ThreadDto,
        _engine_thread_id: &str,
    ) -> Result<()> {
        self.load_workspace(context).await?;
        if context.location_kind == CliLocationKind::Ssh {
            Ok(())
        } else {
            self.state.engines.archive_thread(thread).await
        }
    }

    async fn unarchive_thread(
        &self,
        context: &CliExecutionContext,
        thread: &ThreadDto,
        _engine_thread_id: &str,
    ) -> Result<()> {
        self.load_workspace(context).await?;
        if context.location_kind == CliLocationKind::Ssh {
            Ok(())
        } else {
            self.state.engines.unarchive_thread(thread).await
        }
    }

    async fn forget_session(
        &self,
        context: &CliExecutionContext,
        _thread: &ThreadDto,
        _engine_thread_id: &str,
    ) -> Result<()> {
        self.load_workspace(context).await?;
        Ok(())
    }

    async fn read_thread_preview(
        &self,
        context: &CliExecutionContext,
        thread: &ThreadDto,
        engine_thread_id: &str,
    ) -> Result<Option<String>> {
        self.load_workspace(context).await?;
        if context.location_kind == CliLocationKind::Ssh {
            Ok(None)
        } else {
            Ok(self
                .state
                .engines
                .read_thread_preview(thread, engine_thread_id)
                .await)
        }
    }

    async fn read_thread_sync_snapshot(
        &self,
        context: &CliExecutionContext,
        thread: &ThreadDto,
        _engine_thread_id: &str,
    ) -> Result<Option<ThreadSyncSnapshot>> {
        self.load_workspace(context).await?;
        if context.location_kind == CliLocationKind::Ssh {
            Ok(None)
        } else {
            self.state.engines.read_thread_sync_snapshot(thread).await
        }
    }

    async fn set_thread_name(
        &self,
        context: &CliExecutionContext,
        thread: &ThreadDto,
        engine_thread_id: &str,
        name: &str,
    ) -> Result<()> {
        self.load_workspace(context).await?;
        if context.location_kind == CliLocationKind::Ssh {
            Ok(())
        } else {
            self.state
                .engines
                .set_thread_name(thread, engine_thread_id, name)
                .await
        }
    }

    async fn list_codex_skills(
        &self,
        context: &CliExecutionContext,
        _cwd: &str,
    ) -> Result<Vec<CodexSkillDto>> {
        self.load_workspace(context).await?;
        Err(Self::unsupported("Codex Skill 目录"))
    }

    async fn list_codex_apps(&self, context: &CliExecutionContext) -> Result<Vec<CodexAppDto>> {
        self.load_workspace(context).await?;
        Err(Self::unsupported("Codex Apps 目录"))
    }

    async fn list_codex_plugins(
        &self,
        context: &CliExecutionContext,
        _cwd: &str,
    ) -> Result<Vec<CodexPluginDto>> {
        self.load_workspace(context).await?;
        Err(Self::unsupported("Codex Plugin 目录"))
    }

    async fn get_opencode_runtime_catalog(
        &self,
        context: &CliExecutionContext,
        _cwd: &str,
    ) -> Result<OpenCodeRuntimeCatalogDto> {
        self.load_workspace(context).await?;
        Err(Self::unsupported("OpenCode 参数"))
    }

    async fn refresh_extension_catalog(
        &self,
        context: &CliExecutionContext,
        cwd: Option<&str>,
        requested_kinds: &[String],
    ) -> Result<Vec<ExtensionCatalogKindRefreshDto>> {
        let workspace = self.load_workspace(context).await?;
        if context.location_kind == CliLocationKind::Ssh {
            let catalog = self.remote_extension_catalog(&workspace).await?;
            return Ok(requested_kinds
                .iter()
                .map(|kind| {
                    ExtensionCatalogKindRefreshDto::success(
                        kind,
                        catalog
                            .items
                            .iter()
                            .filter(|item| item.kind == *kind)
                            .cloned()
                            .collect(),
                    )
                })
                .collect());
        }
        let mut results = Vec::new();
        for kind in requested_kinds {
            results.push(extensions::claude::refresh_kind(&self.state.engines, cwd, kind).await);
        }
        Ok(results)
    }

    async fn get_extension_catalog(
        &self,
        context: &CliExecutionContext,
        cwd: Option<&str>,
    ) -> Result<CachedExtensionCatalogDto> {
        let workspace = self.load_workspace(context).await?;
        if context.location_kind == CliLocationKind::Ssh {
            return self.remote_extension_catalog(&workspace).await;
        }
        extensions::refresh::load_cached_catalog(&self.state, "claude", cwd).await
    }

    async fn get_extensions(
        &self,
        context: &CliExecutionContext,
    ) -> Result<Vec<ExtensionItemDto>> {
        let catalog = self.get_extension_catalog(context, None).await?;
        let mut items = catalog.items;
        for item in &mut items {
            match item.kind.as_str() {
                "skill" => {
                    item.insert_text = Some(format!("/{} ", item.name));
                    item.group = Some("skills".to_string());
                }
                "plugin" => {
                    item.panel = Some("plugins".to_string());
                    item.group = Some("plugins".to_string());
                }
                "mcp" => {
                    item.panel = Some("mcp".to_string());
                    item.group = Some("mcp".to_string());
                }
                _ => {}
            }
        }
        let panel_ids = ["skills", "plugins", "mcp"];
        items.extend(panel_ids.into_iter().map(|id| ExtensionItemDto {
            id: id.to_string(),
            provider_id: "claude".to_string(),
            kind: "command".to_string(),
            name: id.to_string(),
            description: None,
            panel: Some(id.to_string()),
            group: Some("commands".to_string()),
            ..Default::default()
        }));
        Ok(items)
    }

    async fn perform_extension_action(
        &self,
        context: &CliExecutionContext,
        item: ExtensionItemDto,
        action: &str,
        scope: Option<&str>,
    ) -> Result<ExtensionActionResultDto> {
        let workspace = self.load_workspace(context).await?;
        anyhow::ensure!(
            context.location_kind == CliLocationKind::Local,
            "SSH 远端 Claude Code 当前不执行扩展变更，也不会调用本机 Claude Code"
        );
        extensions::claude::perform_action(&item, action, scope, Some(workspace.root_path.as_str()))
            .await
    }

    async fn fork_thread(
        &self,
        context: &CliExecutionContext,
        _engine_thread_id: &str,
        _cwd: &str,
        _model: &str,
        _sandbox: SandboxPolicy,
    ) -> Result<CliForkedThread> {
        self.load_workspace(context).await?;
        Err(Self::unsupported("会话分支"))
    }

    async fn rollback_thread(
        &self,
        context: &CliExecutionContext,
        _engine_thread_id: &str,
        _num_turns: u32,
    ) -> Result<ThreadSyncSnapshot> {
        self.load_workspace(context).await?;
        Err(Self::unsupported("会话回滚"))
    }

    async fn compact_thread(
        &self,
        context: &CliExecutionContext,
        _engine_thread_id: &str,
    ) -> Result<()> {
        self.load_workspace(context).await?;
        Err(Self::unsupported("会话压缩"))
    }

    async fn start_review(
        &self,
        context: &CliExecutionContext,
        _source_engine_thread_id: &str,
        _target: Value,
        _delivery: Option<&str>,
        _event_tx: mpsc::Sender<EngineEvent>,
        _cancellation: CancellationToken,
        _started_tx: oneshot::Sender<CliReviewStarted>,
    ) -> Result<()> {
        self.load_workspace(context).await?;
        Err(Self::unsupported("Codex 代码审查"))
    }
}
