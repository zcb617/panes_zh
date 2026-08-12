# Panes 电脑操作 SDK 集成设计方案

## 1. 文档信息

- 功能名称：Panes 电脑操作能力
- 方案状态：待评审，尚未进入实现
- 目标分支：`codex/computer-control-sdk`
- 工作树：`D:\\work\\panes_zh\\.worktree\\computer-control-sdk`
- 第一版运行方式：Panes 主进程内直接集成 CUA Driver SDK；模型通过各引擎的原生工具适配器调用
- 第一版明确不采用：`cua-driver mcp` 外部进程、`--panes-computer-control-mcp`、第二个 `Panes.exe`、用户全局 CLI 配置改写、提前添加 exe 白名单

本方案是“业务边界、系统架构、授权模型、生命周期、接口约束和验收标准”的合并文档。实现前以本方案为准；如果技术预研发现 CUA Rust SDK 在 Panes 的 Windows 构建链上不可用，必须先回到方案评审，不得悄悄退回外部 MCP 进程模式。

## 2. 先说清楚业务：谁在做什么

Panes 是 GUI 和引擎编排层。Codex、Claude Code、OpenCode 是 Panes 已经接入的智能体引擎。电脑操作能力属于 Panes 的一项产品能力，不属于左上角的 MCP 管理功能。

电脑操作的真实调用链只有这一条：

```text
用户在 Panes 设置中打开能力
        ↓
模型在当前引擎中决定调用“电脑操作工具”
        ↓
对应引擎的工具适配器把调用转给 Panes 主进程
        ↓
Panes 主进程检查授权、目标窗口和安全边界
        ↓
Panes 主进程内的 CUA Driver SDK 调用 Windows UIA / SendInput / 屏幕捕获
        ↓
操作结果和截图回到对应引擎，再回到模型
```

这里没有第二个业务宿主。Panes 启动的 Codex app-server、Claude sidecar、OpenCode server 仍然是各自引擎本来就需要的进程；电脑操作只在这些进程与 Panes 之间增加工具调用，不会再启动一个 Panes。

CUA Driver SDK 负责“把指令变成鼠标、键盘、窗口、UIA 和截图动作”。它不负责理解用户任务，也不负责决定下一步点击哪里。下一步做什么仍由模型决定，Panes 负责把模型的单次工具调用限制在授权范围内。

## 3. 现状问题与改造目标

### 3.1 现状问题

当前实现把 CUA Driver 作为外部 MCP 子进程，并通过 `--panes-computer-control-mcp` 让另一个 Panes 进程充当代理。这会带来以下问题：

- 启动第二个 `Panes.exe`，产生多个带 `--panes-computer-control-mcp` 参数的进程。
- 托盘或窗口关闭后，代理链不能可靠收敛，容易留下孤儿进程。
- 每次进入设置页都做外部 driver 检测，driver 不在 PATH 或代理断开时会一直显示“检测中”。
- 黑框闪现来自外部命令行进程启动，不是 Panes 正常 GUI 行为。
- 通过全局 Codex、OpenCode、Claude 配置接入，电脑操作能力和用户已有 CLI 配置耦合。
- 需要提前添加 exe，无法满足“模型实际要操作时再申请授权”的业务要求。
- MCP 工具调用跨越多个进程，授权、工具调用、进程关闭分别由不同对象负责，难以保证一致的生命周期。

### 3.2 改造目标

- CUA Driver 只在 Panes 主进程内创建一个 SDK runtime，并在电脑操作功能开启的 Panes 启动阶段完成初始化。
- 只有 SDK 已经就绪后，模型实际发起电脑操作工具调用时，Panes 才弹出授权申请。
- 授权按当前任务、目标应用和操作范围临时生效，不要求用户预先提供 exe。
- 关闭能力、结束任务、引擎断开或 Panes 退出时，授权和 SDK runtime 可确定性释放。
- Codex、Claude、OpenCode 均通过各自原生接入点调用同一个 Panes 电脑操作服务。
- 设置页只展示 Panes 功能状态，不把它混入 MCP 列表，不展示外部 driver 路径检测。

## 4. 总体架构

### 4.1 分层

