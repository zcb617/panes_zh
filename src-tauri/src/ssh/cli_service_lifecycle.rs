use std::{
    any::Any,
    collections::HashMap,
    sync::{Arc, LazyLock},
};

use anyhow::Context;
use tokio::sync::{Mutex, RwLock};

use crate::ssh::cli_tunnel_registry::{self, RemoteCliRuntimeCache, SshCliTunnel};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SshCliServiceEntryState {
    Ready,
    Terminating,
}

/// 一台远端机器上一个 CLI 服务的生命周期入口。
///
/// 服务由“SSH 连接配置 ID + CLI ID”唯一标识。外部业务只通过 `get` 取得已就绪
/// 的服务，再读取其运行时；启动、停止和运行时缓存均由本模块管理。
pub(crate) struct SshCliService {
    connection_id: String,
    cli_id: String,
    tunnel: Arc<SshCliTunnel>,
    state: Mutex<SshCliServiceEntryState>,
}

impl SshCliService {
    /// 仅供启动阶段创建 CLI 专属运行时使用。
    pub(crate) fn tunnel(&self) -> &Arc<SshCliTunnel> {
        &self.tunnel
    }

    /// 将启动阶段已经创建的 CLI 专属运行时登记到当前服务。
    pub(crate) async fn set_runtime<T>(&self, runtime: Arc<T>) -> anyhow::Result<()>
    where
        T: Any + Send + Sync + 'static,
    {
        let state = self.state.lock().await;
        anyhow::ensure!(
            *state == SshCliServiceEntryState::Ready,
            "SSH 远端 CLI 服务正在终止，不能登记运行时: connection_id={} cli_id={}",
            self.connection_id,
            self.cli_id
        );
        drop(state);

        let service_generation = self
            .tunnel
            .service_lifecycle
            .lock()
            .await
            .service_generation;
        anyhow::ensure!(
            service_generation > 0,
            "SSH 远端 CLI 服务尚未启动，不能登记运行时: connection_id={} cli_id={}",
            self.connection_id,
            self.cli_id
        );

        *self.tunnel.service_runtime.lock().await = Some(RemoteCliRuntimeCache {
            service_generation,
            runtime,
        });
        Ok(())
    }

    /// 供 CLI 接口实现类取得启动阶段登记的专属运行时。
    pub(crate) async fn get_runtime<T>(&self) -> anyhow::Result<Arc<T>>
    where
        T: Any + Send + Sync + 'static,
    {
        let state = self.state.lock().await;
        anyhow::ensure!(
            *state == SshCliServiceEntryState::Ready,
            "SSH 远端 CLI 服务正在终止，不能读取运行时: connection_id={} cli_id={}",
            self.connection_id,
            self.cli_id
        );
        drop(state);

        let service_generation = self
            .tunnel
            .service_lifecycle
            .lock()
            .await
            .service_generation;
        let runtime = {
            let cached_runtime = self.tunnel.service_runtime.lock().await;
            let entry = cached_runtime.as_ref().with_context(|| {
                format!(
                    "SSH 远端 CLI 服务尚未登记运行时: connection_id={} cli_id={}",
                    self.connection_id, self.cli_id
                )
            })?;
            anyhow::ensure!(
                entry.service_generation == service_generation,
                "SSH 远端 CLI 服务运行时已失效: connection_id={} cli_id={}",
                self.connection_id,
                self.cli_id
            );
            entry.runtime.clone()
        };

        runtime.downcast::<T>().map_err(|_| {
            anyhow::anyhow!(
                "SSH 远端 CLI 服务运行时类型不匹配: connection_id={} cli_id={} expected={}",
                self.connection_id,
                self.cli_id,
                std::any::type_name::<T>()
            )
        })
    }
}

#[derive(Default)]
pub(crate) struct SshCliServiceLifecycleRegistry {
    services: RwLock<HashMap<String, HashMap<String, Arc<SshCliService>>>>,
}

static SSH_CLI_SERVICES: LazyLock<SshCliServiceLifecycleRegistry> =
    LazyLock::new(SshCliServiceLifecycleRegistry::default);

/// 取得已由启动阶段登记的远端 CLI 服务；该方法不会启动或重连服务。
pub async fn get(connection_id: &str, cli_id: &str) -> anyhow::Result<Arc<SshCliService>> {
    SSH_CLI_SERVICES.get(connection_id, cli_id).await
}

/// 启动并登记一个远端 CLI 服务。相同“连接配置 ID + CLI ID”重复调用时复用已有服务。
pub async fn set(connection_id: &str, cli_id: &str) -> anyhow::Result<Arc<SshCliService>> {
    SSH_CLI_SERVICES.set(connection_id, cli_id).await
}

