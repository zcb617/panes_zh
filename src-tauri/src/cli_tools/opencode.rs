use std::{collections::HashSet, sync::Arc};

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
        capabilities_for_engine, map_engine_capabilities, map_model_info, opencode::OpenCodeEngine,
        ApprovalRequestRoute, CodexRuntimeEvent, Engine, EngineCapabilities, EngineEvent,
        EngineSteerReceipt, EngineThread, ModelInfo, OpenCodeRemoteSessionSummary, SandboxPolicy,
        ThreadScope, ThreadSyncSnapshot, TurnInput,
    },
    extensions,
    models::{
        CachedExtensionCatalogDto, ChatProviderUsageDto, CodexAppDto, CodexPluginDto,
        CodexSkillDto, EngineHealthDto, EngineInfoDto, ExtensionActionResultDto,
        ExtensionCatalogKindRefreshDto, ExtensionItemDto, OpenCodeRuntimeCatalogDto, ThreadDto,
        ThreadStatusDto, WorkspaceDto,
    },
    path_utils, remote_project_opencode_runtime_service, ssh,
    state::AppState,
};

/// OpenCode 对统一 CLI 操作接口的实现。
///
/// 本机项目继续使用现有 OpenCode 引擎；SSH 项目继续使用现有 OpenCode 服务和
/// tunnel 生命周期。远端操作失败时直接返回错误，不会改用本机 OpenCode。
#[derive(Clone)]
pub struct OpenCodeCli {
    state: AppState,
    remote_turn_use: Arc<Mutex<Option<Arc<OpenCodeEngine>>>>,
}

impl OpenCodeCli {
    pub fn new(state: AppState) -> Self {
        Self {
            state,
            remote_turn_use: Arc::new(Mutex::new(None)),
        }
    }

    fn configure_local_computer_control(&self) {
        self.state
            .engines
            .set_local_opencode_computer_control_service(
                self.state.computer_control_service.clone(),
            );
    }

