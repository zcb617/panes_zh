use std::sync::Arc;

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::{broadcast, mpsc, oneshot, Mutex};
use tokio_util::sync::CancellationToken;

use super::{
    CliExecutionContext, CliForkedThread, CliLocationKind, CliReviewStarted,
    CliSessionNotFoundError, CliSessionSnapshot, CliTool,
};
use crate::{
    db,
    engines::{
        capabilities_for_engine,
        codex::{CodexEngine, CodexReviewStarted, CodexThreadNotFoundError},
        map_engine_capabilities, map_model_info, map_provider_usage, ApprovalRequestRoute,
        CodexRuntimeEvent, Engine, EngineCapabilities, EngineEvent, EngineSteerReceipt,
        EngineThread, ModelInfo, SandboxPolicy, ThreadScope, ThreadSyncSnapshot, TurnInput,
    },
    extensions,
    local_cli_service_lifecycle::{LocalCliHandle, LocalCliServiceLifecycle},
    models::{
        CachedExtensionCatalogDto, ChatProviderUsageDto, CodexAppDto, CodexPluginDto,
        CodexSkillDto, EngineHealthDto, EngineInfoDto, ExtensionActionResultDto,
        ExtensionCatalogKindRefreshDto, ExtensionItemDto, OpenCodeRuntimeCatalogDto, ThreadDto,
        ThreadStatusDto, WorkspaceDto,
    },
    path_utils, remote_project_codex_runtime_service, remote_project_session_refresh_service, ssh,
    state::AppState,
};

/// Codex 对统一 CLI 操作接口的实现。
///
/// 本机项目继续使用现有 Codex 业务对象；SSH 项目继续使用现有远端 Codex
/// 服务和生命周期入口。任何远端操作失败时都直接返回错误，不会改用本机 Codex。
#[derive(Clone)]
pub struct CodexCli {
    state: AppState,
    remote_turn_use: Arc<Mutex<Option<Arc<CodexEngine>>>>,
}

impl CodexCli {
    pub fn new(state: AppState) -> Self {
        Self {
            state,
            remote_turn_use: Arc::new(Mutex::new(None)),
        }
    }

    async fn local_engine(&self) -> Result<Arc<CodexEngine>> {
        let service = LocalCliServiceLifecycle::get("codex").await?;
        match service.handle() {
            LocalCliHandle::Codex(engine) => Ok(engine.clone()),
            _ => anyhow::bail!("本地 CLI 生命周期返回了错误的 Codex 句柄类型"),
        }
    }

    async fn configure_local_computer_control(&self) -> Result<Arc<CodexEngine>> {
        let engine = self.local_engine().await?;
        engine.set_computer_control_service(self.state.computer_control_service.clone());
        Ok(engine)
    }

    /// 用户进入某个 workspace 的 Codex 功能时，读取该 workspace 的正式项目位置，作为后续本机或 SSH 操作的依据。
    pub async fn execution_context(
        &self,
        workspace_id: Option<&str>,
    ) -> Result<CliExecutionContext> {
        let workspace_id = workspace_id
            .map(str::trim)
            .filter(|workspace_id| !workspace_id.is_empty())
            .ok_or_else(|| anyhow::anyhow!("请先选择项目"))?
            .to_string();
        let db = self.state.db.clone();
        let workspace = tokio::task::spawn_blocking(move || {
            db::workspaces::find_workspace_by_id(&db, &workspace_id)?.ok_or_else(|| {
                anyhow::anyhow!("项目不存在或已被移除，请重新选择项目: {workspace_id}")
            })
        })
        .await
        .context("读取 Codex workspace 任务失败")??;
        CliExecutionContext::from_workspace(&workspace)
    }