/// 终止一个远端 CLI 服务并移除其运行时登记。
pub async fn terminate(connection_id: &str, cli_id: &str) -> anyhow::Result<bool> {
    SSH_CLI_SERVICES.terminate(connection_id, cli_id).await
}

/// 终止当前应用已登记的全部远端 CLI 服务。
pub async fn terminate_all() -> anyhow::Result<()> {
    SSH_CLI_SERVICES.terminate_all().await
}

impl SshCliServiceLifecycleRegistry {
    async fn get(&self, connection_id: &str, cli_id: &str) -> anyhow::Result<Arc<SshCliService>> {
        let service = self
            .services
            .read()
            .await
            .get(connection_id)
            .and_then(|host_services| host_services.get(cli_id))
            .cloned()
            .with_context(|| {
                format!(
                    "SSH 远端 CLI 服务未在启动阶段登记: connection_id={connection_id} cli_id={cli_id}"
                )
            })?;

        let state = service.state.lock().await;
        anyhow::ensure!(
            *state == SshCliServiceEntryState::Ready,
            "SSH 远端 CLI 服务正在终止: connection_id={connection_id} cli_id={cli_id}"
        );
        drop(state);
        Ok(service)
    }

    async fn set(&self, connection_id: &str, cli_id: &str) -> anyhow::Result<Arc<SshCliService>> {
        let existing = self
            .services
            .read()
            .await
            .get(connection_id)
            .and_then(|host_services| host_services.get(cli_id))
            .cloned();
        if let Some(service) = existing {
            let state = service.state.lock().await;
            anyhow::ensure!(
                *state == SshCliServiceEntryState::Ready,
                "SSH 远端 CLI 服务正在终止，不能重复登记: connection_id={connection_id} cli_id={cli_id}"
            );
            drop(state);
            return Ok(service);
        }

        let tunnel = cli_tunnel_registry::start_remote_cli_service(connection_id, cli_id).await?;
        let service = Arc::new(SshCliService {
            connection_id: connection_id.to_string(),
            cli_id: cli_id.to_string(),
            tunnel,
            state: Mutex::new(SshCliServiceEntryState::Ready),
        });

        let registered = {
            let mut services = self.services.write().await;
            let host_services = services.entry(connection_id.to_string()).or_default();
            if let Some(existing) = host_services.get(cli_id) {
                existing.clone()
            } else {
                host_services.insert(cli_id.to_string(), service.clone());
                service
            }
        };
        let state = registered.state.lock().await;
        anyhow::ensure!(
            *state == SshCliServiceEntryState::Ready,
            "SSH 远端 CLI 服务正在终止，不能重复登记: connection_id={connection_id} cli_id={cli_id}"
        );
        drop(state);
        Ok(registered)
    }

    async fn terminate(&self, connection_id: &str, cli_id: &str) -> anyhow::Result<bool> {
        let service = self.get(connection_id, cli_id).await?;
        {
            let mut state = service.state.lock().await;
            *state = SshCliServiceEntryState::Terminating;
        }

        let result = cli_tunnel_registry::stop_remote_cli_service(connection_id, cli_id).await;
        match result {
            Ok(stopped) => {
                let mut services = self.services.write().await;
                let remove_connection = if let Some(host_services) = services.get_mut(connection_id)
                {
                    let remove_service = host_services
                        .get(cli_id)
                        .map(|registered| Arc::ptr_eq(registered, &service))
                        .unwrap_or(false);
                    if remove_service {
                        host_services.remove(cli_id);
                    }
                    host_services.is_empty()
                } else {
                    false
                };
                if remove_connection {
                    services.remove(connection_id);
                }
                Ok(stopped)
            }
            Err(error) => {
                *service.state.lock().await = SshCliServiceEntryState::Ready;
                Err(error)
            }
        }
    }

    async fn terminate_all(&self) -> anyhow::Result<()> {
        let keys = self
            .services
            .read()
            .await
            .iter()
            .flat_map(|(connection_id, host_services)| {
                host_services
                    .keys()
                    .map(move |cli_id| (connection_id.clone(), cli_id.clone()))
            })
            .collect::<Vec<_>>();

        let mut errors = Vec::new();
        for (connection_id, cli_id) in keys {
            if let Err(error) = self.terminate(&connection_id, &cli_id).await {
                errors.push(format!(
                    "connection_id={connection_id} cli_id={cli_id} error={error:#}"
                ));
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            anyhow::bail!("停止 SSH 远端 CLI 服务失败: {}", errors.join("; "));
        }
    }
}
