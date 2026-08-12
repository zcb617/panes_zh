# Panes 电脑操作 SDK 阶段 1：统一服务和 Codex 工具接入

> 本文件是统一阶段 1 的历史执行记录；早期文件名中的“阶段 2”不再作为阶段编号依据。

## 1. 阶段目标

在不启动外部电脑操作 MCP、不启动第二份 Panes、也不在进入设置页时初始化 CUA runtime 的前提下，把 Panes 的电脑操作能力接入 Codex App Server：

1. Panes 为新建 Codex 线程注册自己的动态工具目录。
2. Codex 发起实际 `item/tool/call` 时，由 Panes 统一服务先检查开关、工具和目标范围。
3. 首次访问目标或执行动作时，Panes 弹出授权请求；用户明确允许后才调用 CUA Driver SDK。
4. CUA SDK 的结果按 Codex 动态工具响应格式回传；失败、拒绝和超时均不执行实际电脑操作。

## 2. 范围

### 2.1 本阶段包含

- `ComputerControlService`：统一入口、工具目录、目标范围判断和错误映射。
- 按线程、回合、目标和操作类别隔离的临时授权状态。
- 复用现有 Panes 授权确认事件和确认窗口。
- Codex 新线程 `thread/start.dynamicTools` 注册。
- Codex `item/tool/call` 请求接收、授权、CUA SDK 调用和 `DynamicToolCallResponse` 回传。
- CUA SDK 通用工具调用入口；未授权前不初始化 SDK。
- 阶段测试、设计记录和风险记录。

### 2.2 本阶段不包含

- 设置页“添加 exe”产品路径的替换；该项属于阶段 3。
- Claude 和 OpenCode 的工具适配；该项属于阶段 2。
- 本阶段当时未处理旧 MCP broker、旧配置字段和旧命令；它们已在后续重构中彻底删除。
- 把旧白名单转换为永久授权。

## 3. 业务流程

```text
Codex 新建线程
    -> Panes 注册 panes_computer_control 动态工具目录
    -> 模型决定实际调用工具
    -> Codex 发送 item/tool/call
    -> Panes 校验开关、工具、目标范围和线程/回合
    -> 未命中临时授权：发出电脑操作授权事件并等待用户
    -> 用户拒绝/超时/任务取消：返回失败，不调用 SDK
    -> 用户允许：调用 Panes 内唯一 CUA SDK runtime
    -> 将 SDK 结果转换为 contentItems，回传 Codex
```

### 3.1 授权粒度

授权键由以下信息组成：

- 引擎：固定为 `codex`。
- `threadId` 和 `turnId`：只在当前任务回合内有效。
- 目标摘要：应用、窗口或受限资源。
- 操作类别：观察、输入、启动或剪贴板。

因此，允许一次点击不会自动允许另一个应用，也不会跨回合复用；本阶段不持久化授权。

### 3.2 目标边界

- 明确拒绝 `scope=desktop`、`capture_scope=desktop` 等全桌面范围。
- 输入类工具必须能从参数识别应用、窗口、PID 或其他受限目标。
- 仅有屏幕尺寸、运行状态等元数据的查询可使用电脑操作运行状态资源。
- Panes 自身进程不允许作为目标。

## 4. 工具目录

动态工具使用 `panes_computer_control` 命名空间。工具名称、说明和输入 schema 全部直接来自 CUA SDK 的规范工具目录；Panes 不维护第二份硬编码工具名单，也不对 SDK 工具目录取交集。

工具类别：

- 观察：`list_apps`、`list_windows`、`get_window_state`、`get_accessibility_tree`、`verify_state`、`get_screen_size`、`get_cursor_position`、`health_report`、`get_session_state`。
- 输入：`click`、`double_click`、`right_click`、`drag`、`type_text`、`press_key`、`hotkey`、`set_value`、`invoke_menu`、`scroll`、`move_cursor`、`zoom`。
- 应用和会话：`start_session`、`end_session`、`launch_app`。
- 剪贴板：`clipboard_read`、`clipboard_write`。

官方 CUA v0.19.3 的 `get_accessibility_tree` 结果本身包含目标窗口截图；是否可以继续作为图片 content item 回传，必须以锁定版本的 CUA 返回值和 Codex 图片合同测试为准。本阶段先保留结果转换入口，不把 base64 文本冒充图片能力。

## 5. 接口和状态

### 5.1 Panes 内部接口

- `ComputerControlService::dynamic_tools_spec()`：生成 Codex `dynamicTools`。
- `ComputerControlService::invoke_for_codex(...)`：执行一次统一电脑操作调用。
- `ComputerControlService::respond(...)`：接收授权窗口的允许或拒绝。
- `CuaDriverSdk::invoke(...)`：通过官方 C ABI 调用指定工具；首次调用前才惰性初始化。

### 5.2 Codex 协议接口

- 新线程：`thread/start` 参数包含 `dynamicTools`。
- 工具请求：`item/tool/call`，读取 `threadId`、`turnId`、`callId`、`tool` 和 `arguments`。
- 工具回传：`contentItems` + `success`。

