/*
use crate::{
    commands::harness::{detect_harness, detect_via_login_shell, HARNESSES},
    models::HarnessReport,
    runtime_env,
};

pub(crate) struct LocalCliServiceLifecycle;

impl LocalCliServiceLifecycle {
    pub(crate) async fn list_ready() -> Result<HarnessReport, String> {
        let mut harnesses = Vec::new();

        for def in HARNESSES {
            let status = detect_harness(def).await;
            harnesses.push(status);
        }

        let package_manager_available = runtime_env::resolve_executable("npm").is_some()
            || detect_via_login_shell("npm", "--version").await.is_some();

        let mise_preferred =
            runtime_env::is_flatpak() && runtime_env::resolve_executable("mise").is_some();
        let preferred_install_method = if mise_preferred {
            Some("mise".to_string())
        } else if package_manager_available {
            Some("npm".to_string())
        } else {
            None
        };

        Ok(HarnessReport {
            harnesses,
            npm_available: package_manager_available,
            preferred_inst
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, LazyLock,
    },
};

use anyhow::Context;
use tokio::sync::{Mutex, RwLock};

use crate::engines::{
    claude_suse crate::engines::{
    claude_sidecar::ClaudeSidecarEngine, codex::CodexEngine, opencode::OpenCodeEngine,
};
use crate::{commands::harness::detect_via_login_shell, runtime_env};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LocalCliServiceEntryState {
    Ready,
    Terminating,
}

pub(crate) enum LocalCliHandle {
    Codex(Arc<CodexEngine>),
    OpenCode(Arc<OpenCodeEngine>),
    Claude(Arc<ClaudeSidecarEngine>),
}

/// 本机一个 CLI 服务的生命周期入口。
///
/// 服务由 CLI ID 唯一标识。业务代码只通过 `get` 取得已经由 Panes 启动阶段
/// 创建并登记的本地 CLI 句柄。
pub(crate) struct LocalCliService {
    cli_id: String,
    generation: u64,
    handle: LocalCliHandle,
    state: Mutex<LocalCliServiceEntryState>,
}

impl LocalCliService {
    pub(crate) fn cli_id(&self) -> &str {
        &self.cli_id
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    pub(crate) fn handle(&self) -> &LocalCliHandle {
        &self.handle
    }
}

#[derive(Default)]
struct LocalCliServiceLifecycleRegistry {
    services: RwLock<HashMap<String, Arc<LocalCliService>>>,
    mutation_lock: Mutex<()>,
}

static LOCAL_CLI_SERVICES: LazyLock<LocalCliServiceLifecycleRegistry> =
    LazyLock::new(LocalCliServiceLifecycleRegistry::default);
static NEXT_SERVICE_GENERATION: AtomicU64 = AtomicU64::new(1);

pub(crate) struct LocalCliServiceLifecycle;

impl LocalCliServiceLifecycle {
    /// 探测本机三种聊天 CLI，并将已安装的 CLI 服务逐个登记到生命周期 MAP。
    pub(crate) async fn init() -> anyhow::Result<()> {
        for (cli_id, command) in [
            ("codex", "codex"),
            ("opencode", "opencode"),
            ("claude", "claude"),
        ] {
            let found = runtime_env::resolve_executable(command).is_some()
                || detect_via_login_shell(command, "--version").await.is_some();
            if !found {
                log::warn!("本机未探测到 CLI，跳过生命周期登记: cli_id={cli_id}");
                continue;
            }

            Self::set(cli_id)
                .await
                .with_context(|| format!("初始化本地 CLI 服务失败: cli_id={cli_id}"))?;
        }

        Ok(())
    }

服务；该方法不会启动服务。
    pub(crate) async fn get(cli_id: &str) -> anyhow::Result<Arc<LocalCliService>> {
        LOCAL_CLI_SERVICES.get(cli_id).await
    }

    /// 列出已经完成登记并处于 Ready 状态的本地 CLI 服务。
    pub(crate) async fn list_ready() -> Vec<Arc<LocalCliService>> {
        LOCAL_CLI_SERVICES.list_ready().await
    }

    /// 启动并登记一个本地 CLI 服务。相同 CLI ID 重复调用时复用已有服务。
    pub(crate) async fn set(cli_id: &str) -> anyhow::Result<Arc<LocalCliService>> {
        LOCAL_CLI_SERVICES.set(cli_id).await
    }

    /// 终止一个本地 CLI 服务并移除登记。
    pub(crate) async fn terminate(cli_id: &str) -> anyhow::Result<bool> {
        LOCAL_CLI_SERVICES.terminate(cli_id).await
    }

    /// 终止当前 Panes 进程已经登记的全部本地 CLI 服务。
    pub(crate) async fn terminate_all() -> anyhow::Result<()> {
        LOCAL_CLI_SERVICES.terminate_all().await
    }
}

impl LocalCliServiceLifecycleRegistry {
    async fn get(&self, cli_id: &str) -> anyhow::Result<Arc<LocalCliService>> {
        let service = self
            .services
            .read()
            .await
            .get(cli_id)
            .cloned()
            .with_context(|| format!("本地 CLI 服务未在 Panes 启动阶段登记: cli_id={cli_id}"))?;

        let state = service.state.lock().await;
        anyhow::ensure!(
            *state == LocalCliServiceEntryState::Ready,
            "本地 CLI 服务正在终止: cli_id={cli_id}"
        );
        drop(state);
        Ok(service)
    }

    async fn list_ready(&self) -> Vec<Arc<LocalCliService>> {
        let mut services = self
            .services
            .read()
            .await
            .values()
            .cloned()
            .collect::<Vec<_>>();
        services.sort_by(|left, right| left.cli_id().cmp(right.cli_id()));

        let mut ready = Vec::with_capacity(services.len());
        for service in services {
            if *service.state.lock().await == LocalCliServiceEntryState::Ready {
                ready.push(service);
            }
        }
        ready
    }

    async fn set(&self, cli_id: &str) -> anyhow::Result<Arc<LocalCliService>> {
        let _mutation_guard = self.mutation_lock.lock().await;
        let existing = self.services.read().await.get(cli_id).cloned();
        if let Some(service) = existing {
            let state = service.state.lock().await;
            anyhow::ensure!(
                *state == LocalCliServiceEntryState::Ready,
                "本地 CLI 服务正在终止，不能重复登记: cli_id={cli_id}"
            );
            drop(state);
            return Ok(service);
        }

        let handle = match cli_id {
            "codex" => {
                let engine = Arc::new(CodexEngine::default());
                engine.prewarm().await?;
                LocalCliHandle::Codex(engine)
            }
            "opencode" => {
                let engine = Arc::new(OpenCodeEngine::default());
                engine.prewarm().await?;
                LocalCliHandle::OpenCode(engine)
            }
            "claude" => {
                let engine = Arc::new(ClaudeSidecarEngine::default());
                engine.prewarm().await?;
                LocalCliHandle::Claude(engine)
            }
            _ => anyhow::bail!("不支持的本地 CLI 工具: {cli_id}"),
        };

        let service = Arc::new(LocalCliService {
            cli_id: cli_id.to_string(),
            generation: NEXT_SERVICE_GENERATION.fetch_add(1, Ordering::Relaxed),
            handle,
            state: Mutex::new(LocalCliServiceEntryState::Ready),
        });

        let mut services = self.services.write().await;
        if let Some(existing) = services.get(cli_id) {
            return Ok(existing.clone());
        }
        services.insert(cli_id.to_string(), service.clone());
        Ok(service)
    }

    async fn terminate(&self, cli_id: &str) -> anyhow::Result<bool> {
        let _mutation_guard = self.mutation_lock.lock().await;
        let service = match self.get(cli_id).await {
            Ok(service) => service,
            Err(error) if error.to_string().contains("未在 Panes 启动阶段登记") => {
                return Ok(false);
            }
            Err(error) => return Err(error),
        };

        *service.state.lock().await = LocalCliServiceEntryState::Terminating;

        let mut services = self.services.write().await;
        let remove_service = services
            .get(cli_id)
            .map(|registered| Arc::ptr_eq(registered, &service))
            .unwrap_or(false);
        if remove_service {
            services.remove(cli_id);
        }
        Ok(remove_service)
    }

    async fn terminate_all(&self) -> anyhow::Result<()> {
        let cli_ids = self
            .services
            .read()
            .await
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        let mut errors = Vec::new();

        for cli_id in cli_ids {
            if let Err(error) = self.terminate(&cli_id).await {
                errors.push(format!("cli_id={cli_id} error={error:#}"));
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            anyhow::bail!("停止本地 CLI 服务失败: {}", errors.join("; "));
        }
    }
}
all_method,
        })
    }
}
*/