    /// 根据 workspace 建立 OpenCode 操作目标。
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
        .context("读取 OpenCode workspace 任务失败")??;
        CliExecutionContext::from_workspace(&workspace)
    }

    /// 根据项目目录找到所属 workspace，供 OpenCode 参数和扩展查询使用。
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
        .context("按项目目录读取 OpenCode workspace 任务失败")??;
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
            "当前 workspace 的项目目录与 OpenCode 操作目标不一致"
        );

        match context.location_kind {
            CliLocationKind::Local => {
                anyhow::ensure!(
                    workspace.location_kind != "ssh",
                    "当前 workspace 是 SSH 远端项目，不能使用本机 OpenCode"
                );
            }
            CliLocationKind::Ssh => {
                anyhow::ensure!(
                    workspace.location_kind == "ssh",
                    "当前 workspace 不是 SSH 远端项目"
                );
                anyhow::ensure!(
                    workspace.ssh_connection_id == context.ssh_connection_id,
                    "当前 workspace 的 SSH 绑定与 OpenCode 操作目标不一致"
                );
            }
        }

        Ok(workspace)
    }

    async fn workspace_roots(&self, workspace: &WorkspaceDto) -> Result<Vec<String>> {
        let db = self.state.db.clone();
        let workspace_id = workspace.id.clone();
        let repos = tokio::task::spawn_blocking(move || db::repos::get_repos(&db, &workspace_id))
            .await
            .context("读取 OpenCode workspace 仓库任务失败")??;
        let mut roots = vec![workspace.root_path.clone()];
        for repo in repos {
            if !roots
                .iter()
                .any(|root| path_utils::paths_equal(root, &repo.path))
            {
                roots.push(repo.path);
            }
        }
        Ok(roots)
    }

    async fn resolve_workspace_cwd(
        &self,
        workspace: &WorkspaceDto,
        cwd: Option<&str>,
    ) -> Result<String> {
        let requested = cwd
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(workspace.root_path.as_str());
        let roots = self.workspace_roots(workspace).await?;
        roots
            .into_iter()
            .find(|root| path_utils::paths_equal(root, requested))
            .ok_or_else(|| anyhow::anyhow!("OpenCode 项目目录不属于当前 workspace: {requested}"))
    }

    async fn thread_cwd(&self, workspace: &WorkspaceDto, thread: &ThreadDto) -> Result<String> {
        let Some(repo_id) = thread.repo_id.as_deref() else {
            if let Some(remote_cwd) = thread
                .engine_metadata
                .as_ref()
                .and_then(|metadata| metadata.get("opencodeRemoteCwd"))
                .and_then(Value::as_str)
            {
                return self
                    .resolve_workspace_cwd(workspace, Some(remote_cwd))
                    .await;
            }
            return Ok(workspace.root_path.clone());
        };
        let db = self.state.db.clone();
        let repo_id = repo_id.to_string();
        let lookup_repo_id = repo_id.clone();
        let repo =
            tokio::task::spawn_blocking(move || db::repos::find_repo_by_id(&db, &lookup_repo_id))
                .await
                .context("读取 OpenCode 会话仓库任务失败")??
                .ok_or_else(|| anyhow::anyhow!("OpenCode 会话仓库不存在: {repo_id}"))?;
        anyhow::ensure!(
            repo.workspace_id == workspace.id,
            "OpenCode 会话仓库不属于当前 workspace"
        );
        self.resolve_workspace_cwd(workspace, Some(repo.path.as_str()))
            .await
    }

    async fn list_workspace_sessions(
        &self,
        context: &CliExecutionContext,
        workspace: &WorkspaceDto,
        search_term: Option<&str>,
        archived: Option<bool>,
    ) -> Result<Vec<OpenCodeRemoteSessionSummary>> {
        let roots = self.workspace_roots(workspace).await?;
        let mut sessions = Vec::new();
        if context.location_kind == CliLocationKind::Ssh {
            // 旧实现先取得 Tunnel 临时占用；现在远端服务端由 cli_service_lifecycle
            // 常驻管理，CLI 实现只创建自己的 OpenCode 客户端。
            // let service_use =
            //     remote_project_opencode_runtime_service::acquire_temporary(workspace).await?;
            let engine = remote_project_opencode_runtime_service::runtime(workspace).await?;
            let result = async {
                for cwd in roots.iter() {
                    sessions.extend(engine.list_sessions(cwd, search_term, archived).await?);
                }
                Ok::<_, anyhow::Error>(())
            }
            .await;
            // service_use.release().await;
            result?;
        } else {
            for cwd in roots.iter() {
                sessions.extend(
                    self.state
                        .engines
                        .list_opencode_remote_sessions(cwd, search_term, archived)
                        .await?,
                );
            }
        }

        sessions.retain(|session| {
            roots
                .iter()
                .any(|root| path_utils::paths_equal(root, &session.cwd))
        });
        sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        let mut seen = HashSet::new();
        sessions.retain(|session| seen.insert(session.engine_thread_id.clone()));
        Ok(sessions)
    }

    fn map_session(summary: OpenCodeRemoteSessionSummary, is_ssh: bool) -> CliSessionSnapshot {
        let timestamp_to_rfc3339 = |timestamp: i64| {
            let (seconds, nanos) = if timestamp > 10_000_000_000 {
                (timestamp / 1000, ((timestamp % 1000) as u32) * 1_000_000)
            } else {
                (timestamp, 0)
            };
            chrono::DateTime::from_timestamp(seconds, nanos).map(|value| value.to_rfc3339())
        };
        let title = summary
            .title
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| {
                format!(
                    "OpenCode session {}",
                    summary.engine_thread_id.chars().take(8).collect::<String>()
                )
            });
        let metadata = json!({
            "sshRemote": is_ssh,
            "opencodeRemoteCwd": summary.cwd.clone(),
            "opencodeRemote": {
                "id": summary.engine_thread_id.clone(),
                "title": summary.title.clone(),
                "cwd": summary.cwd.clone(),
                "archived": summary.archived,
                "createdAt": summary.created_at,
                "updatedAt": summary.updated_at,
            },
        });

        CliSessionSnapshot {
            engine_thread_id: summary.engine_thread_id,
            title,
            preview: None,
            cwd: summary.cwd,
            model_id: "unknown".to_string(),
            created_at: timestamp_to_rfc3339(summary.created_at),
            updated_at: timestamp_to_rfc3339(summary.updated_at),
            source_kind: None,
            raw_status: None,
            active_flags: Vec::new(),
            status: ThreadStatusDto::Idle,
            archived: summary.archived,
            metadata,
        }
    }

    fn validate_thread(context: &CliExecutionContext, thread: &ThreadDto) -> Result<()> {
        anyhow::ensure!(
            thread.workspace_id == context.workspace_id,
            "OpenCode 会话不属于当前 workspace"
        );
        anyhow::ensure!(thread.engine_id == "opencode", "当前会话不属于 OpenCode");
        Ok(())
    }

    fn unsupported(action: &str) -> anyhow::Error {
        anyhow::anyhow!("OpenCode 当前不支持{action}")
    }
}

