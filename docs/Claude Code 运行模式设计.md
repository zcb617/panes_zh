# Claude Code 运行模式设计

## 一、文档目标

本文规定 Panes 使用 SSH 远端 Claude Code 时支持的两种运行模式，以及系统配置参数对运行线路的选择规则。

会话句柄的建立、复用、空闲计时和销毁由 [Claude Code CLI 会话进程复用优化方案](Claude%20Code%20CLI%20会话进程复用优化方案.md) 规定，本文不重复描述其内部生命周期。

## 二、运行模式

### 2.1 会话复用模式

同一个 Panes 会话建立一个 Claude Code 会话句柄。

同一会话的多轮对话持续复用该会话句柄和对应的 Claude Code 进程。一轮对话结束后不立即关闭，后续关闭由会话句柄生命周期管理规则决定。

这是系统默认运行模式。

### 2.2 单轮启动模式

同一个 Panes 会话的每轮对话分别启动一个 Claude Code 进程。

当前一轮结束后，本轮 Claude Code 进程随之结束；下一轮对话重新启动 Claude Code，并使用已有 Claude Code 会话编号恢复对话历史。

该模式保留现有单轮运行线路，不修改其内部实现。

## 三、系统配置参数

运行模式由 Panes 系统配置文件 `config.toml` 控制：

```toml
[claude_code]
session_mode = "reuse_session"
```

支持的配置值：

| 配置值 | 运行模式 |
| --- | --- |
| `reuse_session` | 会话复用模式 |
| `per_turn` | 单轮启动模式 |

配置文件没有该参数时，默认使用 `reuse_session`。

开发环境使用 `.panes-dev/config.toml`；正式环境使用 Panes 应用数据目录中的 `config.toml`。

## 四、配置入口边界

该参数是系统运行参数，不是普通用户功能：

- 不在配置界面展示；
- 不提供前端修改入口；
- 不保存到项目、会话或数据库；
- 不随项目和会话切换；
- 需要排查或回退时，由开发人员直接修改 `config.toml`。

## 五、运行线路选择

`ClaudeCodeCli` 读取系统配置参数，并按参数选择运行线路：

```text
ClaudeCodeCli
→ session_mode = reuse_session
   → 进入会话复用线路

ClaudeCodeCli
→ session_mode = per_turn
   → 进入现有单轮启动线路
```

两条线路同时保留，但同一时刻只执行配置参数指定的线路。

会话复用线路使用 Claude Code 会话句柄生命周期管理类；单轮启动线路继续使用当前每轮启动的实现。

## 六、固定要求

1. 默认使用会话复用模式。
2. 当前单轮启动线路必须保留。
3. 会话复用线路不得通过修改单轮线路内部实现完成。
4. 两条线路由 `ClaudeCodeCli` 根据系统配置参数选择。
5. 配置参数不得暴露到用户配置界面。
6. SSH 失败时，两种模式都不得回退到本机 Claude Code、其他 CLI 或其他项目目录。