| 层 | 责任 | 不负责 |
| --- | --- | --- |
| Panes 设置层 | 开关、授权弹窗、临时授权查看和撤销、运行状态 | 直接操作鼠标键盘 |
| Panes 电脑操作服务 | 统一工具目录、参数校验、授权状态机、会话生命周期、错误归一化 | 理解自然语言任务 |
| CUA Driver SDK | 窗口发现、UIA 树、截图、鼠标、键盘、滚轮和窗口动作 | 用户授权 UI、任务规划、模型推理 |
| Codex 适配器 | 动态工具注册、`dynamicToolCall` 转发、图片结果映射 | 启动 CUA 或保存授权 |
| Claude 适配器 | Claude Agent SDK 自定义工具转发、文本和图片结果映射 | 修改 Claude 全局 MCP 配置 |
| OpenCode 适配器 | Panes 专属运行目录中的自定义工具、请求认证、结果转发 | 修改 OpenCode 全局配置 |

### 4.2 Panes 主进程内的核心对象

新增一个长期持有、在功能开启时初始化的 `ComputerControlService`，由 Rust `AppState` 持有。建议包含：

- `config`：仅保存功能开关和版本迁移信息。
- `driver_runtime`：启动时创建的 CUA Driver SDK 实例；整个 Panes 进程最多一个。
- `tool_catalog`：Panes 对模型暴露的稳定工具目录，不直接把 SDK 所有内部工具原样暴露给模型。
- `authorization_manager`：管理待授权请求、临时授权、撤销和超时。
- `engine_adapters`：Codex、Claude、OpenCode 的请求入口和关联上下文。
- `request_registry`：用 `request_id` 关联引擎调用、UI 授权、SDK 调用和最终结果。

服务不监听固定端口，不生成 `broker.json`，不启动 `cua-driver mcp` 或共享 daemon。若官方 SDK 在某个平台需要私有 worker（例如 Windows UIAccess worker），该 worker 必须由 SDK runtime owner 创建、认证和清理；它不是第二个 Panes，也不是用户可配置的常驻服务。OpenCode 适配器需要回调时，使用 Panes 为当前 OpenCode server 实例创建的随机端口和一次性 token；该通道只用于已有 OpenCode 进程回调 Panes，不是电脑操作服务的常驻 daemon。

### 4.3 单向依赖关系

```text
Engine tool adapter → ComputerControlService → AuthorizationManager
                                             → CuaDriver SDK
Settings UI         → Tauri commands        → ComputerControlService
```

引擎适配器不能直接调用 CUA Driver SDK，UI 也不能直接调用 CUA Driver SDK。这样授权和关闭逻辑只有一个入口，避免再次形成“多个代理各自认为自己拥有电脑控制权”的问题。

## 5. CUA Driver SDK 集成方式

### 5.1 官方产品边界

CUA 官方 README 和 runtime RFC 已经把应用 SDK 定义为直接集成边界：应用通过 `CuaDriver.create()` 在自身进程内创建 runtime，直接使用工具目录、会话和调用能力；daemon、HTTP、MCP 和私有 worker 只是可选适配层。Panes 采用这个 direct application SDK 路径，不启动 `cua-driver mcp`，也不连接共享 daemon。

### 5.2 Panes 使用官方预编译原生库

CUA 官方发布工作流会构建 `cua-driver-sdk`，并为各平台打包完成的原生库和 ABI 头文件：

| 平台 | 官方库文件 | ABI 头文件 |
| --- | --- | --- |
| Windows | `cua_driver_sdk.dll` | `cua_driver_abi.h` |
| macOS | `libcua_driver_sdk.dylib` | `cua_driver_abi.h` |
| Linux | `libcua_driver_sdk.so` | `cua_driver_abi.h` |

因此，Panes 不编译 CUA 源码。Panes 的构建和发布流程只需要固定一个官方 release 版本，取得对应平台的 release archive，把官方原生库、ABI 头文件和该 archive 声明的运行时依赖作为 Tauri resources 随 Panes 一起发布。Panes 侧实现一个很薄的 Rust FFI wrapper，通过稳定 C ABI 调用 `create`、工具目录、session、invoke、cancel 和 `shutdown`。

官方 Windows archive 还可能包含 UIAccess worker 等运行时组件；如果目标版本声明这些组件是必需依赖，就必须和 DLL 一起放入同一个受控 resources 目录，由 SDK runtime 负责启动和回收，不能让 Panes 自己把它当成 MCP 服务管理。

