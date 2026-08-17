# 多 CLI 工具统一接口架构设计

## 一、文档定位

本文定义 Panes 中 Codex、OpenCode、Claude Code 等 CLI 工具的统一接口架构。

本架构的核心要求是：

1. 每个 CLI 工具都必须拥有以该 CLI 工具命名的独立目录。
2. 与某个 CLI 工具相关的实现必须放在该 CLI 工具自己的目录内。
3. 相同业务能力必须先定义统一接口，再由各 CLI 工具分别实现。
4. 调用方只能依赖统一接口，不得直接依赖某个 CLI 工具的内部函数。
5. 当前选择哪个 CLI 工具，就通过统一注册表取得哪个 CLI 工具的接口实现。
6. 本机目标和 SSH 目标共用接口，但由当前 CLI 工具在其目录内部选择对应实现。
7. 任一 CLI 工具失败时，禁止回退到其他 CLI 工具或本机目录。

本文是架构设计文档，不代表相关目录和接口已经完成代码迁移。

## 二、需求事实

| 需求 ID | 原始要求 | 架构要求 |
| --- | --- | --- |
| U-01 | 每一个 CLI 工具都应有一个以 CLI 工具命名的目录 | Codex、OpenCode、Claude Code 分别拥有独立实现目录 |
| U-02 | 所有与 CLI 工具相关的内容都独立放置 | CLI 的运行时、会话、扩展、附件和协议实现归入自身目录 |
| U-03 | 当前是什么 CLI 工具，就调用什么目录里的函数 | 通过 CLI 注册表按 `cliId` 解析具体实现 |
| U-04 | 接口必须统一，类似 Java `interface` | Rust 使用 `trait`，TypeScript 使用 `interface` |
| U-05 | 获取可用扩展时，先定义接口，各 CLI 分别实现 | 定义 `CliExtensionProvider`，三个 CLI 分别实现 |
| U-06 | 调用位置实例化接口 | 调用方持有接口类型，由工厂或注册表创建、返回具体实现 |

## 三、架构原则

### 3.1 调用方依赖接口

调用方不得包含以下实现判断：

```text
如果是 Codex，就调用 Codex 函数；
如果是 OpenCode，就调用 OpenCode 函数；
如果是 Claude Code，就调用 Claude Code 函数。
```

调用方只允许执行：

```text
根据当前 cliId 取得统一接口实例；
调用统一接口方法；
消费统一返回结果。
```

CLI 差异由各 CLI 工具自己的实现负责处理。

### 3.2 接口类型与实现类型分离

“实例化接口”的准确含义是：

1. 调用方变量声明为接口类型；
2. 工厂或注册表根据当前 `cliId` 创建或取得具体实现；
3. 调用方通过接口变量调用方法；
4. 调用方不知道实际对象是 Codex、OpenCode 还是 Claude Code。

Java 形式如下：

```java
CliExtensionProvider provider =
    CliExtensionProviderFactory.create(currentCliId);

CliExtensionCatalog catalog =
    provider.getAvailableExtensions(context);
```

Rust 中对应为 `dyn Trait`：

```rust
let provider: Arc<dyn CliExtensionProvider> =
    registry.resolve_extension_provider(&cli_id)?;

let catalog = provider
    .get_available_extensions(&context)
    .await?;
```

### 3.3 CLI 实现严格隔离

- Codex 实现不得读取 OpenCode 或 Claude Code 的目录。
- OpenCode 实现不得读取 Codex 或 Claude Code 的目录。
- Claude Code 实现不得读取 Codex 或 OpenCode 的目录。
- SSH 实现不得读取本机扩展目录作为后备数据。
- 一个 CLI 查询失败时必须返回该 CLI 的明确错误。
- 禁止使用其他 CLI、其他机器或其他 workspace 的历史数据冒充当前目录。

### 3.4 公共目录只存放契约

公共目录只允许放置：

