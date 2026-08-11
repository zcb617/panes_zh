# Codex Steer 运行状态可见性优化方案

## 1. 背景与问题结论

Panes 在 Codex turn 执行期间支持通过 `turn/steer` 追加用户指令。现有链路没有丢失消息，Codex App Server 也能立即确认请求成功；问题在于，App Server 可能要等当前模型响应到达边界后，才把追加指令真正注入会话。

当前界面把以下三个事实混在了一起：

1. Panes 已经提交追加指令；
2. Codex App Server 已经接受追加指令；
3. Codex 模型已经开始处理追加指令。

现有协议链路只能可靠确认前两项，不能可靠确认第三项。同时，当前助手消息一旦已有文本、工具块或 steer 块，`ChatPanel` 就不再显示“正在思考”占位。因此，在 steer 等待模型边界或 Codex 只产生不可展示的内部 reasoning 时，界面会长时间没有活动反馈。

## 2. 优化目标

1. 用户提交 steer 后，立即看到明确的提交状态。
2. `turn/steer` 返回成功后，只表达“Codex 已接受”，不误报“已经开始处理”。
3. 只要当前 turn 仍在运行，助手消息末尾始终有可见的活动提示。
4. 有工具执行、等待授权等更具体状态时，优先显示真实状态。
5. RPC 失败时保留用户追加的原文并显示失败状态，不再静默移除。
6. 普通 `send`、流式文本、工具输出、授权和 turn 完成行为保持不变。
7. 不伪造 reasoning 内容，不修改 Codex App Server 的 steer 执行语义。

## 3. 非目标

1. 不取消当前 turn 后重新发送 steer。
2. 不把 steer 改造成新的普通 turn。
3. 不根据任意下一条引擎事件猜测“某条 steer 已开始处理”。
4. 不通过轮询 rollout、数据库或日志判断 steer 是否生效。
5. 不修改流监听、前端短窗口批处理或 Rust 增量合并逻辑。
6. 不展示或解密 Codex 的内部 reasoning。

## 4. 状态模型

### 4.1 Steer 交付状态

在 `SteerBlock` 中增加可选状态：

```ts
type SteerDeliveryStatus = "sending" | "accepted" | "applied" | "failed" | "settled";
```

| 状态 | 可核查事实 | 界面文案 |
| --- | --- | --- |
| `sending` | 前端已经发起 IPC，尚未收到结果 | 正在追加… |
| `accepted` | `turn/steer` RPC 已成功返回，但 Codex 尚未把该消息插入当前 turn | 已追加，等待进入当前处理步骤 |
| `applied` | Codex 发出对应的 `userMessage item/started`，该 steer 已进入当前 turn 的真实事件序列 | 已进入当前处理步骤 |
| `failed` | IPC、RPC 或后端处理失败 | 追加失败 |
| `settled` | 当前 turn 已完成或取消 | 不显示动态交付文案 |

`accepted` 不能命名为 `processing`。RPC 返回只证明 Codex 已接收请求；只有后续出现对应的 `userMessage item/started`，才能把 steer 标记为 `applied`。

### 4.2 Turn 活动状态

Turn 活动提示与“助手消息是否已有内容”解耦。

| 当前状态 | 活动提示 |
| --- | --- |
| streaming，助手还没有可见内容 | Codex 正在思考… |
| streaming，助手已经有可见内容 | Codex 仍在运行… |
| 存在运行中的工具动作 | 正在执行：动作摘要 |
| 存在待处理授权 | 等待你的确认 |
| 最近存在 `accepted` steer，且没有更具体状态 | Codex 仍在运行，追加消息正在等待进入当前处理步骤 |
| turn 完成、失败或取消 | 不显示活动提示 |

活动提示只描述整个 turn，不宣称正在处理某一条具体 steer。

## 5. 数据流设计

```text
用户提交 steer
    ↓
前端生成 clientSteerId，插入 sending steer 块；该块临时保持在流式内容尾部
    ↓
IPC steer_message(clientSteerId, ...)
    ↓
Rust 持久化用户消息并调用 turn/steer
    ├─ 失败：返回错误，前端标记 failed
    └─ 成功：返回 SteerReceipt，前端标记 accepted
    ↓
当前 turn 持续 streaming
    ↓
首个 userMessage item/started 是本轮原始提问，只登记、不落 steer 块
    ↓
后续 userMessage item/started 按 FIFO 匹配待处理 steer
    ↓
后端在该事件边界写入 SteerApplied，前端把对应块固定在此处并标记 applied
    ↓
后续助手文本、工具和授权事件继续追加在该 steer 块之后
    ↓
助手消息末尾持续显示 Turn 活动提示；尚未 applied 的 steer 仍保持在尾部
    ↓
TurnCompleted / cancel
    ↓
accepted/applied steer 标记为 settled，隐藏活动提示
```

## 6. 接口设计

### 6.1 前端类型