运行时加载只允许使用 Panes 安装包或 resources 目录的相对路径，并在启动时校验 ABI 版本、目标架构和依赖完整性。不存在查找 PATH、扫描电脑或写死开发机绝对路径的流程。

### 5.3 SDK runtime 约束

- Panes 进程最多创建一个 CUA runtime。
- `enabled = false` 时不加载原生库，也不启动 SDK 私有 worker。
- `enabled = true` 时，Panes 在启动阶段加载原生库、校验 ABI、创建 runtime 并检查可用性；状态页必须在“就绪”或“初始化失败”之间收敛。
- SDK 初始化、授权和工具执行是三个互不替代的状态：SDK 未就绪时不得弹出授权；授权通过后才允许执行工具。
- 初始化失败显示明确错误码；不循环探测、不弹授权，也不执行工具。
- `shutdown()` 必须幂等；关闭能力、引擎停止、Panes 退出都可以安全调用。
- SDK runtime 不通过共享 socket 或外部 CLI 代理；若官方 runtime 使用私有 worker，worker 的创建、认证和退出必须绑定 runtime 生命周期。

## 6. Panes 统一工具目录

模型看到的是稳定、受控的 Panes 工具，而不是 CUA Driver 内部全部工具。第一版建议工具目录如下：

| 工具 | 作用 | 默认风险级别 |
| --- | --- | --- |
| `list_windows` | 查看可操作窗口的标题、进程、应用路径和窗口标识 | 观察 |
| `get_window_state` | 获取指定窗口的截图、尺寸、前台状态和可用 UIA 信息 | 观察 |
| `focus_window` | 将已授权窗口切到前台 | 输入准备 |
| `click` | 在已授权窗口坐标点击 | 输入 |
| `double_click` | 在已授权窗口坐标双击 | 输入 |
| `type_text` | 在已授权窗口输入文本 | 输入 |
| `key_press` | 发送受限键盘按键或组合键 | 输入 |
| `scroll` | 在已授权窗口滚动 | 输入 |
| `wait` | 等待窗口状态变化或 UI 稳定 | 无副作用 |
| `launch_app` | 按用户在本次授权中确认的路径启动应用 | 高风险 |

第一版不暴露任意 shell、任意进程终止、任意文件读写、注册表修改、全屏无目标点击或“操作整个桌面”的工具。所有输入动作必须带有目标窗口标识；目标窗口失效时返回 `target_not_found`，不能自动退化为全局坐标点击。

工具参数中必须包含：

- `request_id`：用于追踪本次调用。
- `target_window`：由 `list_windows` / `get_window_state` 返回的受控标识，不接受模型自行伪造的裸 HWND。
- `operation_reason`：模型生成的简短意图说明，用于授权弹窗展示和审计。
- 动作参数：坐标、文本、按键或滚动量。

## 7. 授权设计

### 7.1 授权原则

授权在“模型真正准备调用电脑工具”时发起。用户不需要先添加 exe，不需要先配置 MCP，也不需要先判断某个 driver 是否在 PATH。

授权不是一次永久开关，而是一个临时、可撤销的资源授权：

- 绑定当前 Panes 任务/turn、引擎、目标应用和操作范围。
- 默认只在当前任务有效，任务结束、取消、超时、引擎断开或 Panes 退出即撤销。
- 同一任务内同一应用的同一风险范围不重复询问每一次点击。
- 从观察升级到输入、从已运行应用升级到启动应用、从一个应用切换到另一个应用，都要重新判断授权。
- 第一版不提供“永久允许全部应用”，避免把一次授权变成隐形的全局权限。

### 7.2 授权对象

授权请求包含：

- `request_id`、`engine`、`thread_id`、`turn_id`。
- 目标应用：规范化 exe 路径、进程标识、窗口标题和窗口标识；没有现成 exe 时显示 SDK 能识别到的实际目标信息。
- 操作范围：查看窗口、查看屏幕、鼠标输入、键盘输入、启动应用等。
- 模型意图：例如“在 ERP 的销售订单窗口中点击新增”。
- 即将执行的具体动作：点击坐标、输入文本、按键组合或启动路径。
- 过期时间和取消原因。

### 7.3 授权状态机