- 统一接口；
- 统一输入、输出 DTO；
- CLI 注册表；
- 无 CLI 业务含义的基础类型。

CLI 的查询、解析、协议转换、SSH 路由和错误处理不得放进公共目录。

## 四、目标目录结构

### 4.1 Rust 后端

```text
src-tauri/src/cli_tools/
├── mod.rs
├── contracts/
│   ├── mod.rs
│   ├── context.rs
│   ├── runtime.rs
│   ├── conversation.rs
│   ├── extensions.rs
│   └── attachments.rs
├── registry.rs
├── codex/
│   ├── mod.rs
│   ├── tool.rs
│   ├── local.rs
│   ├── ssh.rs
│   ├── runtime.rs
│   ├── conversation.rs
│   ├── extensions.rs
│   ├── attachments.rs
│   └── protocol.rs
├── opencode/
│   ├── mod.rs
│   ├── tool.rs
│   ├── local.rs
│   ├── ssh.rs
│   ├── runtime.rs
│   ├── conversation.rs
│   ├── extensions.rs
│   ├── attachments.rs
│   └── protocol.rs
└── claude_code/
    ├── mod.rs
    ├── tool.rs
    ├── local.rs
    ├── ssh.rs
    ├── runtime.rs
    ├── conversation.rs
    ├── extensions.rs
    ├── attachments.rs
    └── protocol.rs
```

目录职责：

- `contracts/`：只定义统一接口和统一 DTO。
- `registry.rs`：只负责按 `cliId` 返回接口实例。
- `codex/`：只包含 Codex 实现。
- `opencode/`：只包含 OpenCode 实现。
- `claude_code/`：只包含 Claude Code 实现。

### 4.2 TypeScript 前端

```text
src/cli-tools/
├── contracts/
│   ├── context.ts
│   ├── runtime.ts
│   ├── extensions.ts
│   └── slash-command.ts
├── registry.ts
├── codex/
│   ├── adapter.ts
│   └── slash-command.ts
├── opencode/
│   ├── adapter.ts
│   └── slash-command.ts
└── claude-code/
    ├── adapter.ts
    └── slash-command.ts
```

前端 CLI 目录只负责前端行为，例如：

- 将统一扩展目录转换成当前 CLI 的 `/` 菜单项目；
- 处理用户选择某类扩展后的输入框行为；
- 展示当前 CLI 支持的能力；
- 调用统一 IPC。

前端不得重新实现远端查询和 SSH 路由。

## 五、统一执行上下文

所有 CLI 接口必须接收统一上下文：

```rust
pub struct CliExecutionContext {
    pub cli_id: CliId,
    pub target_key: String,
    pub workspace_id: String,
    pub root_path: String,
    pub location_kind: CliLocationKind,
    pub ssh_connection_id: Option<String>,
}
```

字段含义：

| 字段 | 必填 | 含义 |
| --- | --- | --- |
| `cli_id` | 是 | 当前 CLI 工具稳定标识 |
| `target_key` | 是 | 当前执行机器稳定标识，例如 `local` 或 `ssh:<connection_id>` |
| `workspace_id` | 是 | 当前 workspace 稳定 ID |
| `root_path` | 是 | 当前目标机器上的项目根目录 |
| `location_kind` | 是 | 本机或 SSH |
| `ssh_connection_id` | SSH 时必填 | SSH 连接稳定 ID |

安全约束：

- `root_path` 必须由后端根据 `workspace_id` 读取并校验。
- SSH 连接必须由后端根据 workspace 绑定关系解析。
- 前端不得传入任意连接覆盖 workspace 的正式绑定。
- `cli_id`、`target_key` 和 workspace 目标不匹配时必须拒绝执行。

## 六、统一接口体系

### 6.1 顶层 CLI 工具接口