    /// 用户刷新某个项目目录的 Codex 扩展时，找到该目录所属的 workspace，保证 SSH 项目只读取正式绑定的远端目录。
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
        .context("按项目目录读取 Codex workspace 任务失败")??;
        match workspace {
            Some(workspace) => CliExecutionContext::from_workspace(&workspace),
            None => self.execution_context(None).await,
        }
    }

    // 旧的 Codex 专用整轮入口已经停用；调用方现在通过 CliTool::acquire_turn 取得整轮使用权。
    // pub async fn for_turn(
    //     state: AppState,
    //     context: &CliExecutionContext,
    //     thread_id: &str,
    // ) -> Result<Self> {
    //     let cli = Self::new(state);
    //     if context.location_kind == CliLocationKind::Ssh {
    //         let workspace = cli.load_workspace(context).await?;
    //         let service_use =
    //             remote_project_codex_runtime_service::acquire_turn(&workspace, thread_id).await?;
    //         *cli.remote_turn_use.lock().await = Some(service_use);
    //     }
    //     Ok(cli)
    // }

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
            "当前 workspace 的项目目录与 Codex 操作目标不一致"
        );

        match context.location_kind {
            CliLocationKind::Local => {
                anyhow::ensure!(
                    workspace.location_kind != "ssh",
                    "当前 workspace 是 SSH 远端项目，不能使用本机 Codex"
                );
            }
            CliLocationKind::Ssh => {
                anyhow::ensure!(
                    workspace.location_kind == "ssh",
                    "当前 workspace 不是 SSH 远端项目"
                );
                anyhow::ensure!(
                    workspace.ssh_connection_id == context.ssh_connection_id,
                    "当前 workspace 的 SSH 绑定与 Codex 操作目标不一致"
                );
            }
        }

        Ok(workspace)
    }

    fn map_session(
        summary: crate::engines::CodexRemoteThreadSummary,
        is_ssh: bool,
    ) -> CliSessionSnapshot {
        let status = match summary.status_type.as_str() {
            "systemError" => ThreadStatusDto::Error,
            "active"
                if summary.active_flags.iter().any(|flag| {
                    matches!(flag.as_str(), "waitingOnApproval" | "waitingOnUserInput")
                }) =>
            {
                ThreadStatusDto::AwaitingApproval
            }
            "active" => ThreadStatusDto::Streaming,
            "completed" => ThreadStatusDto::Completed,
            _ => ThreadStatusDto::Idle,
        };
        let timestamp_to_rfc3339 = |timestamp: i64| {
            let (seconds, nanos) = if timestamp > 10_000_000_000 {
                (timestamp / 1000, ((timestamp % 1000) as u32) * 1_000_000)
            } else {
                (timestamp, 0)
            };
            chrono::DateTime::from_timestamp(seconds, nanos).map(|value| value.to_rfc3339())
        };
        let created_at = timestamp_to_rfc3339(summary.created_at);
        let updated_at = timestamp_to_rfc3339(summary.updated_at);
        let preview = (!summary.preview.trim().is_empty()).then(|| summary.preview.clone());
        let metadata = json!({
            "sshRemote": is_ssh,
            "codexRemoteCwd": summary.cwd.clone(),
            "codexRemote": {
                "id": summary.engine_thread_id.clone(),
                "title": summary.title.clone(),
                "preview": summary.preview.clone(),
                "cwd": summary.cwd.clone(),
                "model": summary.model_id.clone(),
                "reasoningEffort": summary.reasoning_effort.clone(),
                "modelProvider": summary.model_provider.clone(),
                "sourceKind": summary.source_kind.clone(),
                "status": {
                    "type": summary.status_type.clone(),
                    "activeFlags": summary.active_flags.clone(),
                },
                "archived": summary.archived,
                "createdAt": summary.created_at,
                "updatedAt": summary.updated_at,
            },
            "codexModelProvider": summary.model_provider.clone(),
            "reasoningEffort": summary.reasoning_effort.clone(),
            "codexSourceKind": summary.source_kind.clone(),
            "codexThreadStatus": summary.status_type.clone(),
            "codexThreadActiveFlags": summary.active_flags.clone(),
        });

        CliSessionSnapshot {
            engine_thread_id: summary.engine_thread_id.clone(),
            title: summary.title.unwrap_or(summary.engine_thread_id),
            preview,
            cwd: summary.cwd,
            model_id: summary.model_id.unwrap_or_else(|| "unknown".to_string()),
            reasoning_effort: summary.reasoning_effort,
            created_at,
            updated_at,
            source_kind: Some(summary.source_kind),
            raw_status: Some(summary.status_type),
            active_flags: summary.active_flags,
            status,
            archived: summary.archived,
            metadata,
        }
    }
}

#[async_trait]
impl CliTool for CodexCli {
    fn id(&self) -> &str {
        "codex"
    }

    fn name(&self) -> &str {
        "Codex"
    }

    fn capabilities(&self) -> EngineCapabilities {
        capabilities_for_engine("codex")
    }

    async fn execution_context(&self, workspace_id: Option<&str>) -> Result<CliExecutionContext> {
        CodexCli::execution_context(self, workspace_id).await
    }

    async fn execution_context_for_cwd(&self, cwd: Option<&str>) -> Result<CliExecutionContext> {
        CodexCli::execution_context_for_cwd(self, cwd).await
    }

