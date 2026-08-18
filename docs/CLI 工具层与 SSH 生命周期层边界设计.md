# CLI 工具、远端服务与 SSH Tunnel 三层架构设计

## 一、文档定位

本文是 Panes 远端 CLI 调用链的唯一边界规范。后续开发、排错、重构和代码审查，都必须以本文规定的三层架构为准。

本文替代旧的“两层模型”。旧模型中“CLI 实现直接取得 Tunnel 并启动远端服务”的设计已经废止。

关联文档：

- [多 CLI 工具统一接口架构设计](多%20CLI%20工具统一接口架构设计.md)
- [SshCliTunnelRegistry 生命周期管理](ssh-remote-project/SshCliTunnelRegistry%20声明周期管理.md)
- [启动时同步 SSH 远端项目会话设计](ssh-remote-project/启动时同步SSH远端项目会话设计.md)

## 二、固定三层架构

远端 CLI 调用链固定分为以下三层，调用方向只能从上到下：

```text
业务调用方
    ↓
CliTool 统一接口、工厂、各 CLI 实现
    ↓
CLI Service Lifecycle
    ↓
SSH Tunnel Lifecycle
```

三层分别是：

1. CLI 工具实现层；
2. CLI 远端服务生命周期层；
3. SSH Tunnel 生命周期层。

这里的“CLI 远端服务”是运行在远端机器上的 Codex、OpenCode、Claude Code 服务端进程，不是 Panes 的业务服务，也不是本地协议客户端对象。

## 三、第一层：CLI 工具实现层

### 3.1 组成

本层包括：

- `CliTool` 统一接口；
- `CliToolFactory` 工厂；
- `CodexCli`、`OpenCodeCli`、`ClaudeCodeCli` 等具体实现；
- 各 CLI 自己的客户端运行服务和协议客户端对象；
- 调用统一接口的业务代码。

### 3.2 职责

本层只负责用户可感知的 CLI 业务，包括：

- 会话刷新、创建、恢复、归档和删除；
- 消息发送、追加、打断和审批；
- 模型、额度、Skill、MCP、插件和扩展；
- 图片及附件传输；
- 各 CLI 不同的协议、认证和返回值解析；
- 将结果转换为 Panes 的统一业务 DTO；
- 管理各 CLI 自己的客户端对象，例如 `CodexEngine`、`OpenCodeEngine`、`ClaudeRemoteEngine`。

各 CLI 的差异必须留在自己的实现或客户端运行服务中，不能塞进公共生命周期层。

### 3.3 使用远端服务的方式

CLI 实现需要远端服务时，只能通过：

```rust
cli_service_lifecycle::get(connection_id, cli_id)
```

取得已经就绪的服务入口，再建立或复用当前 CLI 自己的客户端对象。

CLI 实现不得：

- 直接调用 Tunnel 生命周期创建、启动、停止或释放远端 CLI 服务；
- 查询远端服务挂在哪个 Tunnel；
- 决定本地转发端口或远端服务端口；
- 管理 Tunnel 引用计数；
- 把 CLI 客户端对象登记到 CLI Service Lifecycle；
- 在 SSH 失败后回退到本机 CLI 或其他 CLI。

## 四、第二层：CLI 远端服务生命周期层

### 4.1 定位

`cli_service_lifecycle` 专门管理建立在 SSH Tunnel 上的远端 CLI 服务端进程。

服务由下面两个参数唯一确定：

```text
connection_id + cli_id
```

- `connection_id` 表示 SSH 连接配置；
- `cli_id` 表示 Codex、OpenCode、Claude Code 等 CLI 类型。

### 4.2 set 的完整语义

```rust
cli_service_lifecycle::set(connection_id, cli_id)
```

`set` 不是把外部已经创建好的服务放进 Map。它必须在生命周期内部完成完整创建过程：

1. 根据 `connection_id + cli_id` 检查是否已有 Ready 服务；
2. 通过底层 Tunnel 生命周期取得所需 Tunnel；
3. 在该 Tunnel 上启动当前 CLI 的远端服务端；
4. 等待并确认服务端已经就绪；
5. 只有全部成功后，才把服务登记为 Ready；
6. 并发调用同一组参数时，只允许创建一个服务；
7. 重复调用时复用已经 Ready 的服务。

服务创建失败时不得留下“已登记但不可用”的生命周期记录。

### 4.3 get 的完整语义

```rust
cli_service_lifecycle::get(connection_id, cli_id)
```

`get` 只返回启动阶段已经创建并登记为 Ready 的远端服务入口：

- 不临时启动服务；
- 不重新创建 Tunnel；
- 不返回 Tunnel 给业务层；
- 不创建任何 CLI 客户端 Engine；
- 服务不存在或正在终止时必须明确报错。

对上层只暴露连接远端服务所需的最小信息，例如本地入口、认证信息和服务代次。

### 4.4 终止语义

- `terminate(connection_id, cli_id)`：停止指定远端 CLI 服务并移除登记；
- `terminate_all()`：停止本进程登记的全部远端 CLI 服务；
- 应用退出时必须先停止 CLI 远端服务，再关闭底层 Tunnel。

### 4.5 明确禁止

CLI Service Lifecycle 不得管理：

- `CodexEngine`；
- `OpenCodeEngine`；
- `ClaudeRemoteEngine`；
- 会话、消息、模型、额度、Skill、MCP 等业务；
- CLI 业务 DTO；
- CLI 工厂和具体 CLI 实现。

它只管理远端服务端进程的创建、复用、就绪、停止和失效。