```rust
#[async_trait]
pub trait CliTool: Send + Sync {
    fn id(&self) -> CliId;

    fn runtime_provider(&self) -> &dyn CliRuntimeProvider;

    fn conversation_provider(&self) -> &dyn CliConversationProvider;

    fn extension_provider(&self) -> &dyn CliExtensionProvider;

    fn attachment_provider(&self) -> &dyn CliAttachmentProvider;
}
```

顶层接口只负责暴露稳定业务能力，不实现任何具体 CLI 逻辑。

### 6.2 运行时接口

```rust
#[async_trait]
pub trait CliRuntimeProvider: Send + Sync {
    async fn health(
        &self,
        context: &CliExecutionContext,
    ) -> anyhow::Result<CliHealth>;

    async fn models(
        &self,
        context: &CliExecutionContext,
    ) -> anyhow::Result<Vec<CliModel>>;

    async fn account(
        &self,
        context: &CliExecutionContext,
    ) -> anyhow::Result<Option<CliAccount>>;

    async fn usage(
        &self,
        context: &CliExecutionContext,
    ) -> anyhow::Result<Option<CliUsage>>;
}
```

### 6.3 对话接口

```rust
#[async_trait]
pub trait CliConversationProvider: Send + Sync {
    async fn list_sessions(
        &self,
        context: &CliExecutionContext,
    ) -> anyhow::Result<Vec<CliSession>>;

    async fn send_message(
        &self,
        context: &CliExecutionContext,
        request: CliSendMessageRequest,
    ) -> anyhow::Result<()>;

    async fn cancel(
        &self,
        context: &CliExecutionContext,
        session_id: &str,
    ) -> anyhow::Result<()>;

    async fn respond_to_approval(
        &self,
        context: &CliExecutionContext,
        request: CliApprovalResponse,
    ) -> anyhow::Result<()>;
}
```

### 6.4 可用扩展接口

```rust
#[async_trait]
pub trait CliExtensionProvider: Send + Sync {
    async fn get_available_extensions(
        &self,
        context: &CliExecutionContext,
    ) -> anyhow::Result<CliExtensionCatalog>;
}
```

所有 CLI 工具必须实现该接口：

```rust
impl CliExtensionProvider for CodexExtensionProvider { /* Codex 实现 */ }
impl CliExtensionProvider for OpenCodeExtensionProvider { /* OpenCode 实现 */ }
impl CliExtensionProvider for ClaudeCodeExtensionProvider { /* Claude Code 实现 */ }
```

### 6.5 附件接口

```rust
#[async_trait]
pub trait CliAttachmentProvider: Send + Sync {
    async fn prepare_attachments(
        &self,
        context: &CliExecutionContext,
        attachments: Vec<CliAttachment>,
    ) -> anyhow::Result<Vec<CliPreparedAttachment>>;
}
```

各 CLI 自己决定最终协议格式，但 SSH 模式必须先把附件上传到远端，禁止把本机路径传给远端 CLI。

## 七、扩展目录统一模型

### 7.1 统一返回对象

```rust
pub struct CliExtensionCatalog {
    pub cli_id: CliId,
    pub target_key: String,
    pub workspace_id: String,
    pub items: Vec<CliExtensionItem>,
}
```

```rust
pub struct CliExtensionItem {
    pub id: String,
    pub kind: CliExtensionKind,
    pub name: String,
    pub description: Option<String>,
    pub enabled: bool,
    pub scope: Option<String>,
    pub source: Option<String>,
    pub path: Option<String>,
    pub health: Option<String>,
}
```

统一类型：

```rust
pub enum CliExtensionKind {
    Skill,
    Plugin,
    Agent,
    Command,
    Mcp,
}
```

不同 CLI 不支持的类型不返回对应项目：

| CLI | 扩展类型 |
| --- | --- |
| Codex | Skill、Plugin、MCP |
| OpenCode | Agent、Command、MCP |
| Claude Code | Skill、Plugin、Command、MCP |

### 7.2 Codex 实现

文件：

```text
cli_tools/codex/extensions.rs
```

职责：

