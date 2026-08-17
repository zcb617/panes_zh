# CLI 工具层与 SSH 生命周期层边界设计

## 一、文档定位

本文规定统一 CLI 工具层与 SSH 生命周期层之间的固定职责边界。

相关文档：

- [多 CLI 工具统一接口架构设计](多%20CLI%20工具统一接口架构设计.md)
- [SshCliTunnelRegistry 生命周期管理](ssh-remote-project/SshCliTunnelRegistry%20声明周期管理.md)

本文重点解决以下问题：

1. Claude Code、Codex、OpenCode 的业务代码应当放在哪里；
2. SSH tunnel 应当由谁创建、复用和关闭；
3. 远端 CLI 服务应当由谁启动、连接、检查和调用；
4. 业务调用方和基础设施调用方分别应当进入哪一层；
5. 如何避免把 CLI 工具类变成 tunnel 生命周期代理。

## 二、两层模型

系统只有两个职责明确的层次。

### 2.1 SSH 生命周期层是网络通道层

SSH 生命周期层相当于“高速公路”。它负责建立和维护可以到达远端机器的网络通道。

它负责：

- 维护全局 `hostId -> cliId -> tunnel` MAP；
- 根据 `hostId + cliId` 创建、取得、复用和关闭 SSH tunnel；
- 维护 tunnel 的临时占用和持续占用；
- 维护 tunnel 相关引用计数；
- 处理连接恢复、连接变化、程序退出和 tunnel 清理；
- 向调用方返回可用的 tunnel、本地映射端口或等价网络连接入口。

它返回的是 tunnel 或网络连接入口，不是 Codex、OpenCode 或 Claude Code 的业务服务对象。

它不负责：

- 判断 Claude Code、Codex 或 OpenCode 是否已具备业务就绪状态；
- 创建或解释某个 CLI 的业务请求；
- 调用 `/sessions`、模型目录、审批或消息协议；
- 解析 CLI 返回的会话、模型、扩展和消息事件；
- 判断某个 `cwd` 属于哪个 workspace；
- 更新会话列表或业务数据库。

### 2.2 CLI 工具层是远端服务和 CLI 业务层

CLI 工具层相当于“在高速公路上运行的车”。每个工具实现负责自己的 CLI 服务和业务协议。

`CodexCli`、`OpenCodeCli` 和 `ClaudeCodeCli` 分别负责：

- 根据当前执行目标选择本机实现或 SSH 远端实现；
- SSH 场景下向生命周期层申请并取得当前 CLI 的 tunnel；
- 通过取得的 tunnel 启动、连接、复用和检查当前 CLI 的远端服务；
- 建立和维护当前 CLI 的协议客户端或运行时对象；
- 查询模型、会话、扩展、健康状态和用量；
- 创建、恢复、发送、取消和审批会话；
- 在每次业务请求中携带当前项目的 `cwd`；
- 解析当前 CLI 的响应和事件；
- 将结果转换成 Panes 现有业务 DTO。

CLI 工具层可以为了完成自己的业务向生命周期层申请 tunnel，但不能替其他基础设施调用方代理 tunnel 操作。

## 三、固定调用方向

### 3.1 CLI 业务调用

调用目的属于模型、会话、消息、审批、扩展或 CLI 健康状态时，必须进入当前 CLI 工具类：

```text
业务调用方
    ↓
CliTool 统一接口
    ↓
当前 CLI 实现
    ↓
向 SSH 生命周期层申请并取得 tunnel
    ↓
当前 CLI 实现通过 tunnel 创建或连接远端 CLI 服务
    ↓
当前 CLI 实现执行协议和业务操作
```

例如，Claude Code 查询远端会话的正确调用链是：

```text
ClaudeCodeCli::list_sessions
    ↓
向 SSH 生命周期层取得 Claude 对应的 tunnel
    ↓
ClaudeCodeCli 通过 tunnel 连接 Claude 远端服务
    ↓
ClaudeCodeCli 调用 /sessions
    ↓
ClaudeCodeCli 校验 cwd、解析会话并返回业务结果
```

`list_sessions` 属于 Claude Code 业务，必须保留在 `ClaudeCodeCli`。取得 tunnel 只是该业务方法内部的一步。

### 3.2 SSH 基础设施调用

调用目的本身属于 tunnel、端口转发、连接恢复、占用或清理时，调用方必须直接进入 SSH 生命周期层：

```text
基础设施调用方
    ↓
SshCliTunnelRegistry
```

禁止写成：

```text
基础设施调用方
    ↓
ClaudeCodeCli / CodexCli / OpenCodeCli
    ↓
SshCliTunnelRegistry
```