## 五、第三层：SSH Tunnel 生命周期层

### 5.1 定位

SSH Tunnel 生命周期层是 CLI 远端服务生命周期层的底层支持，只负责网络通道。

### 5.2 职责

- 按 `connection_id + cli_id` 管理 Tunnel；
- 建立和维护 SSH 连接及端口转发；
- 分配和维护本地端口、远端端口；
- 处理 Tunnel 创建、复用、恢复、失效和关闭；
- 提供远端服务启动、停止所需的底层执行能力；
- 处理连接变化和应用退出时的 Tunnel 清理。

### 5.3 明确禁止

Tunnel 生命周期层不得：

- 依赖 `CliTool`、CLI 工厂或具体 CLI 实现；
- 解析会话、消息、模型、额度、Skill、MCP 等业务协议；
- 保存任何 CLI 客户端 Engine；
- 成为业务调用方直接获取远端 CLI 服务的入口；
- 新建第二套重复的 Tunnel Map 或生命周期管理器。

## 六、固定调用流程

### 6.1 应用启动和远端会话刷新

```text
启动阶段确定 connection_id + cli_id
    ↓
cli_service_lifecycle::set(connection_id, cli_id)
    ↓
CLI Service Lifecycle 通过 Tunnel Lifecycle 启动并登记远端服务
    ↓
CliToolFactory 创建对应 CLI 实现
    ↓
业务通过 CliTool 统一接口刷新远端会话
    ↓
CLI 实现调用 cli_service_lifecycle::get(...)
    ↓
CLI 实现使用自己的客户端对象完成业务请求
```

必须先 `set`，再刷新会话。禁止先调用 CLI 业务、发现服务不存在后再临时通过 Tunnel 启动。

### 6.2 普通 CLI 业务

```text
业务调用方
    ↓
CliTool 统一接口
    ↓
当前 CLI 实现
    ↓
cli_service_lifecycle::get(connection_id, cli_id)
    ↓
当前 CLI 自己的协议客户端
    ↓
远端 CLI 服务端
```

业务层只关心业务操作，不知道 Tunnel、端口分配、远端进程位置和服务启动细节。

### 6.3 应用退出

```text
cli_service_lifecycle::terminate_all()
    ↓
停止全部远端 CLI 服务端
    ↓
关闭 SSH Tunnel
```

禁止先关闭 Tunnel，再尝试停止依赖该 Tunnel 的远端服务。

## 七、所有权矩阵

| 内容 | CLI 工具实现层 | CLI Service Lifecycle | SSH Tunnel Lifecycle |
| --- | --- | --- | --- |
| 统一业务接口和工厂 | 负责 | 不负责 | 不负责 |
| 各 CLI 业务差异 | 负责 | 不负责 | 不负责 |
| CLI 客户端 Engine | 负责 | 不负责 | 不负责 |
| 会话、消息、模型、扩展协议 | 负责 | 不负责 | 不负责 |
| 远端 CLI 服务端创建和就绪 | 不负责 | 负责 | 提供底层能力 |
| 远端 CLI 服务端复用和停止 | 不负责 | 负责 | 提供底层能力 |
| 服务代次和 Ready 状态 | 不负责 | 负责 | 不负责 |
| SSH 连接和端口转发 | 不负责 | 依赖 | 负责 |
| Tunnel Map 和恢复清理 | 不负责 | 不负责 | 负责 |

## 八、代码落点

### 8.1 CLI 工具实现层

- `src-tauri/src/cli_tools.rs`
- `src-tauri/src/cli_tools/codex.rs`
- `src-tauri/src/cli_tools/opencode.rs`
- `src-tauri/src/cli_tools/claude_code.rs`
- `src-tauri/src/remote_project_codex_runtime_service.rs`
- `src-tauri/src/remote_project_opencode_runtime_service.rs`
- `src-tauri/src/remote_project_claude_runtime_service.rs`

### 8.2 CLI 远端服务生命周期层

- `src-tauri/src/ssh/cli_service_lifecycle.rs`

### 8.3 SSH Tunnel 生命周期层

- `src-tauri/src/ssh/cli_tunnel_registry.rs`
- 其他 SSH 连接及转发基础设施。

### 8.4 启动编排

- `src-tauri/src/remote_project_session_refresh_service.rs`
- `src-tauri/src/lib.rs`

启动编排只负责按顺序调用三层，不得吸收任何一层的内部职责。

## 九、代码审查检查清单

涉及远端 CLI 的修改必须逐项确认：

1. 业务是否先进入 `CliTool` 统一接口和对应 CLI 实现；
2. CLI 实现是否只通过 `cli_service_lifecycle::get` 使用远端服务；
3. 是否不存在 CLI 实现直接启动、停止或释放 Tunnel 服务的代码；
4. `set` 是否在生命周期内部完成服务创建，而不是接收外部创建好的服务；
5. CLI Service Lifecycle 是否只保存服务状态，没有保存任何 Engine；
6. 各 CLI 客户端对象是否仍由各自实现层管理；
7. Tunnel 层是否只负责连接、端口和底层执行；
8. 启动刷新是否严格先 `set`、再调用 CLI 业务；
9. 退出是否严格先停止服务、再关闭 Tunnel；
10. 是否没有新增第二套生命周期 Map；
11. SSH 失败时是否明确报错，没有回退本机或其他 CLI；
12. `connection_id + cli_id` 的含义和唯一性是否保持不变。

任何实现只要违反上述调用方向，就必须先修正架构边界，再继续业务开发。