- 本机目标查询本机 Codex。
- SSH 目标通过 Codex SSH tunnel 查询远端 Codex。
- 使用当前 workspace 的远端 `root_path`。
- 读取 Skills、Plugins 和 MCP。
- 不读取 OpenCode、Claude Code 或本机后备数据。

### 7.3 OpenCode 实现

文件：

```text
cli_tools/opencode/extensions.rs
```

职责：

- 本机目标查询本机 OpenCode。
- SSH 目标通过 OpenCode 服务查询远端 OpenCode。
- 读取 Agents、Commands 和 MCP。
- `/` 菜单的 MCP 必须来自当前 `OpenCodeExtensionProvider` 返回值。
- 不使用本机扩展目录代替 SSH 数据。

### 7.4 Claude Code 实现

文件：

```text
cli_tools/claude_code/extensions.rs
```

职责：

- 本机目标查询本机 Claude Code。
- SSH 目标通过 Claude Code SSH tunnel 查询远端 Claude Code。
- 读取 Skills、Plugins、Commands 和 MCP。
- 所有远端路径必须来自当前 SSH 目标。
- 禁止读取本机 `~/.claude` 作为 SSH 后备数据。

## 八、注册表和实例化

### 8.1 注册表

```rust
pub struct CliToolRegistry {
    tools: HashMap<CliId, Arc<dyn CliTool>>,
}

impl CliToolRegistry {
    pub fn resolve(&self, cli_id: &CliId) -> anyhow::Result<Arc<dyn CliTool>> {
        self.tools
            .get(cli_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("unsupported CLI: {cli_id}"))
    }
}
```

应用启动时注册具体实现：

```rust
registry.register(Arc::new(CodexTool::new(...)));
registry.register(Arc::new(OpenCodeTool::new(...)));
registry.register(Arc::new(ClaudeCodeTool::new(...)));
```

调用时只取得接口实例：

```rust
let cli_tool = registry.resolve(&context.cli_id)?;
let provider = cli_tool.extension_provider();
let catalog = provider.get_available_extensions(&context).await?;
```

### 8.2 生命周期约束

- 注册表中的 CLI 工具实现可以是长生命周期实例。
- CLI 工具实例不得自行创建第二套 SSH 生命周期管理器。
- SSH tunnel 和远端服务继续使用统一生命周期注册表。
- CLI 工具实现只通过已有生命周期接口申请和释放使用权。
- `hostId + cliId` 仍对应唯一机器级运行时。

## 九、统一 IPC

前端不再分别调用：

```text
list_codex_skills
list_codex_plugins
get_opencode_runtime_catalog
get_ssh_remote_claude_extension_catalog
```

目标接口为：

```text
get_cli_available_extensions
```

请求：

```ts
interface GetCliAvailableExtensionsRequest {
  cliId: CliToolId;
  workspaceId: string;
}
```

响应：

```ts
interface CliExtensionCatalog {
  cliId: CliToolId;
  targetKey: string;
  workspaceId: string;
  items: CliExtensionItem[];
}
```

后端处理流程：

```text
接收 cliId 和 workspaceId
    ↓
后端读取 workspace 和执行目标
    ↓
构造 CliExecutionContext
    ↓
CliToolRegistry.resolve(cliId)
    ↓
CliExtensionProvider.get_available_extensions(context)
    ↓
返回统一 CliExtensionCatalog
```

## 十、前端调用结构

### 10.1 CLI 适配器接口

```ts
export interface CliToolAdapter {
  readonly id: CliToolId;

  getAvailableExtensions(
    context: CliExecutionContext,
  ): Promise<CliExtensionCatalog>;

  buildSlashItems(
    catalog: CliExtensionCatalog,
  ): SlashCommandItem[];

  applySlashItem(
    item: SlashCommandItem,
    composer: ComposerContext,
  ): void;
}
```

### 10.2 前端注册表

```ts
const cliTool = cliToolRegistry.resolve(selectedCliId);
const catalog = await cliTool.getAvailableExtensions(context);
const slashItems = cliTool.buildSlashItems(catalog);
```