```text
disabled
   ↓ 设置开启 / Panes 启动
initializing ──失败──> failed
   ↓ 成功
ready
   ↓ 需要新权限
awaiting_user ──拒绝──> denied
   ↓ 允许
authorized
   ↓ CUA 调用
executing ──完成──> authorized / ready
   ↓ 任务结束、撤销、关闭或断开
revoked
```

状态机的关键约束：

- SDK 不是 `ready` 时，服务必须直接返回 `sdk_unavailable`，不得创建 pending 授权或显示授权弹窗。
- 用户拒绝后，只结束当前请求，不得自动重试弹窗。
- 用户关闭设置开关，所有 `awaiting_user` 请求立即取消，所有临时授权立即撤销。
- 任务取消和引擎断开必须唤醒正在等待授权或 SDK 调用的任务，返回明确的 `cancelled` / `engine_disconnected`。
- `runtime_already_exists`、`sdk_unavailable`、`unsupported_platform` 等错误要直接返回，不进入无限检测。

### 7.4 设置页和授权弹窗

设置页继续使用独立的“电脑操作能力”区块，不能放到左上角 MCP/扩展列表中。第一版页面调整为：

- “允许电脑操作”总开关。
- “CUA Driver SDK 状态”：已关闭、未初始化、就绪、初始化失败、当前平台不支持。
- “当前授权”：显示当前任务的应用、授权范围和剩余时间，提供撤销按钮。
- “引擎适配器”：Codex、Claude、OpenCode 显示内置适配状态；历史线程没有工具时显示“需要新建会话”。
- 安全说明：授权只在当前任务有效，Panes 退出后全部失效。

删除产品概念上的“允许操作的应用 / 添加 exe”。旧配置字段不再保留，也不参与新的授权判断。

授权弹窗至少包含：

- 哪个引擎、哪个任务发起请求。
- 将操作哪个应用和窗口。
- 将查看什么或执行什么动作。
- “允许本次任务”和“拒绝”两个明确按钮。

## 8. 三个引擎的接入方式

### 8.1 Codex：App Server 动态工具

Panes 已经通过 `codex app-server --listen stdio://` 与 Codex 通信。电脑操作接入 Codex 的方式是：

1. 在 `thread/start` 的 `dynamicTools` 中注册 `panes_computer_control` 命名空间及统一工具目录。
2. Codex 产生 `item/tool/call` 后，Panes 将 `threadId`、`turnId`、`callId` 和工具参数转换为统一服务请求。
3. 统一服务先确认 SDK 为 `ready`，再做授权；只有授权允许后才能调用 CUA SDK。
4. 以 `DynamicToolCallResponse` 回传文本；截图以 app-server schema 支持的 `input_image` content item 回传。

Codex 当前 schema 中 `dynamicTools` 属于 `thread/start`，`thread/resume` 和 `thread/fork` 没有同样的动态工具字段。因此不能假设恢复旧线程时可以临时补工具。设计要求：

- Panes 创建的新线程一律带上 Panes 电脑工具定义，即使当前开关关闭；关闭时工具调用只返回“能力未启用”，不会初始化 SDK。
- 这样开关可以即时生效，不需要为每次开关切换重启 Codex。
- 由旧版本 Panes 创建、没有该工具定义的历史线程不做隐式重建；UI 明确提示“新建会话后可使用电脑操作”。
- `deferLoading` 只作为工具目录优化选项，不能用来代替授权，也不能假设它能在恢复线程中动态添加工具。

Codex 的动态工具调用不能走现有 shell/MCP 审批语义。电脑授权由 Panes 自己的 `authorization_manager` 决定，Codex 的 app-server approval 仅保留给它自己的审批场景。

### 8.2 Claude：Claude Agent SDK 自定义工具

Panes 已有 Claude Agent SDK sidecar。改造方式是：

- 在 sidecar 内注册 Claude Agent SDK 的自定义工具，工具名称使用 `panes_computer_control_*`。
- 自定义工具通过现有 sidecar 与 Panes Rust 主进程的 JSON Lines 双向协议转发，携带 `request_id`、会话和任务上下文。
- Rust 主进程统一检查授权并调用 CUA SDK。
- 结果返回文本和图片 content block，由 Claude Agent SDK 继续交给模型。

