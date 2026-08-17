# 启动时同步 SSH 远端项目会话设计

## 一、文档定位

本文只设计 SSH 远端项目在 Panes 启动时的会话同步闭环。它服务于第四阶段“远端项目对话能力”中的会话发现与左侧会话列表展示。

本文不设计远端项目的正式对话、流式消息、审批、附件上传和会话持续占用；这些业务仍属于第四阶段后续的对话管理范围。

## 二、已确认的业务目标

Panes 启动后，系统要把已保存的 SSH 远端服务器恢复为可用的 SSH tunnel Map，再在后台扫描每个 SSH 远端项目目录下的远端 CLI 会话，并同步到本地会话数据库。

左侧栏始终只读取本地数据库中的项目会话。远端扫描完成后，后端通知前端；前端据此重新读取该项目的本地会话数据并更新左侧栏。

例如，远端主机 A 的项目 `/work/project-a` 下有 OpenCode 三条会话、Claude Code 两条会话，后台同步完成后，该项目左侧栏显示这五条会话。即使其中的会话不是通过 Panes 创建的，也必须能在扫描后进入本地库并显示。

## 三、硬性边界

### 3.1 只处理 SSH 远端项目路线

本文仅适用于：

```text
SSH 远端项目 workspace
  -> sshConnectionId
  -> 远端主机
  -> 远端 CLI
  -> 远端项目目录下的会话
```

不得调用、修改或复用下列本机外部 CLI 服务路线：

- `list_codex_remote_threads`
- `attach_codex_remote_thread`
- `list_opencode_remote_sessions`
- `attach_opencode_remote_session`

这些旧函数中的 `remote` 是“本机 Panes 连接本机外部 CLI 服务”的含义，不是 SSH 远端主机。它们不属于本设计，也不能作为 SSH 远端项目同步的入口。

### 3.2 SSH tunnel 与远端 CLI 服务的边界

SSH tunnel 是长期存在的网络通路：

```text
本机 127.0.0.1:<local-port>
  -> SSH tunnel
  -> 远端 127.0.0.1:<remote-port>
```

远端 Codex、OpenCode、Claude Code 相关服务是按需启动和关闭的业务运行时。同步会话时可以临时拉起服务；同步结束且没有其他持续使用者时，应释放该临时使用并由既有生命周期规则关闭远端服务。

同步业务不建立、不删除、不维护 SSH tunnel，也不关心远端 IP、SSH 鉴权和端口转发细节。它只按 `hostId + cliId` 从 `SshCliTunnelRegistry` 获得本机连接入口。

### 3.3 本地路线保持不变

本地项目的会话刷新、会话发现和左侧栏行为保持原样。SSH 项目的同步入口必须独立分流，不能让现有本地刷新流程承担 SSH 语义。

## 四、涉及模块与职责

### 4.1 `SshCliTunnelRegistry`

文件：`src-tauri/src/ssh/cli_tunnel_registry.rs`

在现有 tunnel Map 和 CLI 生命周期能力中增加：

```rust
init_all_ssh_remote_server(...)
```

该方法仅在启动阶段负责：

1. 从本地 SSH 连接配置中读取已保存且启用的远端服务器。
2. 逐台校验连接、扫描该主机已安装且受 Panes 支持的 CLI。
3. 为扫描到的每个 `hostId + cliId` 恢复或建立 SSH tunnel。
4. 将 tunnel 通过既有 `add` 语义恢复到全局 Map。
5. 返回每台主机、每个 CLI 的恢复结果，供上层决定哪些远端项目可以继续同步。

该方法绝对不负责：

- 扫描项目会话；
- 启动远端 CLI 服务；
- 写入 `threads` 表；
- 通知前端；
- 决定左侧栏显示内容。

恢复时遵守既有 Map 语义：同一个 `hostId + cliId` 已存在 tunnel 时，不覆盖原 tunnel；只有 Map 中不存在时才新增。

### 4.2 SSH 远端项目会话刷新业务

新增独立文件：

```text
src-tauri/src/remote_project_session_refresh_service.rs
```

该文件是“SSH 远端项目会话刷新”业务的唯一入口。它按具体 SSH workspace 执行一次同步，负责：