CLI 工具类不是生命周期层的门面、代理或转发器。外部函数如果需要 tunnel，就直接调用生命周期层，不能借道某个 CLI 工具类。

### 3.3 禁止反向依赖

SSH 生命周期层不得反向依赖 `CliTool`、`ClaudeCodeCli`、`CodexCli` 或 `OpenCodeCli`。

生命周期层不理解 CLI 的业务协议；CLI 工具层也不拥有全局 tunnel MAP。

## 四、预热的业务边界

`prewarm_engine` 属于 CLI 工具层，它的统一业务含义是：

> 确认当前执行目标上的当前 CLI 已经可以接受业务请求。

对 Claude Code 而言：

```text
ClaudeCodeCli::prewarm_engine
    ↓
本机目标：启动或确认本机 Claude Code 运行入口并建立协议连接

ClaudeCodeCli::prewarm_engine
    ↓
SSH 目标：向生命周期层取得 tunnel
    ↓
通过 tunnel 启动或确认远端 Claude Code 服务
    ↓
建立 Claude Code 协议连接
    ↓
确认服务可以接受请求
```

`hostId + cliId`、tunnel 占用和引用计数是生命周期层的内部机制。CLI 工具类只提出“当前业务需要 tunnel”的申请，不直接维护这些数据。

预热不得：

- 新建第二套 tunnel MAP；
- 按 workspace 创建 tunnel；
- 修改 tunnel 引用计数规则；
- 把 SSH 失败回退成本机 CLI；
- 让生命周期层解析 Claude Code、Codex 或 OpenCode 的就绪协议。

## 五、项目目录与服务复用

管理单位固定为：

```text
hostId + cliId
```

`workspaceId`、项目根目录和 `threadId` 不加入 tunnel MAP 的 key。

同一台远端机器上的多个项目共用同一种 CLI 的 tunnel 和远端服务入口。项目切换通过业务请求中的 `cwd` 完成：

```text
同一 hostId + claude
    ├── workspace A 请求携带 cwd=/project/a
    └── workspace B 请求携带 cwd=/project/b
```

CLI 工具类必须校验：

- 当前 workspace 是本机还是 SSH；
- SSH workspace 是否绑定正式连接；
- 请求中的项目目录是否属于当前 workspace；
- 恢复的会话是否同时匹配 session ID 和 `cwd`。

SSH 失败、session 不存在或 `cwd` 不匹配时必须明确报错，不得调用本机 CLI 创建替代会话。

## 六、CLI 工具接口职责

统一 `CliTool` 接口负责表达用户可感知的 CLI 业务操作，包括：

- 获取模型和运行状态；
- 检查当前 CLI 是否可用；
- 查询和恢复会话；
- 创建会话和发送消息；
- 审批与取消；
- 查询扩展；
- 执行当前 CLI 已支持的业务操作。

调用方根据当前 `cliId` 取得对应实现：

```rust
let cli_tool: &dyn CliTool = cli_tool_resolver.resolve(cli_id)?;
```

解析器只负责：

```text
cliId -> CLI 业务实现
```

解析器不得保存或管理：

- SSH tunnel；
- tunnel 端口；
- tunnel 占用；
- tunnel 引用计数；
- 全局 tunnel 状态。

具体 CLI 实现可以保存自己的协议客户端、远端服务连接和完成当前业务所需的使用凭证，但不能把这些资源暴露成供外部基础设施调用的 tunnel 接口。

## 七、能力差异

三个 CLI 共用同一个接口，不表示它们支持完全相同的业务能力。

调用方必须先读取 `capabilities()`，只向当前 CLI 调用其已经支持的业务操作。

例如，当前 Panes 的 Claude Code 接入没有 Codex 的会话分支、回滚、压缩和代码审查业务入口。正常 Claude Code 页面不得显示或调用这些入口。

各实现中保留的不支持结果只用于防止程序内部错误分发，不属于正常用户流程，也不作为 Claude Code 的功能或验收项。

## 八、所有权矩阵

| 内容 | CLI 工具层 | SSH 生命周期层 |
| --- | --- | --- |
| 当前 CLI 的业务能力 | 负责 | 不负责 |
| CLI 请求和响应协议 | 负责 | 不负责 |
| CLI 远端服务启动、连接和业务就绪检查 | 负责 | 不负责 |
| CLI 协议客户端和运行时对象 | 负责 | 不负责 |
| 模型、扩展、会话、消息和审批 | 负责 | 不负责 |
| 当前项目 `cwd` | 负责 | 不负责 |
| `hostId + cliId -> tunnel` MAP | 不负责 | 负责 |
| tunnel 创建、取得、复用和关闭 | 申请和使用 | 负责 |
| tunnel 临时占用和持续占用 | 申请和释放 | 记录和管理 |
| tunnel 引用计数 | 不负责 | 负责 |
| tunnel 恢复和退出清理 | 不负责 | 负责 |
| 为外部调用方代理 tunnel | 禁止 | 不适用 |