这里的“自定义工具”是 Claude Agent SDK 的进程内工具适配，不是启动一个外部 `cua-driver mcp`。不得再通过 `PANES_COMPUTER_CONTROL_CONFIG` 指向外部 MCP server，也不得修改 Claude 用户全局配置。

### 8.3 OpenCode：Panes 专属运行目录中的自定义工具

Panes 已经负责启动并停止 OpenCode server。接入方式是：

1. 每个 OpenCode server 实例创建一个位于 Panes app data 下的临时运行目录。
2. 在该目录放置 Panes 自己的 OpenCode custom tool 文件。
3. 通过启动该 OpenCode 进程时的 `OPENCODE_CONFIG_DIR` 或等效进程级配置，让 OpenCode 只加载本次 Panes 实例的工具。
4. custom tool 调用 Panes 绑定的本地回调地址，并携带随机一次性 token。
5. Panes 校验 token、会话和请求来源，再进入统一电脑操作服务。
6. OpenCode server 停止时立即废弃 token，清理动作只清理本次临时运行目录；不能修改用户全局配置目录。

OpenCode 自定义工具的图片结果格式必须以实际锁定版本的 `Tool.Result` / custom tool contract 通过适配器测试为准。只有确认截图可以作为模型可读的图片附件回传，才宣称 OpenCode 具备完整视觉闭环；如果当前版本只接受字符串结果，第一阶段验收必须阻断，不得把 base64 文本冒充图片能力。该验证是 OpenCode 适配器的硬性门槛。

## 9. 进程、路径和生命周期

### 9.1 进程模型

目标进程关系如下：

```text
Panes.exe（GUI + Rust 服务 + CUA Driver SDK runtime）
 ├─ codex app-server（Panes 原有引擎进程）
 ├─ claude-agent-sdk-server（Panes 原有 sidecar）
 └─ opencode serve（Panes 原有引擎进程）
```

禁止出现：

```text
Panes.exe
 └─ Panes.exe --panes-computer-control-mcp
     └─ cua-driver mcp
```

官方 SDK 在 Windows 上如果需要 UIAccess worker，可以由 SDK 在 resources 目录内按 runtime 生命周期启动；这不改变 Panes 的宿主边界。实现完成后的进程验收必须确认没有 `--panes-computer-control-mcp`、没有第二个 `Panes.exe`，也没有因进入设置页反复拉起的 driver 检测进程；SDK 私有 worker 必须随 runtime 关闭。

### 9.2 路径规则

- CUA SDK 原生库和 ABI 头文件由官方 release archive 提供，路径只允许通过 `Panes.exe` 相对 resources 目录解析。
- SDK 声明的私有运行时组件也只能从同一 resources 目录加载，不能从 PATH 或开发机目录寻找。
- OpenCode 的工具文件只放在 Panes 为本次 server 创建的 app-data 运行目录。
- 任何配置文件路径必须由 Panes 运行时计算，不得写死开发机 `C:\\Users\\...`、`D:\\work\\...` 等绝对路径。
- 不修改 Codex、Claude、OpenCode 的用户全局配置文件。

### 9.3 关闭顺序

1. 用户关闭电脑操作开关：标记 disabled。
2. 取消等待中的授权和 SDK 请求。
3. 撤销所有临时 session authority。
4. 调用 CUA SDK `shutdown()` 并释放 runtime。
5. 关闭当前 OpenCode 回调 token 和临时目录。
6. 引擎进程继续按其原有生命周期运行；不因关闭电脑操作额外启动或杀掉 Panes。

Panes 退出时执行同一套清理，但即使异常退出也不会留下 `--panes-computer-control-mcp` 或第二个 Panes 进程；SDK 私有 worker 若存在，必须由 runtime owner 负责退出和回收。

## 10. 配置和迁移

### 10.1 新配置

```json
{
  "enabled": false,
  "schemaVersion": 2
}
```

第一版不持久化授权、窗口句柄、进程 ID、driver 路径或 exe 白名单。窗口句柄和进程 ID 只在当前 runtime 有效，不能跨 Panes 重启复用。

### 10.2 旧配置迁移

旧版本中的 `allowed_applications`、`broker`、外部 driver 路径和引擎 MCP 配置全部从当前实现删除。迁移时：

