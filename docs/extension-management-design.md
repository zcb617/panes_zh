# 统一扩展管理中心设计方案

- 状态：首期实现完成
- 最后核验：2026-07-17
- 适用项目：Panes 桌面端
- 首期代理范围：Codex、Claude Code、OpenCode

## 1. 结论

Panes 应在侧边栏“智能体”下方新增“扩展”入口，提供统一的技能、插件和 MCP 管理中心。

页面采用与 Codex 技能/插件页面相近的信息层级，但继续遵守 Panes 自己的产品原则：始终显示当前代理、工作区和运行状态，常用操作优先，危险操作明确说明后果。

首期不应把现有全部终端 Harness 都包装成“已支持扩展管理”。当前项目虽然可以启动 8 种终端 CLI，但真正接入聊天 EngineManager、且具备可复用运行时数据的只有 Codex、Claude Code 和 OpenCode。其他 Harness 应在各自具备稳定查询和管理接口后逐个接入。

统一管理的关键不是新增一个孤立页面，而是建立一个扩展领域模型和单一数据源：

- “已安装 / 已配置”展示本机实际存在的全部项目，不要求它同时存在于官方目录。
- “可用”只展示代理官方目录返回的项目；官方没有对应目录时，该类型没有可用列表。
- 本机已存在但未出现在官方目录中的项目照常展示，不增加特殊标签或警告。
- 对话中的斜杠菜单只展示当前会话真正可调用的项目。
- 两处使用同一个 Store 和同一套后端适配器，避免状态含义不一致。

Panes 只负责客观呈现和管理，不对目录项目进行排名、审核、认证或安全背书。“来自官方目录”只表示目录数据由对应代理官方提供或明确标识，不表示 Panes 或 OpenAI 对项目代码、病毒、隐私和质量作出保证。

## 2. 目标

### 2.1 产品目标

1. 用户可以在一个页面查看当前代理的技能、插件和 MCP。
2. 用户可以明确区分已安装、未安装、已配置、已启用、异常和需要认证等状态。
3. 用户可以切换 Codex、Claude Code、OpenCode，并看到各自真实支持的能力。
4. 用户可以看到扩展所属的全局、项目、插件、内置或托管作用域。
5. 管理页状态与对话中的实际可调用状态保持一致。
6. 后续可以加入安装、卸载、启停、认证和官方目录刷新。

### 2.2 非目标

首期不承诺以下内容：

- 不承诺展示“互联网上全部技能、插件和 MCP”。
- 不聚合第三方市场、社区清单或 Panes 自建目录。
- 不把任意已配置来源、包管理器搜索结果或网页抓取结果当成官方可用目录。
- 不提供推荐、排行榜、编辑精选、代码审核、病毒审核、安全认证或质量背书。
- 不因为本机已安装项目未出现在官方目录中而隐藏它或增加特殊标签。
- 不要求三个代理拥有完全相同的操作能力。
- 不在首期接入所有终端 Harness。
- 不允许前端拼接或执行任意 CLI 命令。
- 不通过 IPC、日志或界面暴露 MCP 密钥、Header、Token 或敏感环境变量。

## 3. 当前实现与证据

### 3.1 页面与导航