1. 读取 workspace 的 `sshConnectionId` 与标准化后的远端项目根目录。
2. 根据 `sshConnectionId` 从 `SshCliTunnelRegistry` 获取该主机当前可用的 CLI tunnel 列表。
3. 对每个 CLI 依次申请一次临时服务使用权。
4. 调用注册表已有的按需启动能力，取得该 CLI 对应的本机入口。
5. 用 CLI 自己的协议，并携带当前远端项目根目录，扫描该项目下的远端会话。
6. 把扫描结果按该 workspace 写入本地会话数据库。
7. 在本次 CLI 扫描完成后释放临时服务使用权。
8. 所有 CLI 处理完毕且本地事务成功提交后，通知前端刷新该 workspace 的会话列表。

该业务只使用 `hostId + cliId -> tunnel`。项目目录只随每个扫描请求传入，不能成为 tunnel Map 的 key，也不能导致为每个项目重复建立 tunnel。

### 4.3 `message_notify_helper`

现有前端已经有全局 `toastStore` 和 `ToastContainer`，可直接用于最终的界面提示；但后端目前没有统一的“业务完成后通知前端重新读库”的封装。

因此新增：

```text
src-tauri/src/message_notify_helper.rs
```

该文件统一封装后端向前端发送业务通知的事件。首个通知场景是 SSH 远端项目会话同步；后续其他业务需要通知前端时也通过这个文件发送，避免业务模块各自直接拼接事件名和 payload。

首个事件定义如下：

```text
事件名：ssh-remote-project-sessions-refreshed
载荷：workspaceId、同步成功的 cliId 列表、同步摘要
```

通知只在会话数据已经成功提交到本地数据库之后发送。通知载荷不携带完整会话列表；前端收到事件后，以 `workspaceId` 从本地数据库重新读取该项目的会话，从而保证左侧栏和数据库一致。

失败场景使用独立事件或带失败状态的同步结果通知，至少包含 `workspaceId`、失败的 `cliId` 和可展示的错误摘要。失败时保留本地已有会话，不清空左侧栏，也不回退到本机同名 CLI。

### 4.4 前端会话状态

前端为 SSH workspace 增加独立的“仅从本地数据库重新加载会话”入口。收到 `ssh-remote-project-sessions-refreshed` 后，只重新读取指定 workspace 的本地会话记录并更新状态。

不得让该通知调用现有会先执行本机外部 CLI 发现逻辑的刷新方法。否则 SSH 项目会再次误入旧的本机 `remote` 路线。

## 五、启动时完整流程

```text
Panes 启动
  │
  ├─ 1. 前端注册 SSH 会话同步完成事件监听
  │
  ├─ 2. 前端按现有逻辑读取本地 workspace 和本地 threads
  │       └─ 左侧栏先显示本地已有缓存，不等待远端网络
  │
  ├─ 3. 后端调用 SshCliTunnelRegistry.init_all_ssh_remote_server(...)
  │       └─ 恢复 hostId -> cliId -> tunnel Map
  │
  ├─ 4. 后端枚举所有 SSH workspace
  │
  ├─ 5. 对每个 SSH workspace 在后台执行一次
  │       RemoteProjectSessionRefreshService.refresh_workspace(...)
  │
  ├─ 6. 每个 CLI 按需启动、扫描、写本地库、释放临时服务使用
  │
  └─ 7. 数据库提交后发送 workspace 级通知
          └─ 前端仅重新读该 workspace 的本地会话并更新左侧栏
```

启动同步必须异步执行，不能阻塞主界面打开。远端服务器不可达、某个 CLI 缺失或某个扫描失败时，只影响对应 workspace 或 CLI 的本轮同步；其他已恢复的服务器和本地项目仍然正常可用。

## 六、单个 workspace 的同步步骤

以远端主机 `192.168.1.12` 上的 Codex、项目 `/work/a` 为例：