### 5.3 前端兼容

本阶段复用已有 `computer-control-approval-requested` 事件和确认窗口，新增字段保持向后兼容；设置页和授权文案的产品化调整留在阶段 3。

## 6. 安全和生命周期

- 开关关闭时返回 `computer_control_disabled`，不初始化 SDK。
- CUA SDK 工具目录中不存在的工具返回 `tool_not_available`。
- 用户拒绝返回 `permission_denied`。
- 目标越界返回 `target_scope_mismatch`。
- 授权等待最多 5 分钟，超时后清理 pending 状态。
- 任务取消会取消授权等待并清理 pending 状态。
- 临时授权只在当前线程/回合内存在；不写入配置文件。
- 日志只记录请求标识、线程、工具、目标摘要和状态，不记录完整输入文本或原始截图。

## 7. 验证清单

| 编号 | 目的 | 结果 |
| --- | --- | --- |
| S2-01 | 新线程动态工具目录包含 Panes 命名空间 | 已实现；Rust 测试构建通过，运行时单测受当前 Windows 测试宿主阻断 |
| S2-02 | 设置开关关闭时不初始化 SDK | 已实现代码路径；待设置页迁移后实机验证 |
| S2-03 | 首次工具调用发出 Panes 授权事件 | 已实现代码路径；待 Codex 实机调用验证 |
| S2-04 | 拒绝授权不调用 SDK | 已实现状态机；待授权窗口实机验证 |
| S2-05 | 允许授权后调用 SDK 并回传成功结果 | 已实现回传链路；待低风险应用实机验证 |
| S2-06 | 全桌面范围和 Panes 自身目标被阻断 | 已实现边界校验；静态检查通过 |
| S2-07 | 超时、取消、回合切换不复用旧授权 | 已实现回合清理、取消和退出清理；待实机验证 |
| S2-08 | Codex 进程链不新增 MCP/Panes 进程 | 本阶段未启动新 MCP；官方 SDK Spike 的进程计数验证已通过 |

## 8. 阶段结果

状态：代码实现完成，实机闭环待后续阶段验收。

### 8.1 实际完成内容

- 新增 `ComputerControlService`，统一处理工具目录、开关、目标范围、临时授权和 CUA 调用。
- 新增 CUA SDK 通用 `invoke` 入口，实际工具调用前才惰性初始化 runtime。
- Codex 新线程注册 `dynamicTools`，接收 `item/tool/call` 并回传 `contentItems` / `success`。
- 授权确认复用既有 Panes 事件和窗口；拒绝、超时、取消、回合结束、引擎连接重置和应用退出都会清理对应授权。
- CUA 返回的文本和图片 content 已转换到 Codex 动态工具结果格式；官方 v0.19.3 的截图结果来自 `get_accessibility_tree` 的图片 content。

### 8.2 验证结果

- Rust 静态检查（包含测试构建）：通过。
- 官方 CUA Windows Spike：已通过，ABI `1.1.0`、driver `0.19.3`、屏幕尺寸 `1493 x 933`、关闭流程返回码 `0`。
- 定向单元测试运行：当前 Windows 测试宿主启动阶段报告系统入口点缺失，未将该项记为通过；测试代码已经完成构建检查。
- Panes 设置页、Codex 实际调用、授权弹窗、Notepad 点击/输入/截图闭环：未执行，等待设置页迁移阶段提供可用开关和实机验收。

### 8.3 未完成项和风险

- 当前设置页仍是旧的 exe/外部 driver 逻辑，不能作为本阶段实机验收入口。
- Claude/OpenCode 还没有接入统一服务。
- 历史 Codex 线程不会隐式重建动态工具；需要新建线程才能获得工具目录。
- `get_accessibility_tree` 的图片回传仍需以实际 Codex 版本做一次视觉闭环确认。

### 8.4 下一阶段入口

进入阶段 2：Claude/OpenCode 适配；之后进入阶段 3，迁移 Panes 设置页开关、移除旧 exe 授权心智；最终在阶段 4 完成 Windows 实机业务验收。

### 8.5 未完成清单回看

本阶段收尾已回看《[电脑操作 SDK 未完成清单](<电脑操作SDK未完成清单.md>)》中的 DONE-000、DONE-010 和 TODO-001 至 TODO-022。

- 当前阶段没有需要立即补做的事项：剩余事项分别依赖 Claude/OpenCode 适配、设置页迁移、Windows GUI 实机环境或跨平台环境，当前阶段无法形成有效验收证据。
- TODO-006 的 Windows 测试宿主仍处于“阻断”，应在阶段 2 开始前再尝试一次；若环境仍然阻断，转入阶段 4 CI 回归并保留失败证据。
- TODO-007 至 TODO-010 已登记为后续 Codex 实机回归，不因代码路径已完成而提前标记为通过。

本次回看结果已同步写入主清单的“阶段收尾检查记录”。下一阶段开始和结束时必须再次回看主清单；若届时某项已经具备当前阶段的验证条件，必须在当前阶段补做并更新状态。