- 不再保留旧字段的读取和写入代码。
- 将旧白名单视为历史数据，不自动转成永久授权。
- 新版本不再写入这些字段。
- 旧的全局 MCP 配置不能由新功能继续追加；清理历史配置需要单独的迁移动作和用户可见说明，不能在启动时静默覆盖用户配置。

## 11. 错误模型和可观测性

统一服务对引擎返回稳定错误码：

| 错误码 | 含义 | 用户可见行为 |
| --- | --- | --- |
| `computer_control_disabled` | 设置开关关闭 | 提示到设置页开启 |
| `sdk_unavailable` | SDK 初始化失败或当前平台不支持 | 显示具体原因和重试 |
| `authorization_required` | 需要用户授权 | 弹出 Panes 授权窗口 |
| `permission_denied` | 用户拒绝 | 结束当前调用，不自动重试 |
| `target_not_found` | 目标窗口已关闭或标识失效 | 要求模型重新发现窗口 |
| `target_scope_mismatch` | 调用试图越过已授权目标 | 阻止并重新申请授权 |
| `runtime_already_exists` | 检测到重复 runtime | 记录错误，不能创建第二实例 |
| `request_timeout` | 工具或授权超时 | 取消请求并释放临时状态 |
| `engine_disconnected` | 引擎断开 | 撤销该引擎任务的授权 |

日志只记录 `request_id`、引擎、线程、工具、目标摘要、状态和耗时，不记录输入框密码、完整文本内容或截图原始数据。所有引擎适配器日志都使用同一个 `request_id`，便于定位“模型发起—授权—SDK 执行—结果回传”的完整链路。

## 12. 分阶段实施计划

各阶段的未完成事项统一登记在《[电脑操作 SDK 未完成清单](<电脑操作SDK未完成清单.md>)》。阶段开始和收尾必须回看该清单；具备当前阶段验证条件的事项必须当场补做，不得只在阶段文档中顺延。

### 阶段 0：官方预编译 SDK 集成 Spike 与 Panes 正式运行时接入，作为硬门槛

详细的业务目的、执行顺序、用例编号和结果记录格式见《电脑操作 SDK 集成测试清单》；Spike 和后续集成都必须按该清单逐条执行，不能只做 DLL 加载烟测。

- 固定一个官方 CUA release 版本，取得对应 Windows archive，不把 CUA 源码加入 Panes 的 Rust 构建配置。
- 核对 archive 是否包含 `cua_driver_sdk.dll`、`cua_driver_abi.h` 以及该版本声明的全部运行时依赖。
- 在当前工作树中只做 Panes 侧最小 FFI 集成：从 resources 相对路径加载原生库，校验 ABI，创建 runtime、列工具、创建 session、获取一次桌面状态、shutdown。
- 将官方资源纳入 Panes 正式 resources，并验证 Panes runtime 在功能开启后的启动初始化、状态读取和 shutdown；此前单列的 runtime 记录归入本阶段，不再作为独立阶段。
- 验证 Panes 进程内只有一个 runtime；如果该版本需要 SDK 私有 worker，验证 worker 随 runtime 创建并随 shutdown 退出。
- 验证 Windows UIA、屏幕捕获、输入依赖能被官方 archive 正确带起。
- Spike 未通过时停止，不进入 UI 和引擎适配器开发。

### 阶段 1：统一服务和 Codex（代码完成，实机闭环待后续）

当前阶段已完成：官方 Windows 资源已纳入 Tauri resources，Panes 主进程已有惰性 CUA runtime、统一电脑操作服务、按线程/回合/目标隔离的临时授权状态、Codex 新线程动态工具注册和 `item/tool/call` 回传。设置页产品迁移和 Windows 实机闭环仍未完成。

- 已新建 `ComputerControlService`、授权状态机和统一工具目录。
- 已加入 Codex 新线程 `dynamicTools` 注册及 `item/tool/call` 回传。
- 已实现授权事件、拒绝、撤销、超时、任务取消和 Panes 退出清理。
- 低风险应用点击、输入和截图实机闭环：未执行，待设置页迁移后验收。

### 阶段 2：Claude 和 OpenCode 适配器（代码完成，真实图片和 GUI 回归待后续）