    async fn get_engine_info(&self, context: &CliExecutionContext) -> Result<EngineInfoDto> {
        /*
        旧实现先通过 workspace 构造远端运行对象，再读取模型。模型目录属于机器，不再执行：
        let workspace = self.load_workspace(context).await?;
        if context.location_kind == CliLocationKind::Ssh {
            let engine = remote_project_codex_runtime_service::runtime(&workspace).await?;
            let models = engine.list_models_runtime().await;
            return Ok(EngineInfoDto {
                id: "codex".to_string(),
                name: "Codex".to_string(),
                models: models.into_iter().map(map_model_info).collect(),
                capabilities: map_engine_capabilities(capabilities_for_engine("codex")),
            });
        }
        */
        if context.location_kind == CliLocationKind::Ssh {
            let connection_id = context
                .ssh_connection_id
                .as_deref()
                .context("SSH 远端 Codex 未绑定连接")?;
            let models =
                remote_project_codex_runtime_service::model_infos(connection_id, None).await?;
            return Ok(EngineInfoDto {
                id: "codex".to_string(),
                name: "Codex".to_string(),
                models: models.into_iter().map(map_model_info).collect(),
                capabilities: map_engine_capabilities(capabilities_for_engine("codex")),
            });
        }

        let engine = self.local_engine().await?;
        let models = engine.list_models_runtime().await;
        Ok(EngineInfoDto {
            id: "codex".to_string(),
            name: "Codex".to_string(),
            models: models.into_iter().map(map_model_info).collect(),
            capabilities: map_engine_capabilities(capabilities_for_engine("codex")),
        })
    }

    async fn models_for_validation(
        &self,
        context: &CliExecutionContext,
        requested_model_id: &str,
    ) -> Result<Vec<ModelInfo>> {
        /*
        旧实现通过 workspace 取得远端模型目录，不再执行：
        let workspace = self.load_workspace(context).await?;
        if context.location_kind == CliLocationKind::Ssh {
            return Ok(remote_project_codex_runtime_service::runtime(&workspace)
                .await?
                .list_models_runtime()
                .await);
        }
        */
        if context.location_kind == CliLocationKind::Ssh {
            let connection_id = context
                .ssh_connection_id
                .as_deref()
                .context("SSH 远端 Codex 未绑定连接")?;
            return remote_project_codex_runtime_service::model_infos(connection_id, None).await;
        }

        let engine = self.local_engine().await?;
        let cached_models = engine.runtime_model_fallback().await;
        if cached_models
            .iter()
            .any(|model| model.id == requested_model_id)
        {
            return Ok(cached_models);
        }
        Ok(engine.list_models_runtime().await)
    }

    async fn get_chat_provider_usage(
        &self,
        context: &CliExecutionContext,
    ) -> Result<Option<ChatProviderUsageDto>> {
        let workspace = self.load_workspace(context).await?;
        if context.location_kind == CliLocationKind::Ssh {
            let result = remote_project_codex_runtime_service::runtime(&workspace)
                .await?
                .usage_limits_snapshot()
                .await;
            return Ok(Some(map_provider_usage("codex", "Codex", result)));
        }

        let engine = self.local_engine().await?;
        Ok(Some(map_provider_usage(
            "codex",
            "Codex",
            engine.usage_limits_snapshot().await,
        )))
    }