- 当前 ActiveView 只有 chat、harnesses 和 settings，新增页面需要扩展该联合类型：
  [src/stores/uiStore.ts](../src/stores/uiStore.ts#L22)。
- “智能体”侧边栏入口当前直接切换 harnesses 页面：
  [src/components/sidebar/Sidebar.tsx](../src/components/sidebar/Sidebar.tsx#L324)。
- 主内容区由 ThreeColumnLayout 根据 ActiveView 决定展示 HarnessPanel、SettingsPage 或聊天：
  [src/components/layout/ThreeColumnLayout.tsx](../src/components/layout/ThreeColumnLayout.tsx#L234)。

因此新增 extensions 视图和对应路由分支即可接入现有页面体系，不需要建立新的窗口或路由框架。

### 3.2 代理范围

终端 Harness 当前包含 Codex、Claude Code、Gemini CLI、Antigravity、Kiro、OpenCode、Kilo Code 和 Factory Droid：
[src-tauri/src/commands/harness.rs](../src-tauri/src/commands/harness.rs#L40)。

聊天 EngineManager 当前只注册 Codex、Claude 和 OpenCode：
[src-tauri/src/engines/mod.rs](../src-tauri/src/engines/mod.rs#L454)。

这意味着“可以启动某个 CLI”与“可以可靠读取和管理该 CLI 的扩展”是两种不同能力。首期下拉框只展示 Codex、Claude Code 和 OpenCode。

### 3.3 已有扩展数据

项目已经定义以下前端类型：

- CodexSkill
- CodexPluginMarketplace
- CodexPlugin
- CodexMcpServer
- OpenCodeRuntimeCatalog
- OpenCodeMcpServer

定义位置：
[src/types.ts](../src/types.ts#L547)。

已有后端能力包括：

- Codex 技能列表：skills/list。
- Codex 插件市场：plugin/list。
- Codex MCP 运行状态：mcpServerStatus/list。
- OpenCode Agent、命令和 MCP 运行时目录。

相关实现：

- [src-tauri/src/engines/codex.rs](../src-tauri/src/engines/codex.rs#L5486)
- [src-tauri/src/engines/opencode.rs](../src-tauri/src/engines/opencode.rs#L1112)
- [src-tauri/src/commands/engines.rs](../src-tauri/src/commands/engines.rs#L57)
- [src/lib/ipc.ts](../src/lib/ipc.ts#L352)

### 3.4 现有斜杠菜单不能直接作为管理数据源

当前 ChatPanel 的状态含义并不统一：

- Codex 技能只保留 enabled 为真的项目。
- Codex 插件会展开市场返回的插件，但没有统一按 installed 和 enabled 过滤。
- MCP 又来自单独的运行时状态。

相关逻辑：
[src/components/chat/ChatPanel.tsx](../src/components/chat/ChatPanel.tsx#L3973)。

因此不能直接把斜杠菜单的数据复制到管理页面。正确方向是先建立统一目录，再让管理页面和斜杠菜单分别基于同一目录执行不同过滤规则。

## 4. 信息架构与界面

### 4.1 入口命名

侧边栏入口建议命名为“扩展”，而不是“插件”。

原因是该页面同时管理技能、插件和 MCP。“插件”只能作为页面内的一个分类。建议入口顺序如下：

~~~text
新建会话
命令
搜索
智能体
扩展
设置
~~~

图标建议使用 Puzzle 或 Boxes 类图标，并沿用现有侧边栏尺寸、选中态和键盘焦点样式。

### 4.2 页面骨架

~~~text
扩展管理                       [Codex ▼] [当前项目：panes_zh] [刷新]
统一管理技能、插件与 MCP

[搜索技能、插件或 MCP……]

[技能 24] [插件 181] [MCP 6]

[全部] [已安装/已配置] [未安装] [已禁用] [异常]
[来源 ▼] [作用域 ▼]

已安装 / 已配置
┌────────────────────┐  ┌────────────────────┐
│ 图标  名称          │  │ 图标  名称          │
│      简短说明        │  │      简短说明        │
│ 来源  版本  作用域   │  │ 状态  认证  作用域   │
│             [管理]   │  │             [配置]   │
└────────────────────┘  └────────────────────┘

可用
……
~~~

大宽度使用两列列表，窄宽度退化为单列。列表应保持紧凑，不采用大型营销卡片、装饰性渐变或玻璃效果。

### 4.3 顶部代理选择

代理下拉框首期选项：

- Codex
- Claude Code
- OpenCode

默认选择规则：

1. 如果当前存在活动聊天线程，优先跟随该线程的引擎。
2. 否则使用该工作区上次选择的代理。
3. 再否则回退到用户默认引擎。

选择状态按工作区持久化。切换代理时必须立即进入新加载状态，不得继续展示上一个代理的目录。

### 4.4 工作区上下文

页面顶部必须显示当前工作区或仓库，因为项目级技能、插件配置和 MCP 配置可能依赖 cwd。

没有打开工作区时：

- 只展示可确定的全局项目。
- 项目作用域操作置灰。
- 明确提示“选择工作区后可查看项目级扩展”。

### 4.5 分类与状态

页面使用三个主分类：

- 技能
- 插件
- MCP

状态文案按类型区分，不能全部使用“安装”：

| 类型 | 状态 |
| --- | --- |
| 技能 | 可用、已启用、已禁用、无效 |
| 插件 | 可用、未安装、已安装、已启用、已禁用 |
| MCP | 可用、未配置、已配置、已连接、未连接、需要认证、异常 |

通用筛选包括：

- 全部
- 已安装/已配置
- 未安装
- 已禁用
- 异常
- 来源
- 作用域

### 4.6 详情面板

点击卡片后从右侧打开详情面板，展示：

- 名称、说明和版本。
- 当前代理。
- 来源、市场或本地路径。
- 全局、项目、插件、内置或托管作用域。
- 安装、配置、启用、认证和健康状态。
- 插件内包含的技能、MCP、命令或 Hooks。
- 是否需要新建会话或重启后生效。
- 当前代理实际支持的操作。

本机已安装或已配置的项目始终正常展示。是否命中官方目录只参与内部合并和去重，不在卡片上增加“非官方”“本地来源”或类似标签。来源、市场或路径可以在详情中作为普通事实信息展示。

不可执行的操作不能只隐藏。对于用户可能预期存在的操作，应置灰并说明原因，例如“OpenCode 当前没有可读取的官方插件目录，因此没有可用插件列表”。

## 5. 数据边界与能力矩阵

目录必须拆成两条独立数据通路：

1. 本机清单：读取当前设备实际安装、配置或启用的技能、插件和 MCP，全部进入“已安装 / 已配置”区域。
2. 官方可用目录：只读取对应代理官方提供并能够明确验证来源的目录，只进入“可用”区域。

合并规则固定如下：

- 本机清单中的项目无论是否命中官方目录，都正常展示。
- 本机项目未命中官方目录时，不增加特殊标签、警告或风险暗示。
- 官方目录中的项目如果已安装，与本机项目合并，不重复展示。
- 只有能够确认来自代理官方目录的未安装项目才能进入“可用”区域。
- 仅仅出现在用户配置的市场、npm 搜索结果、GitHub、社区清单或第三方 API 中，不构成“官方目录”。
- 无法确认目录权威来源时，宁可不展示为可用项目。
- 官方没有提供某一类型的目录时，该类型只展示本机清单，“可用”区域为空。

“全部”表示“本机已安装/已配置项目 + 当前代理官方目录中的可用项目”，不表示全网全部项目。

| 代理 | 本机清单 | 官方可用目录 | 页面行为 |
| --- | --- | --- | --- |
| Codex | skills/list、已安装插件、MCP 配置与运行状态 | 只接收 Codex 官方接口明确提供或标识的技能、插件、MCP 目录；没有官方目录的类型为空 | 本机项目全部展示；官方未安装项进入“可用” |
| Claude Code | 企业、用户、项目和插件技能目录；已安装插件；MCP 配置 | 只接收 Claude Code 官方目录明确返回的项目，不读取第三方 Marketplace 作为可用目录 | 本机项目全部展示；无法确认官方来源的 available 项不进入“可用” |
| OpenCode | debug skill、配置中的 npm/本地插件、MCP 配置与运行状态 | 只有 OpenCode 提供可验证官方目录时才接入；当前本机解析结果不等于官方可用目录 | 本机项目全部展示；没有官方目录时不显示可用项目 |

“官方目录”是数据来源定义，不是安全结论。目录中的项目可能由第三方作者开发；Panes 不检查其源代码、依赖、病毒、数据收集行为或质量，也不使用“推荐”“已审核”“安全”“可信”等文案。

官方能力参考：

- [Claude Code 插件](https://code.claude.com/docs/en/plugins)
- [Claude Code 技能](https://code.claude.com/docs/en/skills)
- [Claude Code MCP](https://code.claude.com/docs/en/mcp)
- [OpenCode 技能](https://opencode.ai/docs/skills/)
- [OpenCode 插件](https://opencode.ai/docs/plugins/)
- [OpenCode MCP](https://opencode.ai/docs/mcp-servers/)

## 6. 统一领域模型

建议新增独立于现有诊断类型的扩展模型。诊断信息可以作为输入，但不能直接充当产品模型。

~~~ts
type ExtensionProviderId = "codex" | "claude" | "opencode";
type ExtensionKind = "skill" | "plugin" | "mcp";
type ExtensionScope =
  | "builtin"
  | "user"
  | "project"
  | "plugin"
  | "managed";

type ExtensionAction =
  | "install"
  | "uninstall"
  | "enable"
  | "disable"
  | "configure"
  | "authenticate"
  | "logout"
  | "open_location"
  | "refresh";

interface ExtensionItem {
  id: string;
  providerId: ExtensionProviderId;
  kind: ExtensionKind;
  name: string;
  description?: string;
  version?: string;

  scope: ExtensionScope;
  source?: string;
  marketplace?: string;
  path?: string;
  parentPluginId?: string;
  category?: string;

  officiallyAvailable: boolean;
  catalogAuthority: "provider_official" | null;
  installed: boolean | null;
  configured: boolean | null;
  enabled: boolean | null;

  health:
    | "unknown"
    | "healthy"
    | "disconnected"
    | "auth_required"
    | "error";
  authState?: "unknown" | "authenticated" | "required" | "failed";

  availableActions: ExtensionAction[];
  requiresNewSession: boolean;
  readOnlyReason?: string;
  warning?: string;
}

interface ExtensionCatalog {
  providerId: ExtensionProviderId;
  cwd?: string;
  items: ExtensionItem[];
  sources: ExtensionSource[];
  capabilities: ExtensionProviderCapabilities;
  fetchedAt: string;
  warnings: string[];
}
~~~

installed、configured 和 enabled 使用 boolean 或 null，而不是合并成一个状态枚举，原因是三种类型的生命周期不同：

- 插件通常有安装状态。
- MCP 通常有配置状态，没有统一的安装状态。
- 内置技能可能可用且启用，但不存在安装操作。

officiallyAvailable 只控制项目能否进入“可用”区域。它不得用于隐藏本机项目，也不得转化成面向用户的安全标签。

页面包含规则：

~~~ts
const showInInstalled =
  item.installed === true || item.configured === true;

const showInAvailable =
  item.officiallyAvailable === true &&
  item.catalogAuthority === "provider_official" &&
  item.installed !== true &&
  item.configured !== true;
~~~

### 6.1 代理能力声明

适配器必须返回能力声明，前端按能力渲染操作，不依据代理名称写大量条件分支。

~~~ts
interface ExtensionProviderCapabilities {
  hasOfficialSkillCatalog: boolean;
  canToggleSkills: boolean;
  hasOfficialPluginCatalog: boolean;
  canInstallPlugins: boolean;
  canTogglePlugins: boolean;
  hasOfficialMcpCatalog: boolean;
  canManageMcp: boolean;
  canAuthenticateMcp: boolean;
}
~~~

## 7. 技术架构

~~~mermaid
flowchart LR
    A["Codex App Server / CLI / 配置"] --> D["Codex 适配器"]
    B["Claude CLI / 技能目录 / 配置"] --> E["Claude 适配器"]
    C["OpenCode HTTP / CLI / 配置"] --> F["OpenCode 适配器"]

    D --> G["ExtensionService"]
    E --> G
    F --> G

    G --> H["Tauri 白名单 IPC"]
    H --> I["extensionStore"]

    I --> J["扩展管理页"]
    I --> K["对话斜杠菜单"]
    I --> L["状态与诊断展示"]
~~~

### 7.1 Rust 后端

建议新增：

~~~text
src-tauri/src/extensions/
  mod.rs
  types.rs
  codex.rs
  claude.rs
  opencode.rs

src-tauri/src/commands/extensions.rs
~~~

适配器职责：

- 查询代理版本与能力。
- 读取 CLI、App Server、HTTP 运行时和配置文件。
- 分开读取本机清单与官方可用目录，并验证官方目录来源。
- 将代理专属结构转换为统一 ExtensionCatalog。
- 执行明确列举的 ExtensionAction。
- 过滤敏感字段。
- 返回部分失败警告，而不是因为一个来源失败就丢弃整页数据。

不允许 perform_extension_action 接收任意命令字符串。后端根据 providerId、kind、action 和 id 选择固定命令模板并校验参数。

### 7.2 Tauri IPC

建议新增三个主要命令：

~~~ts
get_extension_catalog({
  providerId,
  cwd,
  forceRefresh
})

get_extension_details({
  providerId,
  kind,
  extensionId,
  cwd
})

perform_extension_action({
  providerId,
  kind,
  extensionId,
  action,
  scope,
  payload
})
~~~

payload 必须是按 action 定义的结构化联合类型，不允许透传 shell 参数。

### 7.3 前端 Store

新增：

~~~text
src/stores/extensionStore.ts
~~~

缓存键至少包含：

~~~text
providerId + workspaceId/repositoryId + normalizedCwd
~~~

每个缓存条目保存：

- catalog
- phase：idle、loading、ready、error
- error
- fetchedAt
- stale
- 当前动作及其进度
- 请求序号或 AbortController

请求序号用于避免快速切换代理或工作区时，较早请求的结果覆盖较晚请求。

### 7.4 前端组件

建议新增：

~~~text
src/components/extensions/
  ExtensionManagerPage.tsx
  ExtensionToolbar.tsx
  ExtensionTabs.tsx
  ExtensionFilters.tsx
  ExtensionGrid.tsx
  ExtensionCard.tsx
  ExtensionDetails.tsx
  ExtensionActionDialog.tsx
~~~

页面组件只负责筛选、展示和触发动作，不直接调用代理专属 IPC。

### 7.5 斜杠菜单接入

ChatPanel 不再自行拼接 Codex 技能、插件和 MCP 诊断数据，而是读取 extensionStore 的有效项目。

有效性规则：

- 技能：enabled 不为 false，并且当前会话允许调用。
- 插件：installed 为 true、enabled 不为 false，并且其能力适用于当前会话。
- MCP：configured 为 true、enabled 不为 false；连接异常项目可以展示警告，但不能伪装成健康可用。
- 官方可用但未安装的项目只出现在管理页，不进入斜杠菜单。
- 本机已安装项目即使没有命中官方目录，只要当前会话有效，仍可进入斜杠菜单。

## 8. 各代理适配策略

### 8.1 Codex

读取：

- 复用现有 skills/list 读取本机技能。
- 复用现有 plugin/list 和 CLI installed 读取本机插件。
- CLI 或 App Server 返回的 available 数据只有在能够确认来自 Codex 官方目录时才进入“可用”。
- 合并 mcpServerStatus/list 的运行状态与 codex mcp list --json 的配置状态。
- 技能或 MCP 没有官方可用目录时，只展示本机项目。

写操作：

- 插件安装和卸载使用官方 CLI 子命令。
- MCP 添加、删除、登录和登出使用官方 CLI 子命令。
- 技能或插件启停只有在存在稳定官方接口或能够安全保留 TOML 注释时才开放。
- 管理页只为官方可用目录中的未安装项目提供安装入口；本机已有项目仍可按能力卸载或启停。

生效提示：

- 安装或启用插件后，将已有会话标记为“扩展状态可能已过期”。
- 明确提示用户新建会话，不能假设活动会话已经热加载。

### 8.2 Claude Code

读取：

- 插件优先使用 CLI 的结构化 installed 数据读取本机清单。
- available 数据只有在 CLI 元数据能够明确证明它来自 Claude Code 官方目录时才进入“可用”。
- 技能扫描官方作用域目录、项目嵌套目录以及已安装插件内的 skills 目录。
- 只解析技能名称、说明、Frontmatter 和路径，不把完整技能正文发送给前端。
- MCP 使用 CLI 和配置文件建立配置状态；如果 CLI 只有人类可读输出，优先读取稳定配置结构，避免依赖脆弱文本解析。
- 技能或 MCP 没有官方目录时，不构造可用列表。

写操作：

- 插件使用 CLI 完成安装、卸载、更新、启用和禁用。
- MCP 使用 CLI 完成添加、删除、认证和登出。
- 独立技能没有可靠启停接口时保持只读，并提供打开所在位置。
- 不提供从第三方 Marketplace 安装项目的入口。

### 8.3 OpenCode

读取：

- 技能使用 opencode debug skill，但只保留名称、说明、位置等摘要字段，丢弃可能很大的 content。
- 插件读取解析后的配置，识别 npm 包、本地文件和目录。
- MCP 复用现有运行时 /mcp 数据并与配置状态合并。
- 这些本机解析结果只进入“已安装 / 已配置”，不自动成为官方可用目录。

写操作：

- 可以管理或移除本机已经配置的插件，但不提供输入任意 npm 规格来新增插件的入口。
- 修改 JSONC 时必须保留注释和无关字段。
- MCP 启停优先维护官方 enabled 配置。

能力限制：

- 没有可验证的 OpenCode 官方目录时，插件页只展示当前配置和已解析插件。
- “可用”区域为空，不抓取或聚合第三方目录。

## 9. 安全与配置完整性

### 9.1 目录展示与责任边界

- Panes 的“可用”区域只展示代理官方目录；“已安装 / 已配置”区域仍展示本机实际项目。Panes 不对目录内容进行主观筛选、排序或精选。
- Panes 不审核项目代码、依赖、病毒、隐私行为、维护质量或许可证风险。
- “官方目录”只描述目录数据来源，不表示目录项目由代理厂商开发，也不构成安全认证。
- 界面不得使用“精选”“已审核”“已认证”“安全”“可信”等具有背书含义的文案。
- 安装动作必须由用户主动触发；Panes 只执行对应代理提供的安装机制。

### 9.2 命令执行

- 所有命令在 Rust 后端使用参数数组调用，不经过 shell 字符串拼接。
- providerId、kind、action、scope 和 id 必须白名单校验。
- 路径必须规范化并限制在允许的用户或项目配置范围。
- 记录动作名称和结果，但不记录敏感 payload。

### 9.3 敏感信息

以下信息不得返回前端：

- MCP Token。
- Authorization Header。
- Cookie。
- 密钥类环境变量值。
- OAuth 临时凭据。

前端只接收 hasSecret、authState 和脱敏后的变量名。

### 9.4 配置文件

配置修改优先级：

1. 官方 CLI。
2. 代理提供的结构化 API。
3. 能保留注释和字段顺序的 TOML/JSONC 编辑器。
4. 不允许通过重新序列化整个文件覆盖用户注释和未知字段。

托管或企业策略控制的项目显示为只读，并返回具体 readOnlyReason。

### 9.5 危险操作

- 卸载插件需要确认，并列出将失去的技能和 MCP。
- 删除 MCP 配置需要确认。
- 安装官方目录中的项目时展示名称、来源、版本和作用域，但不附加任何质量或安全判断。
- 本机已有项目可以按代理能力管理或移除；页面不为官方目录之外的未安装项目提供安装入口。
- 认证动作必须在后端或系统浏览器完成，前端不能接触凭据。

## 10. 刷新、缓存与错误处理

### 10.1 刷新策略

- 首次进入页面按当前代理和 cwd 加载。
- 切换代理或工作区立即加载对应缓存；过期缓存后台刷新。
- 执行管理动作后强制刷新受影响代理的目录。
- 返回页面时，如果缓存超过约定 TTL，则后台刷新。
- TTL 应作为实现常量集中管理，不散落在组件中。

### 10.2 部分失败

目录允许部分成功，例如：

- 技能读取成功，但官方插件目录不可达。
- 插件读取成功，但某个 MCP 状态查询超时。

页面保留成功数据，并在对应分类显示局部警告。只有整个代理不可用时才展示整页错误。

### 10.3 版本兼容

每个适配器应先检测 CLI 版本和命令能力，不能只按固定版本号判断。

结构化输出解析需要保存脱敏 Fixture，覆盖：

- 字段新增。
- 可选字段缺失。
- 官方目录为空。
- 未登录。
- 托管策略禁用操作。
- 单个来源失败。

## 11. 可访问性与国际化

根据 PRODUCT.md，新增界面必须：

- 支持完整键盘导航和可见焦点。
- 图标按钮提供文本标签或 aria-label。
- 状态不能只依赖颜色。
- 窄窗口和放大文字下仍可使用。
- 支持减少动画。
- 满足 WCAG 2.2 AA 对比度目标。

用户界面文案需要同步维护：

~~~text
src/i18n/resources/en/
src/i18n/resources/pt-BR/
src/i18n/resources/zh-CN/
~~~

建议新增 extensions.json 命名空间，避免把大量扩展管理文案塞入 common.json。

## 12. 预计修改范围

现有文件：

~~~text
src/stores/uiStore.ts
src/components/sidebar/Sidebar.tsx
src/components/layout/ThreeColumnLayout.tsx
src/components/chat/ChatPanel.tsx
src/types.ts
src/lib/ipc.ts
src/globals.css
src-tauri/src/lib.rs
src-tauri/src/commands/mod.rs
src-tauri/src/engines/mod.rs
src-tauri/src/engines/codex.rs
src-tauri/src/engines/opencode.rs
src/i18n/resources/en/
src/i18n/resources/pt-BR/
src/i18n/resources/zh-CN/
~~~

新增文件：

~~~text
src/components/extensions/*
src/stores/extensionStore.ts
src-tauri/src/extensions/*
src-tauri/src/commands/extensions.rs
src/i18n/resources/*/extensions.json
~~~

具体实现时应优先抽取新的扩展服务，不继续扩大 ChatPanel 和现有诊断对象的职责。

## 13. 分阶段实施

### 阶段一：统一只读目录

范围：

- 新增侧边栏“扩展”入口和 extensions 页面。
- 完成代理、工作区、分类、搜索和状态筛选。
- 建立 ExtensionCatalog、代理能力声明和 extensionStore。
- 接入 Codex、Claude Code、OpenCode 的只读适配器。
- 将 ChatPanel 的扩展候选改为读取统一 Store。
- 对未支持操作显示准确原因。

阶段一完成后即可解决“没有统一查看位置”和“管理页、斜杠菜单状态不一致”两个核心问题。

### 阶段二：管理操作

范围：

- Codex、Claude 插件安装与卸载。
- 有可靠接口的插件启用与禁用。
- MCP 添加、删除、启停、认证和登出。
- 只有能够保留原配置内容和注释时才开放技能启停。
- 动作确认、进度、错误恢复和新会话提示。

### 阶段三：官方目录兼容与更多代理

范围：

- 处理代理官方目录协议、字段和版本变化。
- 不建设 Panes 自有目录，不聚合第三方市场、社区清单或包管理器搜索结果。
- 对 Gemini、Kiro、Kilo 等 Harness 分别进行能力核验后接入。
- 只有代理存在可验证官方目录时才展示对应“可用”列表。
- 不具备可靠读取或管理接口的代理保持“尚不支持”，不通过猜测或网页抓取实现。

## 14. 测试方案

### 14.1 Rust 单元测试

- 各代理结构化输出解析 Fixture。
- 路径、作用域和来源归一化。
- 官方目录来源验证与第三方 available 项过滤。
- 本机已安装项目未命中官方目录时仍正常保留。
- 命令动作白名单。
- 命令注入和非法 ID 拒绝。
- 敏感字段脱敏。
- 部分来源失败时仍返回有效目录。

### 14.2 前端单元测试

- extensionStore 缓存键。
- 快速切换代理时的竞态保护。
- 技能、插件、MCP 状态筛选。
- 管理页与斜杠菜单的不同有效性过滤。
- “可用”区域只包含官方目录项目。
- 本机已安装项目不因未命中官方目录而被隐藏或增加特殊标签。
- 空状态、部分错误和完全错误。
- 管理动作完成后的刷新和 stale 状态。

### 14.3 组件与交互测试

- 代理下拉框键盘操作。
- Tab、筛选和搜索。
- 卡片与详情面板焦点管理。
- 窄宽度布局。
- 危险操作确认。
- 不支持操作的原因展示。

### 14.4 集成测试

使用伪造 CLI 可执行文件和固定输出测试：

- 不依赖开发机真实配置。
- 不安装或删除用户真实插件。
- 不连接用户真实 MCP。
- 可稳定覆盖成功、超时、认证失败和版本变化。

## 15. 验收标准

1. 侧边栏“智能体”下方出现“扩展”，并能进入独立页面。
2. 页面可以切换 Codex、Claude Code 和 OpenCode。
3. 切换代理后不会残留上一个代理的数据。
4. 切换工作区后项目级扩展随之变化，全局扩展仍保留。
5. 技能、插件和 MCP 使用各自准确的状态文案。
6. 已安装、未安装、禁用和异常筛选结果准确。
7. “可用”区域只展示能够验证来自对应代理官方目录的项目。
8. 不存在官方目录时，“可用”区域为空，不聚合第三方或自建目录。
9. 本机已安装或已配置项目无论是否命中官方目录都正常展示，不增加特殊标签或风险提示。
10. 页面不包含排序、精选、审核、认证或安全背书功能和文案。
11. 对话斜杠菜单只展示当前会话真正有效的扩展。
12. 管理动作完成后自动刷新，并提示是否需要新建会话或重启。
13. 托管项和不支持的操作置灰且说明原因。
14. MCP 密钥、Header、Token 和敏感环境变量不出现在 IPC、日志和界面。
15. English、Português do Brasil 和简体中文资源保持一致。

## 16. 已确认的产品决策

- 侧边栏入口使用“扩展”。
- 页面内部使用“技能 / 插件 / MCP”三个分类。
- 首期代理为 Codex、Claude Code 和 OpenCode。
- 默认代理跟随活动线程，其次使用工作区上次选择。
- 页面必须展示当前工作区上下文。
- “全部”表示本机已安装/已配置项目加官方目录中的可用项目。
- “可用”只接受对应代理官方目录，不接受第三方市场、社区清单或 Panes 自建目录。
- 本机已安装或已配置项目始终正常展示，不因未命中官方目录增加标签。
- Panes 不进行项目排序、精选、审核、认证或安全背书。
- “官方目录”只代表数据来源，不代表目录项目安全或由代理厂商开发。
- 管理页与斜杠菜单共用数据源，但使用不同过滤规则。
- 第一阶段先完成只读目录，再开放写操作。
- 代理能力不强求完全对齐，由能力声明驱动界面。

## 17. 本机验证基线

2026-07-17 在当前开发环境完成以下只读核验：

- Codex CLI 0.144.5。
- Claude Code 2.1.210。
- OpenCode 1.15.3。
- Codex 插件 CLI 能返回 installed 与 available 两组结构化数据。
- Claude 插件 CLI 能返回 installed 与 available 两组结构化数据。
- OpenCode debug skill 能返回技能名称、说明、位置和正文；方案只使用摘要字段。

这些版本只用于证明当前方案的数据通路可行，不应被直接写成最低支持版本。实际实现必须进行能力检测和兼容解析。

## 18. 实现记录

2026-07-17 已在 `codex/extension-management` 分支完成首期实现：

- 新增统一 `ExtensionCatalog` 前后端模型、能力声明、Tauri 白名单 IPC 和 `extensionStore`。
- 新增 Codex、Claude Code、OpenCode 适配器；本机已存在项目全部保留，“可用”严格过滤为对应代理官方目录。
- Codex 与 Claude Code 插件目录支持安全的官方 CLI 安装、卸载和可用的启停操作；MCP 只开放代理已有的固定认证、登出或移除命令。
- OpenCode 技能、插件和 MCP 本机状态已接入；为保留 JSONC 注释与未知字段，插件配置保持只读。
- 侧边栏新增“扩展”，页面已实现代理切换、工作区上下文、技能/插件/MCP 分类、搜索、状态/来源/作用域筛选、详情与危险操作确认。
- Codex 直接读取官方插件自身 manifest 的分类字段，Claude Code 读取其官方 marketplace manifest；这样 Codex 不需要为分类额外启动 CLI 查询。“可用”列表按官方分类分节，缺失分类统一进入“未分类”。技能按作用域分类，MCP 按插件提供或独立配置分类。
- ChatPanel 的经典斜杠菜单已改为读取统一 Store，只展示已安装或已配置且未禁用的有效项目。
- English、Português do Brasil、简体中文资源已同步，并新增官方目录过滤、竞态保护、敏感错误过滤和命令参数校验测试。

实现继续遵守本方案已确认的责任边界：没有推荐、排名、审核、安全认证或“非官方/本地来源”标签。

## 19. 扩展目录持久化快照与后台刷新

### 19.1 决策

扩展管理页面不得在打开、切换分类、搜索或切换筛选条件时执行 Codex、Claude Code 或 OpenCode CLI。

页面只读取本地 SQLite 快照；CLI 只由应用生命周期内的后台刷新服务执行。应用不阻塞启动：后台服务启动后立即异步进行首次刷新，之后默认刷新间隔为 **6 小时**，不做分钟级 CLI 轮询。

### 19.2 现有基础

项目已使用本地 `workspaces.db`，并已启用 SQLite WAL 与连接池；Tauri `setup` 也已有通过 `tauri::async_runtime::spawn` 启动后台任务的模式。因此无需新增外部服务或网络数据库。

### 19.3 数据表

新增 `extension_catalog_snapshots`，以 `(provider_id, context_key, kind)` 为主键：

| 字段 | 用途 |
| --- | --- |
| `provider_id` | `codex`、`claude` 或 `opencode` |
| `context_key` | 规范化后的工作区/仓库上下文；不同项目级配置不混用 |
| `kind` | `skill`、`plugin`、`mcp`，三类独立持久化 |
| `items_json` | 已脱敏的统一 `ExtensionItemDto` 列表 |
| `fetched_at` | 最近一次成功获取时间 |
| `last_attempt_at` | 最近一次后台尝试时间 |
| `next_refresh_at` | 下一次允许自动刷新时间 |
| `last_error` | 最近一次失败摘要，不包含命令原始输出或凭据 |
| `failure_count` | 连续失败次数，用于退避 |

只能存统一、已脱敏后的 DTO；绝不保存 `codex mcp list --json` 等命令的原始 JSON，因为其中可能带 MCP 环境变量、Token 或 Header。

同一 `(provider_id, context_key, kind)` 独立更新：MCP 刷新失败时保留上次 MCP 快照，不能清空技能或插件，也不能把 MCP 数量改成 0。

### 19.4 后台刷新服务

1. 应用启动时立即启动后台调度器，不在启动链路同步等待扩展目录 CLI；后台任务立即异步执行首次检查。
2. 首次检查完成后，每 6 小时检查当前工作区和当前代理的快照。
3. 切换工作区或代理不会同步刷新；只切换到对应数据库快照。新上下文登记到数据库后会唤醒后台调度器，由后台任务异步刷新，页面不等待 CLI。
4. 同一 provider + context 同时最多一个任务。Codex 的技能、插件、MCP 刷新即使来自不同工作区，也必须全局串行执行；技能启动的 app-server 也属于这把锁。
5. 安装、卸载、认证、移除 MCP 等成功操作后，仅为受影响的类型入队一次刷新。
6. “刷新”按钮改为“请求后台刷新”：立即返回当前快照并显示刷新中，不等待 CLI，也不清空现有列表。

Codex 桌面端远程插件的 `.codex-remote-plugin-install.json` 是独立于 `codex plugin list` 的已安装事实。后台刷新只扫描带该标记的插件缓存目录，读取其受限大小的 plugin manifest；普通缓存目录不能据此被判为已安装。插件 manifest 声明的 `skills` 目录同时纳入“技能”栏，标记为已安装、插件作用域并关联父插件 ID。若该插件也命中 Codex 官方可用目录，则同一个项目同时保留“已安装”和“官方可用”属性。

首次使用没有快照时，页面显示“尚未获取扩展目录；后台正在异步刷新”，并保留手动刷新入口；不能因为打开页面而同步调用 CLI。

#### 前台刷新状态约定

无论刷新由应用启动首刷、6 小时定时刷新、失败退避后的重试、用户手动刷新或扩展操作后的刷新触发，只要任务已经排队或正在执行，扩展页都必须显示“后台正在刷新”。

- 已有快照时继续展示旧数据，并显示刷新提示和旋转状态。
- 没有快照时显示首次获取提示和刷新状态。
- 程序启动后，前端解析出持久化的当前工作区即通知后台登记该工作区；后台调度器立即把这个真实工作区（而不是仅按数据库默认工作区猜测）的三个提供商目录放入队列。工作区切换时同样登记一次。
- 后台任务开始、每个扩展类型写入结果、任务完成时都发送目录更新事件；页面重新从 SQLite 读取状态。
- 即使页面错过事件，目录读取也根据 `next_refresh_at <= 当前时间` 与进程内任务状态返回 `refreshing=true`，不能依赖事件时序。
- 若任务在页面挂载前完成，当前进程仍返回该上下文的“本次后台刷新已结束”状态；页面必须显示该提示，不能因错过开始/完成事件而静默。
- 页面按当前标签的成功快照时间显示相对时长：不足 1 分钟按秒，不足 1 小时按分，不足 1 天按小时，之后按天；失败尝试不得覆盖该时间。
- 成功写入下一次 6 小时刷新时间；失败写入对应退避时间和错误状态，前台不再把失败任务伪装成正在刷新。

### 19.5 页面与 IPC 契约

- 将当前 `get_extension_catalog` 拆为：`get_cached_extension_catalog`（只读 SQLite）和内部后台 `refresh_extension_catalog`。
- 前端不再以 60 秒 TTL 触发目录 CLI；Store 只读取数据库结果。
- 后台任务写入成功后发出 `extension-catalog-updated` 事件，前端收到事件后重新读取数据库快照。
- 页面显示“上次更新于 ……”和“后台刷新中”；失败时只显示简短状态，继续展示最后一次成功数据。

### 19.6 失败与退避

- 单次超时：保留最后成功快照，记录错误摘要。
- 每次程序启动都会排队一次异步首刷，即使上次有失败记录；这次首刷不重置 `failure_count`。同一运行期内后续失败仍按下列退避执行。
- 连续失败：依次在 1 分钟、30 分钟、1 小时后各重试一次；仍失败则回到正常的 6 小时周期。用户手动刷新可跳过等待，但仍复用同一任务去重锁。
- 没有任何成功快照时才显示空态；空态必须明确是“尚未首次获取完成”还是“该代理没有官方目录”，不能显示为零数据。

### 19.7 验收标准

1. 打开扩展页不会启动任何 CLI 子进程。
2. 后台 MCP 刷新失败后，页面仍显示上次成功的 MCP 列表与数量。
3. 插件、技能、MCP 的一次失败不会互相清空。
4. 同一上下文 6 小时内不会自动重复刷新；手动刷新和扩展操作是仅有的例外。
5. 数据库与前端日志中不出现 MCP 密钥、Header、Token 或完整环境变量。