1. 取得 workspace 绑定的 `sshConnectionId`，并取得标准化的远端根目录 `/work/a`。
2. 根据 `sshConnectionId + codex` 从 `SshCliTunnelRegistry` 取得已恢复的 tunnel。
3. 申请 Codex 的一次临时服务使用权，注册表按需拉起远端 Codex app-server，并给出该 tunnel 对应的本机端口。
4. 会话刷新业务使用本机 Codex 客户端连接 `127.0.0.1:<local-port>`，在请求中携带 `cwd=/work/a`，读取远端 Codex 会话。
5. 将返回会话映射为当前 workspace 的本地 `threads` 数据；已有同一远端会话则更新，不存在则新增。
6. 数据库提交成功后，释放 Codex 的临时服务使用权。
7. 如果此时没有任何临时使用和持续会话占用，注册表按已有规则关闭远端 Codex 服务；SSH tunnel 不因此关闭。
8. 发送该 workspace 的同步完成通知；前端重新从本地数据库读取 `/work/a` 的会话。

OpenCode 和 Claude Code 复用完全相同的业务步骤。它们只在“如何启动服务、如何携带项目目录、如何请求会话列表、如何映射会话字段”这四处使用各自协议；不得改变 tunnel Map、启动顺序或数据库归属规则。

## 七、会话本地化与幂等规则

### 7.1 会话归属

远端发现到的会话必须写入对应 SSH workspace，不能只按 CLI 会话 ID 做全局归属。同步身份至少以以下三项确定：

```text
workspaceId + engineId + engineThreadId
```

同一远端主机不同项目、同一 CLI 下可能存在同名或相同格式的会话 ID。它们的本地归属由 workspace 决定，不能因为旧的全局查找逻辑而串到其他项目或本地项目。

### 7.2 同步结果

单次扫描的每条远端会话：

- 本地不存在：创建本地 thread。
- 本地已存在：更新标题、更新时间、摘要和远端状态等可同步字段。
- 远端本轮未返回：不立即删除本地 thread。

最后一项是为了避免网络中断、远端分页异常或 CLI 短暂故障时误删用户已有会话。远端会话删除或失效的正式规则，在后续明确会话删除业务后单独设计。

### 7.3 数据提交与通知顺序

每个 workspace 的本地会话数据必须先完成数据库事务提交，再发前端同步通知。事务失败时不发成功通知；前端继续保留上一次本地数据。

## 八、并发与生命周期规则

1. tunnel 的恢复和维护单位是 `hostId + cliId`，不是 workspace。
2. 同一台主机多个 SSH workspace 同时刷新同一个 CLI 时，共用 tunnel 和远端 CLI 服务；每次刷新各自申请、释放一次临时服务使用权。
3. 同一项目的不同 CLI 可独立同步；一个 CLI 失败不阻塞其他 CLI。
4. 同一个 workspace 的重复同步要串行化或合并为一次，不能并发写入同一批本地会话。
5. 正在进行正式对话的持续会话持有自己的持续使用权。后台同步释放临时使用权时，不能关闭仍被持续会话占用的远端服务。
6. 关闭远端 CLI 服务是注册表内部基于“临时使用数为零且持续会话占用为空”的既有规则自主触发的结果；会话刷新业务不直接杀进程。

## 九、CLI 协议适配要求

### 9.1 Codex

通过 SSH tunnel 暴露的远端 Codex app-server 请求会话列表，并在请求中携带当前远端项目目录 `cwd`。只能同步该目录归属的会话。

### 9.2 OpenCode

通过 SSH tunnel 暴露的远端 OpenCode 服务请求会话列表，并使用 OpenCode 的项目目录参数或 `X-OpenCode-Directory` 请求头限定当前项目目录。只能同步该目录归属的会话。

### 9.3 Claude Code

Claude Code 通过 Panes 安装到远端运行时目录的专用会话适配服务提供 `GET /sessions?cwd=<remote-project-root>`。该服务只绑定远端 `127.0.0.1`，由 SSH tunnel 暴露为本机入口；它从远端 Claude 的 `~/.claude/projects` 项目会话记录中读取当前 `cwd` 的会话摘要，并返回会话 ID、标题和最后活动时间。

Claude 的远端适配器安装检查、按需启动和关闭仍由注册表的 CLI 服务生命周期负责，并且只在 Claude Code 被实际使用时执行。该适配服务不替代也不改动本机 Claude 的既有 stdio 适配器。

