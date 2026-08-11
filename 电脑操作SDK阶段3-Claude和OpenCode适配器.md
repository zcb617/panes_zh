# Panes 电脑操作 SDK 阶段 3：Claude 和 OpenCode 适配器

## 1. 阶段信息

- 阶段：日志阶段 3 / 总体设计阶段 2
- 分支：`codex/computer-control-sdk`
- 工作目录：`.worktree/computer-control-sdk`
- 开始日期：2026-08-11
- 收尾日期：2026-08-11
- 关联主清单：[电脑操作 SDK 未完成清单](<电脑操作SDK未完成清单.md>)
- 关联测试清单：[电脑操作 SDK 集成测试清单](<电脑操作SDK集成测试清单.md>)

## 2. 阶段目标

本阶段只处理引擎适配层，不改设置页产品交互：

1. Claude 使用 Claude Agent SDK 的进程内自定义工具服务器，不再启动 `cua-driver mcp` 外部进程。
2. Claude 和 OpenCode 的电脑操作调用都回到 Panes 的 `ComputerControlService`，由同一个授权状态机决定是否调用 CUA Driver SDK。
3. OpenCode 使用每个 server 独立的 Panes 应用数据临时目录、临时工具文件和一次性本机回调令牌，不写用户全局配置。
4. 保留图片结果的结构化转发入口；对尚未取得真实 OpenCode 运行证据的图片合同，明确记录为待回归。

## 3. 阶段开始清单回看

阶段开始先通读主清单，筛选出当前阶段和当前阶段依赖的事项：

| 清单编号 | 本阶段处理方式 | 阶段开始结论 |
| --- | --- | --- |
| TODO-006 | 重新运行 Windows Rust 测试宿主 | 仍被 `0xc0000139 / STATUS_ENTRYPOINT_NOT_FOUND` 阻断，转由阶段 5 CI/构建环境处理 |
| TODO-011 | Claude sidecar 自定义工具转发 | 本阶段实现并完成 sidecar 协议测试 |
| TODO-012 | OpenCode 自定义工具、隔离目录和进程配置 | 本阶段实现并完成静态契约测试 |
| TODO-013 | OpenCode 图片 tool-result 合同 | 本阶段保留结构化回传代码；真实 OpenCode 图片运行证据仍待回归 |
| TODO-018 | 三引擎共用授权服务 | 本阶段把 Claude/OpenCode 接到统一服务；三个引擎实机隔离仍留待阶段 5 |

本阶段没有把设置页、旧 broker、旧 exe 白名单兼容清理提前混入适配器改动；这些事项仍按主清单进入阶段 4。

## 4. 实际完成内容

### 4.1 统一服务支持多引擎

`ComputerControlService` 增加通用的 `invoke_for_engine(agent, ...)` 入口，原有 Codex 入口保留为兼容包装。授权申请事件带有实际引擎标识，授权 grant key 也包含引擎，避免相同任务标识在不同引擎之间串用授权。

`EngineManager::set_computer_control_service` 现在同时绑定 Codex、Claude 和 OpenCode。

### 4.2 Claude 进程内 SDK 工具服务器

Claude sidecar 通过 `tool` 和 `createSdkMcpServer` 创建 `panes-computer-control` SDK server，登记已审核的电脑操作工具。工具 handler 发出 sidecar 事件：

```json
{
  "type": "computer_control_tool_call",
  "id": "query-id",
  "threadId": "panes-thread-id",
  "turnId": "query-id",
  "callId": "tool-use-id",
  "toolName": "click",
  "arguments": {}
}
```

Rust Claude 引擎收到事件后调用统一服务；授权完成或拒绝后，通过 `computer_control_tool_result` 把 CUA 结果或错误送回 sidecar。文本和图片 content 都保留为 Claude SDK 可识别的 MCP content block。

Claude transport 不再设置 `PANES_COMPUTER_CONTROL_CONFIG`，sidecar 也不再读取该文件来启动外部 MCP server。旧配置生成和 broker 兼容代码暂留到设置页迁移阶段，避免本阶段同时改变旧 UI 行为。

构建 sidecar 时同时携带 Claude SDK 的 peer zod 运行时，保证开发目录和发布资源目录都能创建自定义工具 schema。

### 4.3 OpenCode 隔离工具目录和回调

每个 OpenCode server 启动时：