```ts
export type SteerDeliveryStatus = "sending" | "accepted" | "failed" | "settled";

export interface SteerBlock {
  type: "steer";
  steerId: string;
  content: string;
  deliveryStatus?: SteerDeliveryStatus;
  errorMessage?: string;
  // 现有字段保持不变
}

export interface SteerReceipt {
  clientSteerId: string;
  expectedTurnId: string;
  acceptedAt: string;
}
```

### 6.2 IPC

`ipc.steerMessage` 增加 `clientSteerId` 参数，并把返回值从 `void` 改为 `SteerReceipt`。

### 6.3 Rust

新增序列化 DTO：

```rust
pub struct SteerReceiptDto {
    pub client_steer_id: String,
    pub expected_turn_id: String,
    pub accepted_at: String,
}
```

Codex 引擎的 `steer_message` 在 `turn/steer` 成功后返回实际使用的 `expected_turn_id`。该返回值只作为接收凭证和诊断关联，不作为“模型已经处理”的证明。

## 7. 前端展示设计

### 7.1 Steer 块

保留现有绿色追加块结构，在内容下方增加一行低权重状态文字：

```text
↳ 用户追加内容
  已追加，等待进入当前处理步骤
```

出现对应的 `SteerApplied` 后，块固定在真实事件边界，不再跟随流式内容尾部移动。失败时使用错误色状态文字，并保留原始内容。第一阶段不增加“重试”按钮，避免扩大交互范围；用户可以重新发送。

### 7.2 助手消息末尾活动提示

保留现有空消息思考占位，并增加“已有内容但仍在运行”的尾部活动提示。两者使用同一套现有动画和视觉语言，但分别在当前 DOM 上下文内维护，避免抽取只有一个调用点的函数或组件。

尾部提示放在当前助手消息所有内容块之后，不能覆盖已有文本、steer、工具或授权卡片。

## 8. 文件级改动

| 文件 | 改动 |
| --- | --- |
| `src/types.ts` | 增加 steer 交付状态和 receipt 类型 |
| `src/lib/ipc.ts` | 传递 `clientSteerId`，接收结构化 receipt |
| `src/stores/chatStore.ts` | 实现 sending、accepted、applied、failed、settled 状态转换，并让未 applied 的块保持在流式尾部 |
| `src/components/chat/ChatPanel.tsx` | 将 turn 活动提示与 `hasAssistantContent` 解耦 |
| `src/components/chat/MessageBlocks.tsx` | 渲染 steer 交付状态 |
| `src-tauri/src/models.rs` | 增加 `SteerReceiptDto` |
| `src-tauri/src/commands/chat.rs` | 接收并持久化 `client_steer_id`，返回 receipt |
| `src-tauri/src/engines/mod.rs` | 增加 `SteerApplied` 事件，让引擎收到客户端 steer 标识 |
| `src-tauri/src/engines/codex.rs` | 返回 `expected_turn_id`；维护 turn 内 FIFO 待应用队列；在后续 `userMessage item/started` 处发出 `SteerApplied` |
| `src-tauri/src/models.rs` | 为用户 steer 文本保留 `clientSteerId`，为助手消息持久化 steer 块 |
| `src/stores/chatStore.test.ts` | 覆盖真实事件顺序、多个 steer、失败保留和重载去重 |
| Rust 现有模块测试 | 覆盖 receipt、expected turn ID、用户消息边界和 steer 块序列化 |

实际实现时，如果某个 Rust 类型已有更合适的归属位置，允许放入该既有模块，但不新建只有单一用途的抽象层。

## 9. 诊断日志

新增以下结构化日志，不记录用户消息正文：

- `steer_submit_started`
- `steer_rpc_accepted`
- `steer_rpc_failed`

字段包含：

- `client_steer_id`
- Panes thread ID
- engine thread ID
- expected turn ID
- RPC elapsed milliseconds

不将“下一条引擎事件”记录为 steer 已处理，只可作为普通链路诊断数据。

## 10. 实施顺序

1. 增加类型和测试夹具。
2. 打通 `clientSteerId` 与结构化 receipt。
3. 识别 Codex 的原始 `userMessage` 与后续 steer `userMessage`，建立 FIFO 匹配。
4. 实现 store 中的 steer 状态转换和未应用块的尾部跟随。
5. 实现 steer 块状态展示。
6. 实现助手消息末尾持续活动提示。
7. 补前端单元测试和 Rust 单元测试。
8. 执行格式检查、定向测试和构建检查。
9. 启动实际界面，验证普通 send 与 steer 场景。

## 11. 验收场景