## 九、代码归属判断

### 9.1 归入 CLI 工具目录

满足以下条件的代码归入对应 CLI 工具目录：

- 只服务于 Codex、OpenCode 或 Claude Code 中的一种；
- 启动、连接或检查该 CLI 的远端服务；
- 创建该 CLI 的协议客户端；
- 调用该 CLI 的模型、会话、扩展、审批或消息协议；
- 解析该 CLI 的响应和事件；
- 使用 `cwd` 构造当前项目的 CLI 请求；
- 将 CLI 结果转换成 Panes 业务 DTO。

### 9.2 保留在 SSH 生命周期层

满足以下条件的代码保留在 SSH 生命周期层：

- 持有或操作全局 tunnel MAP；
- 创建、取得、复用或关闭 SSH tunnel；
- 分配本地或远端转发端口；
- 维护 tunnel 临时占用或持续占用；
- 维护 tunnel 引用计数；
- 处理 SSH 连接恢复、连接变化和退出清理。

以下文件是明确保护对象：

```text
src-tauri/src/ssh/cli_tunnel_registry.rs
```

不得在 CLI 工具重构中修改其 MAP 层级、key、占用关系和既有清理流程。

### 9.3 混合代码的判断方法

同一段代码同时出现 CLI 名称和 tunnel 调用时，按调用目的判断：

- 为了执行某个 CLI 业务而取得 tunnel：代码属于 CLI 工具层；
- 为了恢复、枚举、关闭或清理 tunnel：代码属于 SSH 生命周期层；
- CLI 工具层取得 tunnel 后执行 `/sessions`、模型、消息或审批协议：协议部分属于 CLI 工具层；
- 基础设施调用方只需要 tunnel：直接调用生命周期层，不经过 CLI 工具类。

## 十、禁止事项

1. 修改全局 tunnel MAP 的层级和 key。
2. 把 `workspaceId`、项目目录或 `threadId` 加入 tunnel MAP 的 key。
3. 新建第二套 tunnel MAP 或 tunnel 生命周期管理器。
4. 让 CLI 工具类持有或复制全局 tunnel MAP。
5. 让 CLI 工具类自行维护 tunnel 引用计数。
6. 让外部基础设施调用方借道 CLI 工具类操作 tunnel。
7. 让 SSH 生命周期层依赖 CLI 工具接口。
8. 让 SSH 生命周期层解析 CLI 模型、会话、扩展、消息或审批协议。
9. 把 `list_sessions`、模型查询或预热业务移动到 SSH 生命周期层。
10. SSH 失败后回退本机 CLI、本机目录或其他 CLI。
11. 因结构重构改变已有 IPC、数据来源和页面行为。

## 十一、稳定调用关系

### 11.1 CLI 业务

```text
业务调用方
    ↓
CliTool
    ↓
当前 CLI 实现
    ↓
SshCliTunnelRegistry 提供 tunnel
    ↓
当前 CLI 实现创建或连接远端 CLI 服务
    ↓
当前 CLI 实现执行协议和业务
```

### 11.2 SSH 基础设施

```text
连接恢复、tunnel 枚举、关闭或清理
    ↓
SshCliTunnelRegistry
```

两条调用链不得互相借道。

## 十二、实施检查清单

修改 CLI 工具代码前必须确认：

1. 当前操作的目的属于 CLI 业务还是 SSH tunnel 基础设施。
2. CLI 业务是否先进入当前 `CliTool` 实现。
3. SSH 场景是否由当前 CLI 实现向生命周期层取得 tunnel。
4. 当前 CLI 的远端服务、协议客户端和业务解析是否仍位于 CLI 工具层。
5. 外部基础设施调用是否直接进入生命周期层，而不是借道 CLI 工具类。
6. 生命周期层是否仍然只返回 tunnel 或网络入口，而不返回 CLI 业务对象。
7. `cwd` 是否由 CLI 业务请求携带，而不是加入 tunnel MAP。
8. SSH 失败是否明确报错，且没有本机或其他 CLI 回退。
9. 是否保持现有 tunnel MAP、管理单位、占用关系和清理流程不变。
10. 是否没有让生命周期层反向依赖 CLI 工具代码。

任何实现只要违反上述调用方向，就必须先修正边界，再继续业务接入。
