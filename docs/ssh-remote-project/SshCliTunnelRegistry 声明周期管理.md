# SshCliTunnelRegistry 生命周期管理

## 一、文档范围

本文只规定 [`SshCliTunnelRegistry`](../../src-tauri/src/ssh/cli_tunnel_registry.rs) 对“远端 CLI 服务”的生命周期管理。

本文不讨论 SSH tunnel 的建立、保活、重连和销毁；SSH tunnel 是持续存在的网络基础设施。本文也不讨论左侧项目会话列表、会话归属判断和本地数据库更新。

不新增生命周期管理类。远端 CLI 服务的生命周期状态和操作，全部收敛在 `SshCliTunnelRegistry` 中。

## 二、管理单位

全局 tunnel Map 的结构固定为：

```text
hostId
  ↓
cliId
  ↓
tunnel
```

远端 CLI 服务的生命周期管理单位同样固定为：

```text
hostId + cliId
```

不把 `workspaceId`、远端项目目录或 `threadId` 加入 tunnel Map 的 key。

同一台远端主机上，多个项目和多个会话可以共用同一种 CLI 的服务入口。每次业务请求自行携带当前远端项目根目录；项目上下文不由 tunnel 决定。

## 三、Registry 内部状态

每个 `hostId + cliId` 对应的 tunnel，除既有网络信息外，还需要维护以下远端 CLI 服务状态：

```text
serviceState: stopped | starting | running | stopping
temporaryUseCount: 整数
persistentSessionUses: threadId -> 引用计数
```

含义如下：

- `serviceState`：远端 CLI 服务当前是否已运行，或正处于启动、关闭过程。
- `temporaryUseCount`：正在执行一次性业务的数量。
- `persistentSessionUses`：仍在持续对话的远端会话及其占用次数。

`threadId` 只是持续占用的凭证。Registry 不根据它查询项目归属，不刷新会话，不更新数据库，也不控制左侧列表显示。

## 四、临时占用

临时占用适用于“用完即释放”的业务，例如刷新某个远端项目下的会话。

流程固定为：

1. 业务层调用临时占用开始，并传入 `hostId + cliId`。
2. Registry 从 Map 取得既有 tunnel，将 `temporaryUseCount` 加一。
3. 如果远端 CLI 服务尚未运行，Registry 拉起服务；如果已有调用正在启动，同类调用等待这一次启动完成。
4. 业务层使用 tunnel 映射出的本地端口，按对应 CLI 协议并携带当前远端项目目录，获取会话数据。
5. 业务结束时，无论成功、失败或取消，业务层都必须调用临时占用结束。
6. Registry 将 `temporaryUseCount` 减一，并判断是否还允许关闭远端 CLI 服务。

临时业务不能直接调用远端 CLI 服务的启动或关闭函数。

## 五、持续占用

持续占用适用于远端项目内正在对话的会话。它可以并行存在于：

- 不同远端项目；
- 同一个远端项目的不同会话；
- 同一个远端主机、同一种 CLI 下的多个会话。

流程固定为：

1. 某个会话进入持续对话时，业务层调用持续占用开始，并传入 `hostId + cliId + threadId`。
2. Registry 在 `persistentSessionUses` 中登记或增加该 `threadId` 的引用计数。
3. Registry 确保远端 CLI 服务已经运行；若服务正在启动，调用方等待本次启动完成。
4. 该会话之后的每次消息，都通过该 tunnel 和服务入口发送，并在请求中携带自己的远端项目根目录。
5. 该持续对话明确结束时，业务层调用持续占用结束，并传入同一个 `hostId + cliId + threadId`。
6. Registry 减少或移除该 `threadId` 的引用计数，并判断是否还允许关闭远端 CLI 服务。

左侧仍存在会话历史、数据库仍保存 `thread`，不代表持续占用仍然存在。持续占用只由“正在进行中的对话业务”建立和释放。

## 六、关闭判定

远端 CLI 服务只有同时满足下面两个条件时，才允许关闭：

```text
temporaryUseCount == 0
且
persistentSessionUses 为空
```

只要任意临时业务仍在执行，或任意持续会话仍在对话，服务就不能关闭。

关闭远端 CLI 服务不等于删除或关闭 SSH tunnel。服务关闭后，`hostId -> cliId -> tunnel` 记录和 SSH tunnel 仍按网络层规则保留、保活或最终销毁。

## 七、并发规则

对于同一组 `hostId + cliId`：

- 多个临时刷新可以并发占用同一个服务。
- 多个持续会话可以并发占用同一个服务。
- 临时占用和持续占用可以同时存在。
- 服务从 `stopped` 到 `starting` 时，只允许一个实际启动动作；其他调用等待其结果。
- 服务从 `running` 到 `stopping` 时，新的占用请求必须阻止关闭，并确保服务重新处于可用状态后才能继续业务。
- 任何业务不得因为自身结束而直接关闭服务；关闭必须由 Registry 根据全部占用状态统一判定。

## 八、职责边界

业务层只做两类动作：

```text
申请占用
释放占用
```

`SshCliTunnelRegistry` 负责：

- 根据 `hostId + cliId` 取得既有 tunnel；
- 维护临时占用和持续占用；
- 协调远端 CLI 服务的启动、等待和关闭；
- 向业务层提供可用的本机连接入口。

业务层不直接关心远端服务是否已启动，也不直接调用服务停止。Registry 不关心项目归属、会话列表、会话刷新结果或数据库持久化。

## 九、架构结果

同一台远端主机上的项目 A、项目 B，同时使用同一个 CLI 时：

```text
项目 A 的临时刷新 ─┐
项目 B 的持续对话 ─┼─> hostId + cliId 的同一服务入口
项目 A 的另一会话 ─┘
```

三者共享 tunnel 和远端 CLI 服务，但各自请求携带自己的远端项目根目录和会话标识。服务是否关闭只取决于全部占用是否已释放。