#[async_trait]
impl CliTool for OpenCodeCli {
    fn id(&self) -> &str {
        "opencode"
    }

    fn name(&self) -> &str {
        "OpenCode"
    }

    fn capabilities(&self) -> EngineCapabilities {
        capabilities_for_engine("opencode")
    }

    async fn execution_context(&self, workspace_id: Option<&str>) -> Result<CliExecutionContext> {
        OpenCodeCli::execution_context(self, workspace_id).await
    }

    async fn execution_context_for_cwd(&self, cwd: Option<&str>) -> Result<CliExecutionContext> {
        OpenCodeCli::execution_context_for_cwd(self, cwd).await
    }

    async fn get_engine_info(&self, context: &CliExecutionContext) -> Result<EngineInfoDto> {
        let workspace = self.load_workspace(context).await?;
        if context.location_kind == CliLocationKind::Ssh {
            let models = remote_project_opencode_runtime_service::runtime(&workspace)
                .await?
                .list_models_runtime_for_cwd(&workspace.root_path)
                .await;
            anyhow::ensure!(!models.is_empty(), "SSH OpenCode 未返回可用模型");
            return Ok(EngineInfoDto {
                id: "opencode".to_string(),
                name: "OpenCode".to_string(),
                models: models.into_iter().map(map_model_info).collect(),
                capabilities: map_engine_capabilities(capabilities_for_engine("opencode")),
            });
        }

        let models = self
            .state
            .engines
            .models_for_validation("opencode", "")
            .await?;
        Ok(EngineInfoDto {
            id: "opencode".to_string(),
            name: "OpenCode".to_string(),
            models: models.into_iter().map(map_model_info).collect(),
            capabilities: map_engine_capabilities(capabilities_for_engine("opencode")),
        })
    }

    async fn models_for_validation(
        &self,
        context: &CliExecutionContext,
        requested_model_id: &str,
    ) -> Result<Vec<ModelInfo>> {
        let workspace = self.load_workspace(context).await?;
        if context.location_kind == CliLocationKind::Ssh {
            let models = remote_project_opencode_runtime_service::runtime(&workspace)
                .await?
                .list_models_runtime_for_cwd(&workspace.root_path)
                .await;
            anyhow::ensure!(!models.is_empty(), "SSH OpenCode 未返回可用模型");
            return Ok(models);
        }
        self.state
            .engines
            .models_for_validation("opencode", requested_model_id)
            .await
    }

    async fn get_chat_provider_usage(
        &self,
        context: &CliExecutionContext,
    ) -> Result<Option<ChatProviderUsageDto>> {
        self.load_workspace(context).await?;
        Ok(None)
    }