- Claude sidecar 已改为 SDK 自定义工具转发，调用统一 `ComputerControlService`。
- OpenCode 已改为 Panes 专属运行目录、进程级配置和一次性本机回调。
- OpenCode 图片 tool-result 只完成结构化回传入口和静态契约核对，真实图片合同测试尚未执行，因此不能宣称视觉闭环完成。

### 阶段 3：设置页和旧实现迁移

- 设置页改成 Panes 电脑操作能力状态。
- 移除“添加 exe”和外部 driver 检测的产品路径，并彻底删除对应旧代码。
- 停止写入外部 MCP、broker 和 `--panes-computer-control-mcp` 相关配置。

### 阶段 4：回归和验收

- 进程生命周期、引擎断开、Panes 重启和重复开关回归。
- Codex 新线程、Codex 历史线程、Claude、OpenCode 分别验收。
- Windows 实机验证真实鼠标、键盘、窗口和截图。
- 仅在用户实际测试通过后，按 Panes 项目约定执行本地验收构建。

## 13. 验收标准

### 必须满足

- 设置页有独立的 Panes“电脑操作能力”开关，不出现在 MCP/扩展列表。
- 功能开启后的 Panes 启动阶段必须完成 SDK 初始化；只有状态为“可用”时才允许产生电脑操作授权。
- 模型第一次实际调用时才弹授权，不要求预先添加 exe。
- Panes 主进程内只有一个 CUA SDK runtime。
- Panes 使用官方预编译原生库和稳定 C ABI，不要求消费者编译 CUA 源码。
- 进程列表不存在 `--panes-computer-control-mcp`，不存在第二个 Panes。
- 若官方 runtime 使用私有 worker，worker 不能脱离 runtime 常驻。
- 进入设置页不会启动命令行检测窗口，不会出现黑框闪现，也不会长期显示“检测中”。
- 用户拒绝后调用被阻断；授权范围外的应用、窗口和操作被阻断。
- 关闭开关、结束任务、引擎断开和 Panes 退出会撤销临时授权并释放 SDK。
- Codex 新线程能收到动态工具调用和截图；旧线程会明确提示工具未加载，不静默失败。
- Claude 能通过现有 sidecar 调用同一统一服务，不创建外部 MCP。
- OpenCode 不修改全局配置；截图结果只有在实际图片契约测试通过后才算完成。
- 所有路径均为运行时解析的相对/资源路径，不写死开发机绝对路径。

### 明确不接受的结果

- 通过再启动一份 `Panes.exe` 解决工具调用。
- 通过 `cua-driver mcp` 作为常驻服务解决授权。
- 通过设置页扫描 PATH、反复启动 driver 来显示“可用”。
- 通过提前配置 exe 白名单代替运行时授权。
- 把 Codex、Claude、OpenCode 的全局配置都改成指向同一个外部 MCP。
- 只回传截图 base64 文本，却声称模型已经具备视觉电脑操作能力。

## 14. 参考资料

- CUA 项目：https://github.com/trycua/cua
- CUA Driver SDK README：https://github.com/trycua/cua/blob/main/libs/cua-driver/README.md
- CUA SDK runtime RFC：https://github.com/trycua/cua/blob/main/rfcs/2549-cua-driver-sdk-owned-runtime.md
- CUA Driver 跨平台发布工作流：https://github.com/trycua/cua/blob/main/.github/workflows/cd-rust-cua-driver.yml
- CUA C ABI 说明：https://github.com/trycua/cua/blob/main/libs/cua-driver/rust/include/README.md
- OpenCode Custom Tools：https://opencode.ai/docs/custom-tools
- OpenCode Configuration：https://opencode.ai/docs/config
- Codex App Server：https://developers.openai.com/codex/app-server/

## 15. 最终决策

Panes 的电脑操作能力采用“官方预编译 CUA Driver 原生库 + 稳定 C ABI + 三个引擎原生工具适配器 + Panes 统一运行时授权”的方案。Panes 消费官方 release archive，不编译 CUA 源码；CUA 项目自己的 CI 负责生成各平台原生库，Panes 只负责按版本集成和随应用发布。

CUA Driver SDK 是真正执行鼠标、键盘、窗口和截图的驱动层；模型是业务编排者；Panes 是 GUI、授权中心和引擎编排者。三者职责分开，但运行链路只有一个 Panes 宿主，不再通过 MCP 代理或第二份 Panes 拼接出一条不可控的进程链。