## 十、失败与通知规则

| 场景 | 后端行为 | 前端结果 |
| --- | --- | --- |
| SSH 主机恢复失败 | 记录该主机失败原因，不扫描其 workspace | 保留本地缓存；显示一次汇总提示或该项目的刷新失败状态 |
| 某 CLI 未安装 | 跳过该 CLI，不回退到本机 CLI | 其他 CLI 会话仍可显示 |
| 某 CLI 服务启动失败 | 释放本次临时使用，记录错误 | 保留该项目既有本地会话 |
| 某 CLI 扫描失败 | 不删除本地会话，继续下一 CLI | 可提示该 CLI 同步失败 |
| 本地数据库提交失败 | 不发成功通知 | 左侧栏保持上一次数据 |
| 前端通知发送失败 | 记录发送错误；下次刷新可再次通知 | 不影响已提交的本地数据 |

启动批量同步不得对每个项目、每个 CLI 都弹出成功提示。正常成功只更新左侧栏；失败信息按项目汇总或在用户主动刷新时明确展示，避免启动时产生大量提示。

## 十一、实现文件范围

本设计落实时，允许修改或新增的 SSH 远端项目文件范围如下：

- 修改：`src-tauri/src/ssh/cli_tunnel_registry.rs`，增加 `init_all_ssh_remote_server`。
- 新增：`src-tauri/src/remote_project_session_refresh_service.rs`，封装 SSH 远端项目会话刷新业务。
- 新增：`src-tauri/src/message_notify_helper.rs`，统一封装后端通知前端。
- 修改：启动编排入口，仅负责按顺序调用 Map 恢复与后台会话同步。
- 修改：SSH workspace 的前端会话状态入口，增加仅从本地数据库重新读取指定 workspace 会话的能力和事件监听。
- 修改：SSH 远端项目会话所需的数据库查询或幂等写入逻辑。

以下内容明确不在本设计的修改范围内：

- 本机外部 CLI 服务会话发现函数；
- 本地 workspace 的会话刷新逻辑；
- `list_codex_remote_threads`、`list_opencode_remote_sessions` 及其调用链；
- 第三阶段的远端目录树、文件、Git、工作树和终端能力；
- 正式对话、流式消息、审批和附件上传。

如果会话幂等写入需要新增 SQLite 索引或约束，必须按 `docs/database-version-migration.md` 新建更高版本迁移；不得修改已发布迁移。

## 十二、验收标准

1. Panes 启动后，已保存且启用的 SSH 远端服务器能恢复为 `hostId -> cliId -> tunnel` Map。
2. 启动同步不阻塞主界面；左侧栏先显示本地缓存，再按后台同步结果更新。
3. SSH 远端项目只通过自己的 `sshConnectionId` 和 tunnel Map 扫描远端会话。
4. Codex、OpenCode 会话扫描均携带当前远端项目目录，不会把同一主机其他项目的会话显示到本项目。
5. 同步结果先写入并提交本地数据库，再通知前端重新读取该项目会话。
6. SSH 项目收到通知后，不调用任何本机外部 CLI 服务会话发现函数。
7. 远端扫描失败时，不删除现有本地会话，不回退到本机同名 CLI，不影响本地项目路线。
8. 同一主机、同一 CLI 被多个项目同时同步时，共用一条 tunnel 和一个按需运行的远端服务，生命周期计数正确。
9. 正式对话持续占用服务时，后台同步结束不会把该服务关闭。
10. 本地项目及其原有本机外部 CLI 服务路线的行为不发生变化。

## 十三、与后续对话能力的关系

本设计交付的是“远端项目会话发现、同步入库、通知左侧栏更新”的启动期与刷新期能力。

后续正式对话业务只能复用以下稳定结果：

- `SshCliTunnelRegistry` 提供的 `hostId + cliId` 本机连接入口；
- 远端 CLI 的按需启动与生命周期管理；
- 已归属到 SSH workspace 的本地 thread 记录；
- 统一的前端业务通知机制。

后续正式对话不得改变本文已经固定的 tunnel Map 结构、项目目录随请求传递的规则，以及“左侧栏只读本地数据库”的原则。