1. 普通 send：原有思考、文本、工具和完成状态不回归。
2. 首次输出前 steer：立即显示“正在追加”，成功后显示“等待进入当前处理步骤”。
3. steer RPC 已接受但 Codex 尚未应用：后续文本、工具和授权块都出现在 steer 上方，steer 临时保持在尾部。
4. Codex 发出对应 `userMessage item/started`：steer 固定在该真实事件边界，后续回复出现在其下方。
5. 连续多条 steer：按 Codex 的 `userMessage` 边界 FIFO 固定，顺序不变，各自状态独立。
6. 重新加载会话：已持久化的 steer 块不重复，也不回到助手消息顶部。
7. steer 是用户提示词：即使它嵌在助手事件序列中，也必须靠右显示；助手文本、工具和普通通知继续靠左。
8. 五分钟没有公开 reasoning：活动提示持续存在，界面不再表现为静止。
9. 工具执行期间 steer：工具状态优先，steer 仍显示已追加。
10. 等待授权期间 steer：显示等待授权，不伪装为思考。
11. steer RPC 失败：追加内容保留并显示失败原因。
12. turn 正常完成：尾部活动提示消失，动态 steer 状态归档。
13. turn 取消或失败：尾部活动提示消失，不残留“正在追加”。
14. 切换会话再返回：不能把已完成 turn 显示为运行中。

## 12. 完成标准

- 代码行为符合上述状态语义。
- 前端定向测试和 Rust 定向测试通过。
- 前端生产构建通过。
- 实际渲染中能看到 steer 状态与持续活动提示。
- 浏览器或可用的实际 Panes 调试界面中没有相关控制台错误。
- 不触碰主工作区现有的无关改动。

## 13. 执行清单

- [x] 类型与 IPC receipt
- [x] Rust steer receipt
- [x] Store 状态转换
- [x] Steer 状态展示
- [x] Turn 尾部活动提示
- [x] 前端单元测试
- [x] Rust 单元测试
- [x] 构建验证
- [x] 按真实 `userMessage` 边界修正 steer 落位
- [x] 多 steer 与重载去重测试
- [x] 修正后重新构建本地验收版
- [ ] 实际 steer 交互渲染验证

## 14. 本次验证记录

- TypeScript 类型检查：通过。
- `chatStore` 定向测试：38 个测试通过，包含真实事件顺序、两条 steer FIFO、RPC receipt 竞态和重载去重。
- Rust 定向测试：receipt 序列化与精确边界持久化测试通过。
- Rust 编译检查：通过。
- Rust 格式检查：通过。
- Tauri 无安装包构建：通过，仅生成本地验收用 `Panes.exe`，未生成安装包。
- 验收版 `Panes.exe`：已成功启动并加载现有会话。
- 实际 steer 交互：因 Windows 当前处于锁屏状态，未执行点击、输入或发送操作。
- 首次人工验收发现：RPC 接受时立即插入 steer 会让块错误固定在助手回复顶部。
- 实际 Codex rollout 证据显示：原始用户消息、助手说明、steer 用户消息、助手后续回复依次产生；修正方案改以第二个及后续 `userMessage item/started` 作为 steer 的真实落位边界。
- 修正后的本地验收版：`D:\work\panes_zh\.worktree\steer-progress-visibility\src-tauri\target\release\Panes.exe`，生成时间 2026-08-11 18:59:23。
- 修正后实际 steer 交互渲染：等待用户在本地验收版中测试确认。
- 第二次人工验收确认事件顺序正确，但发现 steer 作为用户提示词仍沿用助手侧左对齐样式。
- 已为 `.msg-notice--steer` 增加组件内靠右规则，普通通知、工具和助手正文不受影响。
- 靠右修正后的验收版生成时间：2026-08-11 19:24:44。
- 应用内 Browser 验证未执行成功，连接阶段返回 `Cannot redefine property: process`；未擅自切换到外部浏览器。

## 15. Hooks 跟随事件位置展示

人工验收继续发现：`Hooks (n)` 被固定在助手消息最上方，无法反映 Hook 实际发生在第几段内容之间。根因有两层：前后端存储都把 Notice 插入块数组头部；渲染组件又把所有 Hook Notice 从事件序列中抽出，统一放到消息顶部。

修正规则如下：

1. `hook_` 和兼容的 `codex_hook_` Notice 按收到时的位置追加，普通 Notice 继续置顶。
2. 渲染时不再全局抽取 Hook，而是在原始块序列中把连续 Hook 合并成局部 `Hooks (n)` 折叠组。
3. 文本、工具或其他内容会切断 Hook 分组，因此一条助手消息中可以出现多组 Hooks，每组都位于真实事件边界。
4. 已经持久化且顺序被旧版本改写的历史消息无法可靠恢复原始 Hook 位置；修正后新产生的事件可以准确展示。

新增顺序验收场景：

```text
助手文本 → Hooks (2) → 工具动作 → Hooks (1) → 助手文本
```

普通 Notice 的回归场景仍为：

```text
普通 Notice → 助手文本
```

本轮验证结果：

- TypeScript 类型检查通过。
- `chatStore` 39 项测试与 Hooks 分组 1 项测试通过，共 40 项。
- Rust 普通 Notice 置顶与 Hook Notice 保序测试通过，共 2 项。
- Rust 格式检查通过。
- Tauri 无安装包构建通过，新验收版生成时间为 2026-08-11 19:50:33。
- 新验收版路径：`D:\work\panes_zh\.worktree\steer-progress-visibility\src-tauri\target\release\Panes.exe`。
- 应用内 Browser 连接仍返回 `Cannot redefine property: process`，未擅自改用外部 Playwright；实际 Hooks 交互渲染等待人工验收。