`ChatPanel` 只负责：

- 读取当前 `selectedCliId`；
- 从注册表取得接口实例；
- 展示接口返回的菜单；
- 把用户选择交还当前接口实例处理。

`ChatPanel` 不得负责：

- 判断某个 CLI 应调用哪个具体 IPC；
- 解析某个 CLI 的原始协议；
- 把其他 CLI 的目录作为后备数据；
- 在同一个数组中混合三个 CLI 的命令后再依赖 `disabled` 隐藏。

## 十一、错误模型

统一错误类型：

```rust
pub enum CliToolError {
    UnsupportedCli,
    InvalidExecutionTarget,
    ConnectionUnavailable,
    RuntimeUnavailable,
    AuthenticationRequired,
    CatalogUnavailable,
    ProtocolError,
    Timeout,
}
```

每个错误必须同时包含：

- `cliId`；
- `targetKey`；
- `workspaceId`；
- 用户可见说明；
- 技术详情；
- 是否允许重试。

错误信息必须明确指出当前 CLI 和当前目标，禁止用模糊的“扩展读取失败”掩盖数据来源。

## 十二、现有代码迁移原则

### 12.1 先建立接口，不先改页面分支

迁移顺序：

1. 定义统一接口和 DTO。
2. 建立 CLI 注册表。
3. Codex 实现统一接口，并通过现有功能回归验证。
4. OpenCode 实现统一接口。
5. Claude Code 实现统一接口。
6. 前端改为只调用统一接口。
7. 最后移除散落在调用方的 CLI 分支。

### 12.2 保持现有稳定能力

迁移期间必须保持：

- CLI 和模型选择功能不变；
- 本机项目行为不变；
- SSH workspace 不回退本机；
- 三个 CLI 已完成的正式消息协议不变；
- SSH tunnel 统一生命周期不变；
- 附件远端路径语义不变。

### 12.3 禁止一次性重写

每个 CLI 接入统一接口后都必须先完成独立验证，再切换调用入口。未验证的新实现不得覆盖已有稳定实现。

## 十三、测试设计

### 13.1 接口契约测试

同一套测试分别运行三个实现：

- 返回对象包含正确的 `cliId`；
- 返回对象包含正确的 `targetKey`；
- 返回项目只属于当前 CLI；
- SSH 目标中不出现本机路径；
- 查询失败时不返回其他 CLI 数据。

### 13.2 注册表测试

- `codex` 返回 Codex 实现；
- `opencode` 返回 OpenCode 实现；
- `claude` 返回 Claude Code 实现；
- 未知 `cliId` 返回明确错误；
- 注册表不会返回错误 CLI 的实例。

### 13.3 `/` 菜单测试

- Codex 只显示 Codex 命令、Skills、Plugins、MCP。
- OpenCode 只显示 OpenCode Agents、Commands、MCP。
- Claude Code 只显示 Claude Code Skills、Plugins、Commands、MCP。
- Claude Code 中不得出现 Codex Review、Fork、Rollback 等命令。

### 13.4 回归测试

- 三个 CLI 均可选择。
- 每个 CLI 的模型均可选择。
- 本机与 SSH 目标均能正确切换。
- 消息发送、取消、审批和附件行为不受接口迁移影响。

## 十四、验收标准

1. 每个 CLI 工具都有独立命名目录。
2. 每个相同业务能力都有统一接口定义。
3. 三个 CLI 分别实现统一接口。
4. 调用方只持有接口类型，不直接调用具体 CLI 函数。
5. 当前选择哪个 CLI，就只调用该 CLI 的实现。
6. SSH 目标不会读取本机扩展目录。
7. 任一 CLI 失败时不会回退到其他 CLI。
8. `/` 菜单、运行时信息和实际选择行为使用同一当前 CLI 数据源。
9. CLI 选择和模型选择功能保持正常。