    async fn engine_health(&self, context: &CliExecutionContext) -> Result<EngineHealthDto> {
        let workspace = self.load_workspace(context).await?;
        if context.location_kind == CliLocationKind::Local {
            let report = self.local_engine().await?.health_report().await;
            return Ok(EngineHealthDto {
                id: "codex".to_string(),
                available: report.available,
                version: report.version,
                details: report.details,
                warnings: report.warnings,
                checks: report.checks,
                fixes: report.fixes,
                protocol_diagnostics: report.protocol_diagnostics,
            });
        }

        let connection_id =
            remote_project_codex_runtime_service::validate_remote_codex_workspace(&workspace)?
                .to_string();
        let db = self.state.db.clone();
        let lookup_connection_id = connection_id.clone();
        let connection = tokio::task::spawn_blocking(move || {
            db::ssh_connections::find(&db, &lookup_connection_id)
        })
        .await
        .context("读取 SSH 连接任务失败")??
        .ok_or_else(|| anyhow::anyhow!("SSH 连接不存在: {connection_id}"))?;

        let mut protocol_diagnostics = None;
        let availability = match remote_project_codex_runtime_service::runtime(&workspace).await {
            Ok(engine) => {
                let models = engine.list_models_runtime().await;
                protocol_diagnostics = engine.protocol_diagnostics_snapshot().await;
                if models.is_empty() {
                    Err(anyhow::anyhow!("远端 Codex 模型目录为空"))
                } else {
                    Ok(())
                }
            }
            Err(error) => Err(error),
        };

        let version = if availability.is_ok() {
            let command = ssh::runtime::wrap_remote_login_shell_command("codex --version");
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
            id: "codex".to_string(),
            available: availability.is_ok(),
            version,
            details: Some(match availability {
                Ok(()) => format!("SSH 远端 Codex：{connection_name}"),
                Err(error) => format!("SSH 远端 Codex 不可用：{error:#}"),
            }),
            warnings: Vec::new(),
            checks: Vec::new(),
            fixes: Vec::new(),
            protocol_diagnostics,
        })
    }

    fn subscribe_codex_runtime_events(&self) -> broadcast::Receiver<CodexRuntimeEvent> {
        self.state.engines.subscribe_codex_runtime_events()
    }

    async fn prewarm_engine(&self, context: &CliExecutionContext) -> Result<()> {
        let workspace = self.load_workspace(context).await?;
        if context.location_kind == CliLocationKind::Ssh {
            let _ = remote_project_codex_runtime_service::runtime(&workspace)
                .await?
                .list_models_runtime()
                .await;
            Ok(())
        } else {
            self.local_engine().await?.prewarm().await
        }
    }

    async fn uses_external_sandbox(&self, context: &CliExecutionContext) -> Result<bool> {
        self.load_workspace(context).await?;
        if context.location_kind == CliLocationKind::Ssh {
            Ok(false)
        } else {
            Ok(self.local_engine().await?.uses_external_sandbox().await)
        }
    }

    async fn list_sessions(
        &self,
        context: &CliExecutionContext,
        search_term: Option<&str>,
        archived: Option<bool>,
    ) -> Result<Vec<CliSessionSnapshot>> {
        let workspace = self.load_workspace(context).await?;
        if context.location_kind == CliLocationKind::Ssh
            && search_term.is_none()
            && archived != Some(true)
        {
            // 旧实现先通过 Tunnel 的临时占用启动远端服务，再直接读取 Tunnel：
            // let service_use =
            //     remote_project_codex_runtime_service::acquire_temporary(&workspace).await?;
            let connection_id =
                remote_project_codex_runtime_service::validate_remote_codex_workspace(&workspace)?;
            let service = ssh::cli_service_lifecycle::get(connection_id, "codex").await?;
            let result = remote_project_session_refresh_service::list_codex_sessions(
                service.local_port(),
                &workspace.root_path,
            )
            .await;
            // service_use.release().await;
            let mut sessions = result?
                .into_iter()
                .map(|session| CliSessionSnapshot {
                    engine_thread_id: session.engine_thread_id,
                    title: session.title,
                    preview: None,
                    cwd: session.cwd,
                    model_id: session.model_id,
                    reasoning_effort: session
                        .metadata
                        .get("reasoningEffort")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string),
                    created_at: None,
                    updated_at: session.updated_at,
                    source_kind: None,
                    raw_status: Some(session.status.as_str().to_string()),
                    active_flags: Vec::new(),
                    status: session.status,
                    archived: false,
                    metadata: session.metadata,
                })
                .collect::<Vec<_>>();
            for session in &mut sessions {
                if session.model_id == "unknown" || session.reasoning_effort.is_none() {
                    let engine_thread_id = session.engine_thread_id.clone();
                    *session = self.read_session(context, &engine_thread_id).await?;
                }
            }
            return Ok(sessions);
        }
        let summaries = if context.location_kind == CliLocationKind::Ssh {
            remote_project_codex_runtime_service::runtime(&workspace)
                .await?
                .list_threads(search_term, archived)
                .await?
        } else {
            self.local_engine()
                .await?
                .list_threads(search_term, archived)
                .await?
        };

        let is_ssh = context.location_kind == CliLocationKind::Ssh;
        let mut sessions = summaries
            .into_iter()
            .filter(|session| path_utils::is_path_within_root(&session.cwd, &workspace.root_path))
            .map(|session| Self::map_session(session, is_ssh))
            .collect::<Vec<_>>();
        for session in &mut sessions {
            if session.model_id == "unknown" || session.reasoning_effort.is_none() {
                let engine_thread_id = session.engine_thread_id.clone();
                *session = self.read_session(context, &engine_thread_id).await?;
            }
        }
        Ok(sessions)
    }

    async fn read_session(
        &self,
        context: &CliExecutionContext,
        engine_thread_id: &str,
    ) -> Result<CliSessionSnapshot> {
        let workspace = self.load_workspace(context).await?;
        // 迁移留痕：旧逻辑直接用 `?` 返回全部错误，无法映射明确的 Codex NotFound；禁止恢复执行。
        // let summary = if context.location_kind == CliLocationKind::Ssh {
        //     remote_project_codex_runtime_service::runtime(&workspace)
        //         .await?
        //         .read_remote_thread(engine_thread_id)
        //         .await?
        // } else {
        //     self.state
        //         .engines
        //         .read_codex_remote_thread(engine_thread_id)
        //         .await?
        // };
        let summary_result = if context.location_kind == CliLocationKind::Ssh {
            remote_project_codex_runtime_service::runtime(&workspace)
                .await?
                .read_remote_thread(engine_thread_id)
                .await
        } else {
            self.local_engine()
                .await?
                .read_remote_thread(engine_thread_id)
                .await
        };
        let mut summary = match summary_result {
            Ok(summary) => summary,
            Err(error) => {
                // 只有 Codex app-server 实测的 -32600/thread not loaded
                // 才转换为公共 NotFound；服务、连接和协议错误原样上抛。
                if error.downcast_ref::<CodexThreadNotFoundError>().is_some() {
                    return Err(CliSessionNotFoundError::new("codex", engine_thread_id).into());
                }
                return Err(error);
            }
        };
        if context.location_kind == CliLocationKind::Ssh
            && (summary.model_id.is_none() || summary.reasoning_effort.is_none())
        {
            let (model_id, reasoning_effort) =
                remote_project_codex_runtime_service::runtime(&workspace)
                    .await?
                    .read_thread_runtime(engine_thread_id)
                    .await?;
            if summary.model_id.is_none() {
                summary.model_id = model_id;
            }
            if summary.reasoning_effort.is_none() {
                summary.reasoning_effort = reasoning_effort;
            }
        }
        anyhow::ensure!(
            path_utils::is_path_within_root(&summary.cwd, &workspace.root_path),
            "Codex 会话不属于当前 workspace"
        );
        Ok(Self::map_session(
            summary,
            context.location_kind == CliLocationKind::Ssh,
        ))
    }

    async fn acquire_turn(&self, context: &CliExecutionContext, thread: &ThreadDto) -> Result<()> {
        let workspace = self.load_workspace(context).await?;
        if context.location_kind == CliLocationKind::Ssh {
            *self.remote_turn_use.lock().await =
                Some(remote_project_codex_runtime_service::runtime(&workspace).await?);
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
            let remote_turn_use = self.remote_turn_use.lock().await;
            let engine = remote_turn_use
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("当前 SSH 远端 Codex 会话尚未建立持续使用关系"))?;
            return Engine::start_thread(
                engine.as_ref(),
                scope,
                resume_engine_thread_id,
                model,
                sandbox,
            )
            .await;
        }

        let engine = self.configure_local_computer_control().await?;
        Engine::start_thread(
            engine.as_ref(),
            scope,
            thread.engine_thread_id.as_deref(),
            model,
            sandbox,
        )
        .await
    }

    async fn send_message(
        &self,
        context: &CliExecutionContext,
        _thread: &ThreadDto,
        engine_thread_id: &str,
        input: TurnInput,
        event_tx: mpsc::Sender<EngineEvent>,
        cancellation: CancellationToken,
    ) -> Result<()> {
        self.load_workspace(context).await?;
        if context.location_kind == CliLocationKind::Ssh {
            let engine =
                self.remote_turn_use.lock().await.take().ok_or_else(|| {
                    anyhow::anyhow!("当前 SSH 远端 Codex 会话尚未建立持续使用关系")
                })?;
            let result = Engine::send_message(
                engine.as_ref(),
                engine_thread_id,
                input,
                event_tx,
                cancellation,
            )
            .await;
            return result;
        }

        let engine = self.configure_local_computer_control().await?;
        Engine::send_message(
            engine.as_ref(),
            engine_thread_id,
            input,
            event_tx,
            cancellation,
        )
        .await
    }

    async fn steer_message(
        &self,
        context: &CliExecutionContext,
        _thread: &ThreadDto,
        engine_thread_id: &str,
        client_steer_id: &str,
        content: &str,
        input: TurnInput,
    ) -> Result<EngineSteerReceipt> {
        let workspace = self.load_workspace(context).await?;
        if context.location_kind == CliLocationKind::Ssh {
            let engine = remote_project_codex_runtime_service::runtime(&workspace).await?;
            Engine::steer_message(
                engine.as_ref(),
                engine_thread_id,
                client_steer_id,
                content,
                input,
            )
            .await
        } else {
            let engine = self.configure_local_computer_control().await?;
            Engine::steer_message(
                engine.as_ref(),
                engine_thread_id,
                client_steer_id,
                content,
                input,
            )
            .await
        }
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
            let engine = remote_project_codex_runtime_service::runtime(&workspace).await?;
            Engine::respond_to_approval(engine.as_ref(), approval_id, response, route)
                .await
                .with_context(|| format!("SSH 远端 Codex 审批回复失败: thread_id={}", thread.id))
        } else {
            let engine = self.local_engine().await?;
            Engine::respond_to_approval(engine.as_ref(), approval_id, response, route).await
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
            let Some(engine_thread_id) = thread.engine_thread_id.as_deref() else {
                return Ok(());
            };
            let engine = remote_project_codex_runtime_service::runtime(&workspace).await?;
            Engine::interrupt(engine.as_ref(), engine_thread_id)
                .await
                .with_context(|| format!("SSH 远端 Codex 取消失败: thread_id={}", thread.id))
        } else {
            let engine_thread_id = thread.engine_thread_id.as_deref().unwrap_or("default");
            Engine::interrupt(self.local_engine().await?.as_ref(), engine_thread_id).await
        }
    }

    async fn archive_thread(
        &self,
        context: &CliExecutionContext,
        _thread: &ThreadDto,
        engine_thread_id: &str,
    ) -> Result<()> {
        let workspace = self.load_workspace(context).await?;
        if context.location_kind == CliLocationKind::Ssh {
            let engine = remote_project_codex_runtime_service::runtime(&workspace).await?;
            Engine::archive_thread(engine.as_ref(), engine_thread_id).await
        } else {
            let engine = self.local_engine().await?;
            match Engine::archive_thread(engine.as_ref(), engine_thread_id).await {
                Ok(()) => Ok(()),
                Err(error) => {
                    let archived = match engine.list_threads(None, Some(true)).await {
                        Ok(sessions) => sessions
                            .into_iter()
                            .any(|session| session.engine_thread_id == engine_thread_id),
                        Err(_) => return Err(error),
                    };
                    if archived {
                        return Ok(());
                    }
                    let active = match engine.list_threads(None, Some(false)).await {
                        Ok(sessions) => sessions
                            .into_iter()
                            .any(|session| session.engine_thread_id == engine_thread_id),
                        Err(_) => return Err(error),
                    };
                    if active {
                        Err(error)
                    } else {
                        Ok(())
                    }
                }
            }
        }
    }

    async fn unarchive_thread(
        &self,
        context: &CliExecutionContext,
        _thread: &ThreadDto,
        engine_thread_id: &str,
    ) -> Result<()> {
        let workspace = self.load_workspace(context).await?;
        if context.location_kind == CliLocationKind::Ssh {
            let engine = remote_project_codex_runtime_service::runtime(&workspace).await?;
            Engine::unarchive_thread(engine.as_ref(), engine_thread_id).await
        } else {
            Engine::unarchive_thread(self.local_engine().await?.as_ref(), engine_thread_id).await
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
        _thread: &ThreadDto,
        engine_thread_id: &str,
    ) -> Result<Option<String>> {
        let workspace = self.load_workspace(context).await?;
        if context.location_kind == CliLocationKind::Ssh {
            let preview = remote_project_codex_runtime_service::runtime(&workspace)
                .await?
                .read_thread_preview(engine_thread_id)
                .await;
            Ok(preview)
        } else {
            Ok(self
                .local_engine()
                .await?
                .read_thread_preview(engine_thread_id)
                .await)
        }
    }

    async fn read_thread_sync_snapshot(
        &self,
        context: &CliExecutionContext,
        _thread: &ThreadDto,
        engine_thread_id: &str,
    ) -> Result<Option<ThreadSyncSnapshot>> {
        let workspace = self.load_workspace(context).await?;
        if context.location_kind == CliLocationKind::Ssh {
            return remote_project_codex_runtime_service::runtime(&workspace)
                .await?
                .read_thread_sync_snapshot(engine_thread_id)
                .await
                .map(Some);
        }

        self.local_engine()
            .await?
            .read_thread_sync_snapshot(engine_thread_id)
            .await
            .map(Some)
    }

    async fn set_thread_name(
        &self,
        context: &CliExecutionContext,
        _thread: &ThreadDto,
        engine_thread_id: &str,
        name: &str,
    ) -> Result<()> {
        let workspace = self.load_workspace(context).await?;
        if context.location_kind == CliLocationKind::Ssh {
            remote_project_codex_runtime_service::runtime(&workspace)
                .await?
                .set_thread_name(engine_thread_id, name)
                .await
        } else {
            self.local_engine()
                .await?
                .set_thread_name(engine_thread_id, name)
                .await
        }
    }

    async fn list_codex_skills(
        &self,
        context: &CliExecutionContext,
        cwd: &str,
    ) -> Result<Vec<CodexSkillDto>> {
        let workspace = self.load_workspace(context).await?;
        if context.location_kind == CliLocationKind::Ssh {
            return remote_project_codex_runtime_service::runtime(&workspace)
                .await?
                .list_skills(&workspace.root_path)
                .await;
        }
        self.local_engine().await?.list_skills(cwd).await
    }

    async fn list_codex_apps(&self, context: &CliExecutionContext) -> Result<Vec<CodexAppDto>> {
        let workspace = self.load_workspace(context).await?;
        if context.location_kind == CliLocationKind::Ssh {
            return remote_project_codex_runtime_service::runtime(&workspace)
                .await?
                .list_apps()
                .await;
        }
        self.local_engine().await?.list_apps().await
    }

    async fn list_codex_plugins(
        &self,
        context: &CliExecutionContext,
        cwd: &str,
    ) -> Result<Vec<CodexPluginDto>> {
        let workspace = self.load_workspace(context).await?;
        if context.location_kind == CliLocationKind::Ssh {
            return remote_project_codex_runtime_service::runtime(&workspace)
                .await?
                .list_plugins(&workspace.root_path)
                .await;
        }
        self.local_engine().await?.list_plugins(cwd).await
    }

    async fn get_opencode_runtime_catalog(
        &self,
        context: &CliExecutionContext,
        _cwd: &str,
    ) -> Result<OpenCodeRuntimeCatalogDto> {
        self.load_workspace(context).await?;
        Err(anyhow::anyhow!("Codex 不支持 OpenCode 参数"))
    }

    async fn refresh_extension_catalog(
        &self,
        context: &CliExecutionContext,
        cwd: Option<&str>,
        requested_kinds: &[String],
    ) -> Result<Vec<ExtensionCatalogKindRefreshDto>> {
        self.load_workspace(context).await?;
        if context.location_kind == CliLocationKind::Local {
            let mut results = Vec::new();
            for kind in requested_kinds {
                results.push(
                    crate::extensions::codex::refresh_kind(self.state.engines.as_ref(), cwd, kind)
                        .await,
                );
            }
            return Ok(results);
        }

        let catalog = self.get_extension_catalog(context, cwd).await?;
        Ok(requested_kinds
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
            .collect())
    }

    async fn get_extension_catalog(
        &self,
        context: &CliExecutionContext,
        cwd: Option<&str>,
    ) -> Result<CachedExtensionCatalogDto> {
        let workspace = self.load_workspace(context).await?;
        if context.location_kind == CliLocationKind::Local {
            return extensions::refresh::load_cached_catalog(
                &self.state,
                "codex",
                cwd.or(Some(workspace.root_path.as_str())),
            )
            .await;
        }

        let engine = remote_project_codex_runtime_service::runtime(&workspace).await?;
        let skills_result = engine.list_skills(&workspace.root_path).await;
        let plugins_result = engine.list_plugins(&workspace.root_path).await;
        let diagnostics = engine.protocol_diagnostics_snapshot().await;
        let skills = skills_result?;
        let plugins = plugins_result?;
        let mcp_servers = diagnostics
            .map(|value| value.mcp_servers)
            .unwrap_or_default();

        let mut items = skills
            .into_iter()
            .map(|skill| ExtensionItemDto {
                id: skill.path.clone(),
                provider_id: "codex".to_string(),
                kind: "skill".to_string(),
                name: skill.name,
                description: (!skill.description.trim().is_empty()).then_some(skill.description),
                version: None,
                scope: skill.scope.clone(),
                source: (!skill.scope.trim().is_empty()).then_some(skill.scope),
                marketplace: None,
                path: Some(skill.path),
                parent_plugin_id: None,
                category: None,
                officially_available: false,
                catalog_authority: None,
                installed: Some(true),
                configured: None,
                enabled: Some(skill.enabled),
                health: if skill.enabled { "healthy" } else { "unknown" }.to_string(),
                auth_state: None,
                available_actions: Vec::new(),
                requires_new_session: false,
                read_only_reason: Some("ssh_remote_codex_extension_action".to_string()),
                warning: None,

                ..Default::default()
            })
            .collect::<Vec<_>>();
        items.extend(plugins.into_iter().map(|plugin| ExtensionItemDto {
            id: plugin.id,
            provider_id: "codex".to_string(),
            kind: "plugin".to_string(),
            name: plugin.name,
            description: plugin.description,
            version: None,
            scope: "user".to_string(),
            source: plugin.developer_name,
            marketplace: None,
            path: None,
            parent_plugin_id: None,
            category: None,
            officially_available: false,
            catalog_authority: None,
            installed: Some(plugin.installed),
            configured: None,
            enabled: Some(plugin.enabled),
            health: if plugin.enabled { "healthy" } else { "unknown" }.to_string(),
            auth_state: None,
            available_actions: Vec::new(),
            requires_new_session: false,
            read_only_reason: Some("ssh_remote_codex_extension_action".to_string()),
            warning: None,

            ..Default::default()
        }));
        items.extend(mcp_servers.into_iter().map(|server| ExtensionItemDto {
            id: server.name.clone(),
            provider_id: "codex".to_string(),
            kind: "mcp".to_string(),
            name: server.name,
            description: Some(format!(
                "{} tools · {} resources · {} resource templates",
                server.tool_count, server.resource_count, server.resource_template_count
            )),
            version: None,
            scope: "user".to_string(),
            source: None,
            marketplace: None,
            path: None,
            parent_plugin_id: None,
            category: None,
            officially_available: false,
            catalog_authority: None,
            installed: None,
            configured: Some(true),
            enabled: Some(true),
            health: "healthy".to_string(),
            auth_state: Some(server.auth_status),
            available_actions: Vec::new(),
            requires_new_session: false,
            read_only_reason: Some("ssh_remote_codex_extension_action".to_string()),
            warning: None,

            ..Default::default()
        }));
        let fetched_at = chrono::Utc::now().to_rfc3339();
        let kind_fetched_at = ["skill", "plugin", "mcp"]
            .into_iter()
            .map(|kind| (kind.to_string(), Some(fetched_at.clone())))
            .collect();

        Ok(CachedExtensionCatalogDto {
            provider_id: "codex".to_string(),
            cwd: Some(workspace.root_path),
            items,
            sources: Vec::new(),
            capabilities: extensions::provider_capabilities("codex"),
            fetched_at: Some(fetched_at.clone()),
            kind_fetched_at,
            last_attempt_at: Some(fetched_at.clone()),
            next_refresh_at: None,
            refreshing: false,
            refresh_completed_at: Some(fetched_at),
            has_snapshot: true,
            refresh_errors: Vec::new(),
        })
    }

    async fn get_extensions(&self, context: &CliExecutionContext) -> Result<Vec<ExtensionItemDto>> {
        let catalog = self.get_extension_catalog(context, None).await?;
        let mut items = catalog.items;
        for item in &mut items {
            match item.kind.as_str() {
                "skill" => {
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
        let builtin_ids = [
            "review",
            "fork",
            "rollback",
            "compact",
            "fast",
            "personality",
            "experimental",
        ];
        items.extend(builtin_ids.into_iter().map(|id| ExtensionItemDto {
            id: id.to_string(),
            provider_id: "codex".to_string(),
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
            "SSH 远端 Codex 当前不执行扩展变更，也不会调用本机 Codex"
        );
        let _ = scope;
        crate::extensions::codex::perform_action(&item, action, Some(workspace.root_path.as_str()))
            .await
    }

    async fn fork_thread(
        &self,
        context: &CliExecutionContext,
        engine_thread_id: &str,
        cwd: &str,
        model: &str,
        sandbox: SandboxPolicy,
    ) -> Result<CliForkedThread> {
        self.load_workspace(context).await?;
        anyhow::ensure!(
            context.location_kind == CliLocationKind::Local,
            "SSH 远端 Codex 暂未接入会话分支，当前不会调用本机 Codex 执行"
        );
        let forked = self
            .local_engine()
            .await?
            .fork_thread(engine_thread_id, cwd, model, sandbox)
            .await?;
        Ok(CliForkedThread {
            engine_thread_id: forked.engine_thread_id,
            model_id: forked.model_id,
            title: forked.title,
            preview: forked.preview,
            raw_status: forked.raw_status,
            active_flags: forked.active_flags,
        })
    }

    async fn rollback_thread(
        &self,
        context: &CliExecutionContext,
        engine_thread_id: &str,
        num_turns: u32,
    ) -> Result<ThreadSyncSnapshot> {
        self.load_workspace(context).await?;
        anyhow::ensure!(
            context.location_kind == CliLocationKind::Local,
            "SSH 远端 Codex 暂未接入回滚，当前不会调用本机 Codex 执行"
        );
        self.local_engine()
            .await?
            .rollback_thread(engine_thread_id, num_turns)
            .await
    }

    async fn compact_thread(
        &self,
        context: &CliExecutionContext,
        engine_thread_id: &str,
    ) -> Result<()> {
        self.load_workspace(context).await?;
        anyhow::ensure!(
            context.location_kind == CliLocationKind::Local,
            "SSH 远端 Codex 暂未接入压缩，当前不会调用本机 Codex 执行"
        );
        self.local_engine()
            .await?
            .compact_thread(engine_thread_id)
            .await
    }

    async fn start_review(
        &self,
        context: &CliExecutionContext,
        source_engine_thread_id: &str,
        target: Value,
        delivery: Option<&str>,
        event_tx: mpsc::Sender<EngineEvent>,
        cancellation: CancellationToken,
        started_tx: oneshot::Sender<CliReviewStarted>,
    ) -> Result<()> {
        self.load_workspace(context).await?;
        anyhow::ensure!(
            context.location_kind == CliLocationKind::Local,
            "SSH 远端 Codex 暂未接入代码审查，当前不会调用本机 Codex 执行"
        );
        let (codex_started_tx, codex_started_rx) = oneshot::channel::<CodexReviewStarted>();
        let forward_started = tokio::spawn(async move {
            let started = codex_started_rx.await?;
            started_tx
                .send(CliReviewStarted {
                    review_thread_id: started.review_thread_id,
                })
                .map_err(|_| anyhow::anyhow!("代码审查会话接收方已关闭"))?;
            Ok::<(), anyhow::Error>(())
        });
        self.local_engine()
            .await?
            .start_review(
                source_engine_thread_id,
                target,
                delivery,
                event_tx,
                cancellation,
                codex_started_tx,
            )
            .await?;
        forward_started.await.context("等待代码审查会话失败")??;
        Ok(())
    }
}