- 在 Panes 应用数据目录下创建独立的 `computer-control/opencode-runs/<随机目录>`；
- 生成 `.opencode/tools/panes_computer_control.ts`，以命名导出登记已审核工具；
- 设置进程级 `OPENCODE_CONFIG_DIR` 和 `XDG_CONFIG_HOME`，不修改用户全局配置；
- 启动 `127.0.0.1` 随机端口的本机回调监听器，并把一次性令牌写入工具闭包；
- 回调收到工具名、任务、轮次、调用标识和参数后，调用统一 `ComputerControlService`，再返回 CUA 原始 content。

OpenCode 工具名会在回调入口去掉 `panes_computer_control_` 前缀后再进入审核工具白名单；回调令牌、目标任务和操作范围不会从请求参数中省略。

### 4.4 测试夹具

Claude sidecar mock 增加 SDK `tool` / `createSdkMcpServer` 模拟，并新增真实的“sidecar 发起工具调用—测试桥返回结果—sidecar 完成调用”测试，不再测试外部 `cua-driver.exe mcp` 注入。

## 5. 验证过程和结果

| 验证项 | 实际过程 | 结果 |
| --- | --- | --- |
| Rust 静态编译 | 在 `src-tauri` 运行 Rust 静态检查命令 | 通过；仅有仓库既有 warning 和本阶段 `run_dir` 未读取 warning |
| Claude sidecar 语法 | 对 `src-tauri/sidecar/claude-agent-sdk-server.mjs` 运行脚本语法检查 | 通过 |
| Claude sidecar 协议测试 | 运行 `vitest run tests/claudeAgentSidecar.test.ts` | 18 个通过、1 个平台跳过 |
| Claude SDK API 契约 | 使用已安装 `@anthropic-ai/claude-agent-sdk@0.3.220` 的导出和类型声明核对 `tool`、`createSdkMcpServer`、Zod raw shape | 通过 |
| OpenCode 工具加载契约 | 检查本机当前 OpenCode 可执行文件，确认其加载 `tools/*.{js,ts}`，工具对象使用 `description`、`parameters`、`execute` | 通过静态契约核对 |
| OpenCode 工具源生成 | 增加生成路径、一次性 token、工具命名导出和回调入口单元测试 | 代码已覆盖；Windows 测试宿主无法启动 |
| Windows Rust 单元测试宿主 | 阶段开始重试和阶段实现后重试 | 阻断：`0xc0000139 / STATUS_ENTRYPOINT_NOT_FOUND`，不是测试断言失败 |
| OpenCode 真实图片回传 | 尚未启动真实 provider/业务窗口执行截图工具 | 未执行，保留 TODO-013 |
| 全部前端测试 | 运行完整 Vitest 集合 | 76 个测试文件通过，1 个既有 `pt-BR`/`en` 文案键对齐测试失败，与本阶段改动无关 |

## 6. 未完成项和风险

1. TODO-006：Windows 测试宿主仍缺系统入口点，已明确转阶段 5 CI/构建环境处理。
2. TODO-013：OpenCode 图片 tool-result 只完成结构化回传入口和静态契约核对，尚未有当前 OpenCode 版本的真实图片结果证据。
3. TODO-011、TODO-012：已完成代码适配，但尚未在 Panes GUI 中使用真实 Claude/OpenCode 会话完成授权、窗口动作和进程回收；对应实机证据留到阶段 5。
4. 旧设置页仍可能读取或生成外部 broker/exe 兼容字段；本阶段没有把旧 UI 清理冒充为适配器完成，阶段 4 负责迁移。

## 7. 阶段收尾清单回看

收尾再次回看 TODO-006、TODO-011、TODO-012、TODO-013、TODO-018：

- TODO-006 已重新执行，结果不变，状态保持“阻断”，计划改为阶段 5 CI/构建环境处理。
- TODO-011 已有 sidecar 协议和 Rust 转发代码证据，但真实 GUI 授权仍待阶段 5，因此状态改为“待回归”。
- TODO-012 已有 OpenCode 隔离目录、进程级配置、回调和工具源生成代码证据，但真实 server 调用仍待阶段 5，因此状态改为“待回归”。
- TODO-013 没有真实图片结果，状态保持“待回归”。
- TODO-018 仅完成服务绑定和引擎字段隔离，三引擎并行实机验证留待阶段 5，状态保持“未完成”。

## 8. 下一阶段入口

下一阶段为日志阶段 4 / 总体设计阶段 3：设置页迁移。入口要求：

- 电脑操作能力继续作为 Panes 独立功能，不混入左上角 MCP UI；
- 设置页不再要求预先添加 exe；
- 开关只控制 Panes 统一服务是否允许模型在实际调用时申请授权；
- 迁移旧检测、黑框、`检测中` 和外部 MCP/broker 配置前，先依据主清单逐项决定兼容读取和停止写入策略。