    async fn engine_health(&self, context: &CliExecutionContext) -> Result<EngineHealthDto> {
        let workspace = self.load_workspace(context).await?;
        if context.location_kind == CliLocationKind::Local {
            return self.state.engines.health("opencode").await;
        }

        let connection_id =
            remote_project_opencode_runtime_service::validate_remote_opencode_workspace(
                &workspace,
            )?
            .to_string();
        let db = self.state.db.clone();
        let lookup_connection_id = connection_id.clone();
        let connection = tokio::task::spawn_blocking(move || {
            db::ssh_connections::find(&db, &lookup_connection_id)
        })
        .await
        .context("读取 SSH 连接任务失败")??
        .ok_or_else(|| anyhow::anyhow!("SSH 连接不存在: {connection_id}"))?;
        let availability = match remote_project_opencode_runtime_service::runtime(&workspace).await
        {
            Ok(engine) => engine.prewarm().await,
            Err(error) => Err(error),
        };
        let version = if availability.is_ok() {
            let command = ssh::runtime::wrap_remote_login_shell_command("opencode --version");
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
            id: "opencode".to_string(),
            available: availability.is_ok(),
            version,
            details: Some(match availability {
                Ok(()) => format!("SSH 远端 OpenCode：{connection_name}"),
                Err(error) => format!("SSH 远端 OpenCode 不可用：{error:#}"),
            }),
            warnings: Vec::new(),
            checks: Vec::new(),
            fixes: Vec::new(),
            protocol_diagnostics: None,
        })
    }

    fn subscribe_codex_runtime_events(&self) -> broadcast::Receiver<CodexRuntimeEvent> {
        let (_event_tx, event_rx) = broadcast::channel(1);
        event_rx
    }

    async fn prewarm_engine(&self, context: &CliExecutionContext) -> Result<()> {
        let workspace = self.load_workspace(context).await?;
        if context.location_kind == CliLocationKind::Ssh {
            remote_project_opencode_runtime_service::runtime(&workspace)
                .await?
                .prewarm()
                .await
        } else {
            self.state.engines.prewarm("opencode").await
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
        let summaries = self
            .list_workspace_sessions(context, &workspace, search_term, archived)
            .await?;
        let is_ssh = context.location_kind == CliLocationKind::Ssh;
        Ok(summaries
            .into_iter()
            .map(|summary| Self::map_session(summary, is_ssh))
            .collect())
    }

    async fn read_session(
        &self,
        context: &CliExecutionContext,
        engine_thread_id: &str,
    ) -> Result<CliSessionSnapshot> {
        let workspace = self.load_workspace(context).await?;
        // 旧实现仅作架构迁移留痕，禁止恢复执行：
        // let summary = self
        //     .list_workspace_sessions(context, &workspace, None, None)
        //     .await?
        //     .into_iter()
        //     .find(|session| session.engine_thread_id == engine_thread_id)
        //     .ok_or_else(|| {
        //         anyhow::anyhow!(
        //             "OpenCode 会话不属于当前 workspace 或已不存在: {engine_thread_id}"
        //         )
        //     })?;
        // SSH 只能通过 CLI Service Lifecycle 取得 OpenCode 客户端，并且只发一次按 ID 请求。
        if context.location_kind != CliLocationKind::Ssh {
            // 本机 OpenCode 允许 workspace 根目录和各仓库目录分别拥有会话；只有明确的
            // 404 才继续尝试下一个目录，其他错误必须原样返回，不能误报为“会话不存在”。
            let roots = self.workspace_roots(&workspace).await?;
            for cwd in roots.iter() {
                match self
                    .state
                    .engines
                    .read_opencode_remote_session(cwd, engine_thread_id)
                    .await
                {
                    Ok(summary) => {
                        anyhow::ensure!(
                            summary.engine_thread_id == engine_thread_id,
                            "OpenCode 返回了错误的会话 ID: expected={} actual={}",
                            engine_thread_id,
                            summary.engine_thread_id
                        );
                        anyhow::ensure!(
                            roots
                                .iter()
                                .any(|root| path_utils::paths_equal(root, &summary.cwd)),
                            "OpenCode 会话目录不属于当前 workspace: {}",
                            summary.cwd
                        );
                        return Ok(Self::map_session(summary, false));
                    }
                    Err(error) => {
                        let is_not_found = error
                            .downcast_ref::<reqwest::Error>()
                            .and_then(|cause| cause.status())
                            .is_some_and(|status| status == reqwest::StatusCode::NOT_FOUND);
                        if !is_not_found {
                            return Err(error);
                        }
                    }
                }
            }
            // 所有候选目录均确认返回 404，交给公共恢复编排识别为会话不存在。
            // 迁移留痕：旧实现把“未找到”作为普通业务错误返回，恢复编排无法区分 404：
            // anyhow::bail!("OpenCode 会话不属于当前 workspace 或已不存在: {engine_thread_id}");
            return Err(CliSessionNotFoundError::new("opencode", engine_thread_id).into());
        }

        // 迁移留痕：旧实现直接使用 `.await?`，会把 SSH 404 原样暴露给公共恢复编排：
        // let summary = remote_project_opencode_runtime_service::runtime(&workspace)
        //     .await?
        //     .read_session(&workspace.root_path, engine_thread_id)
        //     .await?;
        let summary = match remote_project_opencode_runtime_service::runtime(&workspace)
            .await?
            .read_session(&workspace.root_path, engine_thread_id)
            .await
        {
            Ok(summary) => summary,
            Err(error) => {
                // SSH 只允许这一次按 ID 请求；仅确认 HTTP 404 时映射公共 NotFound，
                // 连接、解析和其他 HTTP 错误必须原样返回，不能回退到本机或列表查询。
                let is_not_found = error
                    .downcast_ref::<reqwest::Error>()
                    .and_then(|cause| cause.status())
                    .is_some_and(|status| status == reqwest::StatusCode::NOT_FOUND);
                if is_not_found {
                    return Err(CliSessionNotFoundError::new("opencode", engine_thread_id).into());
                }
                return Err(error);
            }
        };
        anyhow::ensure!(
            summary.engine_thread_id == engine_thread_id,
            "OpenCode 返回了错误的会话 ID: expected={} actual={}",
            engine_thread_id,
            summary.engine_thread_id
        );
        let roots = self.workspace_roots(&workspace).await?;
        anyhow::ensure!(
            roots
                .iter()
                .any(|root| path_utils::paths_equal(root, &summary.cwd)),
            "OpenCode 会话目录不属于当前 workspace: {}",
            summary.cwd
        );
        Ok(Self::map_session(
            summary,
            context.location_kind == CliLocationKind::Ssh,
        ))
    }

    async fn acquire_turn(&self, context: &CliExecutionContext, thread: &ThreadDto) -> Result<()> {
        let workspace = self.load_workspace(context).await?;
        Self::validate_thread(context, thread)?;
        if context.location_kind == CliLocationKind::Ssh {
            *self.remote_turn_use.lock().await =
                Some(remote_project_opencode_runtime_service::runtime(&workspace).await?);
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
        let workspace = self.load_workspace(context).await?;
        Self::validate_thread(context, thread)?;
        let scope_cwd = match &scope {
            ThreadScope::Repo { repo_path } => repo_path.as_str(),
            ThreadScope::Workspace { root_path, .. } => root_path.as_str(),
        };
        self.resolve_workspace_cwd(&workspace, Some(scope_cwd))
            .await?;
        if context.location_kind == CliLocationKind::Ssh {
            let remote_turn_use = self.remote_turn_use.lock().await;
            let engine = remote_turn_use.as_ref().ok_or_else(|| {
                anyhow::anyhow!("当前 SSH 远端 OpenCode 会话尚未建立持续使用关系")
            })?;
            return Engine::start_thread(
                engine.as_ref(),
                scope,
                resume_engine_thread_id,
                model,
                sandbox,
            )
            .await;
        }

        self.configure_local_computer_control();
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
        Self::validate_thread(context, thread)?;
        if context.location_kind == CliLocationKind::Ssh {
            let engine = self.remote_turn_use.lock().await.take().ok_or_else(|| {
                anyhow::anyhow!("当前 SSH 远端 OpenCode 会话尚未建立持续使用关系")
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

        self.configure_local_computer_control();
        self.state
            .engines
            .send_message(thread, engine_thread_id, input, event_tx, cancellation)
            .await
    }

    async fn steer_message(
        &self,
        context: &CliExecutionContext,
        thread: &ThreadDto,
        _engine_thread_id: &str,
        _client_steer_id: &str,
        _content: &str,
        _input: TurnInput,
    ) -> Result<EngineSteerReceipt> {
        self.load_workspace(context).await?;
        Self::validate_thread(context, thread)?;
        Err(Self::unsupported("运行中补充消息"))
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
        Self::validate_thread(context, thread)?;
        if context.location_kind == CliLocationKind::Ssh {
            let engine = remote_project_opencode_runtime_service::runtime(&workspace).await?;
            Engine::respond_to_approval(engine.as_ref(), approval_id, response, route)
                .await
                .with_context(|| {
                    format!(
                        "SSH 远端 OpenCode 审批或问题回复失败: thread_id={}",
                        thread.id
                    )
                })
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
        engine_thread_id: &str,
    ) -> Result<()> {
        let workspace = self.load_workspace(context).await?;
        Self::validate_thread(context, thread)?;
        let Some(actual_engine_thread_id) = thread.engine_thread_id.as_deref() else {
            return Ok(());
        };
        anyhow::ensure!(
            actual_engine_thread_id == engine_thread_id,
            "OpenCode 会话标识与当前会话不一致"
        );
        let cwd = self.thread_cwd(&workspace, thread).await?;
        if context.location_kind == CliLocationKind::Ssh {
            remote_project_opencode_runtime_service::runtime(&workspace)
                .await?
                .abort_session(&cwd, actual_engine_thread_id)
                .await
        } else {
            self.state.engines.interrupt(thread).await
        }
    }

    async fn archive_thread(
        &self,
        context: &CliExecutionContext,
        thread: &ThreadDto,
        engine_thread_id: &str,
    ) -> Result<()> {
        let workspace = self.load_workspace(context).await?;
        Self::validate_thread(context, thread)?;
        let cwd = self.thread_cwd(&workspace, thread).await?;
        if context.location_kind == CliLocationKind::Ssh {
            remote_project_opencode_runtime_service::runtime(&workspace)
                .await?
                .set_session_archived(&cwd, engine_thread_id, true)
                .await
        } else {
            self.state
                .engines
                .archive_opencode_remote_session(&cwd, engine_thread_id)
                .await
        }
    }

    async fn unarchive_thread(
        &self,
        context: &CliExecutionContext,
        thread: &ThreadDto,
        engine_thread_id: &str,
    ) -> Result<()> {
        let workspace = self.load_workspace(context).await?;
        Self::validate_thread(context, thread)?;
        let cwd = self.thread_cwd(&workspace, thread).await?;
        if context.location_kind == CliLocationKind::Ssh {
            remote_project_opencode_runtime_service::runtime(&workspace)
                .await?
                .set_session_archived(&cwd, engine_thread_id, false)
                .await
        } else {
            self.state
                .engines
                .unarchive_opencode_remote_session(&cwd, engine_thread_id)
                .await
        }
    }

    async fn forget_session(
        &self,
        context: &CliExecutionContext,
        thread: &ThreadDto,
        engine_thread_id: &str,
    ) -> Result<()> {
        let workspace = self.load_workspace(context).await?;
        Self::validate_thread(context, thread)?;
        let _active_turn_engine = self.remote_turn_use.lock().await.take();
        if context.location_kind == CliLocationKind::Ssh {
            remote_project_opencode_runtime_service::runtime(&workspace)
                .await?
                .forget_session(engine_thread_id)
                .await;
        } else {
            self.state
                .engines
                .forget_opencode_session(engine_thread_id)
                .await;
        }
        Ok(())
    }

    async fn read_thread_preview(
        &self,
        context: &CliExecutionContext,
        thread: &ThreadDto,
        _engine_thread_id: &str,
    ) -> Result<Option<String>> {
        self.load_workspace(context).await?;
        Self::validate_thread(context, thread)?;
        Ok(None)
    }

    async fn read_thread_sync_snapshot(
        &self,
        context: &CliExecutionContext,
        thread: &ThreadDto,
        _engine_thread_id: &str,
    ) -> Result<Option<ThreadSyncSnapshot>> {
        self.load_workspace(context).await?;
        Self::validate_thread(context, thread)?;
        Ok(None)
    }

    async fn set_thread_name(
        &self,
        context: &CliExecutionContext,
        thread: &ThreadDto,
        _engine_thread_id: &str,
        _name: &str,
    ) -> Result<()> {
        self.load_workspace(context).await?;
        Self::validate_thread(context, thread)?;
        Ok(())
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
        cwd: &str,
    ) -> Result<OpenCodeRuntimeCatalogDto> {
        let workspace = self.load_workspace(context).await?;
        let cwd = self.resolve_workspace_cwd(&workspace, Some(cwd)).await?;
        if context.location_kind == CliLocationKind::Ssh {
            remote_project_opencode_runtime_service::runtime(&workspace)
                .await?
                .runtime_catalog(&cwd)
                .await
        } else {
            self.configure_local_computer_control();
            self.state.engines.opencode_runtime_catalog(&cwd).await
        }
    }

    async fn refresh_extension_catalog(
        &self,
        context: &CliExecutionContext,
        cwd: Option<&str>,
        requested_kinds: &[String],
    ) -> Result<Vec<ExtensionCatalogKindRefreshDto>> {
        let workspace = self.load_workspace(context).await?;
        let cwd = self.resolve_workspace_cwd(&workspace, cwd).await?;
        if context.location_kind == CliLocationKind::Local {
            self.configure_local_computer_control();
            let mut results = Vec::new();
            for kind in requested_kinds {
                results.push(
                    extensions::opencode::refresh_kind(
                        self.state.engines.as_ref(),
                        Some(cwd.as_str()),
                        kind,
                    )
                    .await,
                );
            }
            return Ok(results);
        }

        let catalog = self
            .get_extension_catalog(context, Some(cwd.as_str()))
            .await?;
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
        let cwd = self.resolve_workspace_cwd(&workspace, cwd).await?;
        if context.location_kind == CliLocationKind::Local {
            return extensions::refresh::load_cached_catalog(
                &self.state,
                "opencode",
                Some(cwd.as_str()),
            )
            .await;
        }

        let runtime = self
            .get_opencode_runtime_catalog(context, cwd.as_str())
            .await?;
        let mut items = runtime
            .agents
            .into_iter()
            .filter(|agent| !agent.hidden)
            .map(|agent| ExtensionItemDto {
                id: agent.name.clone(),
                provider_id: "opencode".to_string(),
                kind: "agent".to_string(),
                name: agent.name,
                description: agent.description,
                version: None,
                scope: if agent.native { "native" } else { "project" }.to_string(),
                source: Some(agent.mode),
                marketplace: None,
                path: None,
                parent_plugin_id: None,
                category: agent.variant,
                officially_available: false,
                catalog_authority: None,
                installed: Some(true),
                configured: Some(true),
                enabled: Some(true),
                health: "healthy".to_string(),
                auth_state: None,
                available_actions: Vec::new(),
                requires_new_session: false,
                read_only_reason: Some("ssh_remote_opencode_extension_action".to_string()),
                warning: None,
                ..Default::default()
            })
            .collect::<Vec<_>>();
        // OpenCode 当前运行时目录只区分 Agent、Command 和 MCP；没有独立的 Skill 目录。
        // 用户在 OpenCode 里看到的 /xxx 项来自 OpenCode 自己的 Command 目录，不能强行标成 skill。
        // 因此这里保留 OpenCode 原生业务对象：Command -> kind=command，MCP -> kind=mcp。
        items.extend(
            runtime
                .commands
                .into_iter()
                .map(|command| ExtensionItemDto {
                    id: command.name.clone(),
                    provider_id: "opencode".to_string(),
                    kind: "command".to_string(),
                    name: command.name,
                    description: command.description,
                    version: None,
                    scope: "project".to_string(),
                    source: command.source,
                    marketplace: None,
                    path: None,
                    parent_plugin_id: None,
                    category: command.agent,
                    officially_available: false,
                    catalog_authority: None,
                    installed: Some(true),
                    configured: Some(true),
                    enabled: Some(true),
                    health: "healthy".to_string(),
                    auth_state: None,
                    available_actions: Vec::new(),
                    requires_new_session: false,
                    read_only_reason: Some("ssh_remote_opencode_extension_action".to_string()),
                    warning: None,
                    ..Default::default()
                }),
        );
        items.extend(
            runtime
                .mcp_servers
                .into_iter()
                .filter(|server| server.name != "panes-computer-control")
                .map(|server| {
                    let normalized_status = server.status.to_ascii_lowercase();
                    let health = if normalized_status.contains("connected") {
                        "healthy"
                    } else if normalized_status.contains("auth") {
                        "auth_required"
                    } else if normalized_status.contains("failed")
                        || normalized_status.contains("error")
                    {
                        "error"
                    } else {
                        "unknown"
                    };
                    let auth_state = match health {
                        "healthy" => "authenticated",
                        "auth_required" => "required",
                        _ => "unknown",
                    };
                    ExtensionItemDto {
                        id: server.name.clone(),
                        provider_id: "opencode".to_string(),
                        kind: "mcp".to_string(),
                        name: server.name,
                        description: server.detail,
                        version: None,
                        scope: "project".to_string(),
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
                        health: health.to_string(),
                        auth_state: Some(auth_state.to_string()),
                        available_actions: Vec::new(),
                        requires_new_session: false,
                        read_only_reason: Some("ssh_remote_opencode_extension_action".to_string()),
                        warning: None,
                        ..Default::default()
                    }
                }),
        );
        let fetched_at = chrono::Utc::now().to_rfc3339();
        let kind_fetched_at = ["agent", "command", "mcp"]
            .into_iter()
            .map(|kind| (kind.to_string(), Some(fetched_at.clone())))
            .collect();

        Ok(CachedExtensionCatalogDto {
            provider_id: "opencode".to_string(),
            cwd: Some(cwd),
            items,
            sources: Vec::new(),
            capabilities: extensions::provider_capabilities("opencode"),
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
                "agent" => {
                    item.panel = Some("agents".to_string());
                    item.group = Some("agents".to_string());
                }
                "command" => {
                    // OpenCode 的具体 Command 是可直接插入输入框的 /xxx 项，不是面板入口。
                    // 是否打开面板由 panel 字段决定；这里没有 panel，所以点击后插入 insert_text。
                    item.insert_text = Some(format!("/{} ", item.name));
                    item.group = Some("commands".to_string());
                }
                "mcp" => {
                    item.panel = Some("mcp".to_string());
                    item.group = Some("mcp".to_string());
                }
                _ => {}
            }
        }
        // 这里补的是 OpenCode 的一级面板入口。
        // 它们同样是 kind=command，但带 panel 字段；前端会按 panel 打开对应面板。
        let panel_ids = ["agents", "commands", "sessions"];
        items.extend(panel_ids.into_iter().map(|id| ExtensionItemDto {
            id: id.to_string(),
            provider_id: "opencode".to_string(),
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
        _scope: Option<&str>,
    ) -> Result<ExtensionActionResultDto> {
        let workspace = self.load_workspace(context).await?;
        anyhow::ensure!(
            context.location_kind == CliLocationKind::Local,
            "SSH 远端 OpenCode 当前不执行扩展变更，也不会调用本机 OpenCode"
        );
        let cwd = self.resolve_workspace_cwd(&workspace, None).await?;
        extensions::opencode::perform_action(&item, action, Some(cwd.as_str())).await
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
        Err(Self::unsupported("代码审查"))
    }
}
