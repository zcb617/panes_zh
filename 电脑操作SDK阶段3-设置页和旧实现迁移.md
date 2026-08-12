# 阶段 3：设置页和旧实现迁移

执行时间：2026-08-12（Asia/Hong_Kong）

执行分支：`codex/computer-control-sdk`

工作树：`D:\\work\\panes_zh\\.worktree\\computer-control-sdk`

关联主清单：[电脑操作 SDK 未完成清单](<电脑操作SDK未完成清单.md>) 的 TODO-014、TODO-015、TODO-016、TODO-017。

## 阶段目标

将设置页从“外部 driver、MCP 配置和 exe 白名单”产品路径迁移为 Panes 内置的电脑操作能力；设置页只能读取和控制当前 Panes 主进程中的 CUA SDK 状态，不能因为打开页面或切换开关启动外部检测进程、broker、第二个 Panes 或全局 MCP 配置写入。

## 实际完成内容

### 1. 直接 SDK 设置命令

- 新增 `src-tauri/src/commands/computer_control_settings.rs`，提供电脑操作设置状态、总开关和单条授权撤销命令。
- 状态仅由 Panes 配置、内置 `ComputerControlService` 和 `CuaDriverSdk::status()` 组成；设置页不会自行初始化 SDK，也不会探测 PATH、执行外部 driver 或读取用户全局 CLI 配置。SDK 由 Panes 启动阶段初始化。
- SDK 状态统一为：`disabled`、`uninitialized`、`ready`、`failed`、`unsupported`。
- 开启总开关时先初始化 SDK；关闭总开关时撤销全部 pending/grant 授权，并显式 shutdown 当前 SDK runtime。

### 2. 临时授权状态和设置页展示

- 授权 grant 改为保存完整授权记录，可按 `request_id` 列出和撤销；记录包含引擎、工具、调用、应用、操作类型、范围、任务和回合。
- 设置页删除产品流程中的“添加 exe / 允许应用”入口，改为独立开关、CUA SDK 状态、当前授权、内置引擎适配器和既有安全说明。
- Codex、Claude Code、OpenCode 显示为内置适配器；当前授权可在设置页撤销。
- 授权弹窗补充操作类型、授权范围、任务和回合，便于用户在执行前判断请求。
- 英文、简体中文、葡萄牙语资源同步更新，并保持相同键结构。

### 3. 旧 MCP / broker 路径停用并保留审计代码

- `src-tauri/src/lib.rs` 不再在 Panes 启动时调用 `start_approval_broker`，因此不再生成 broker 文件或监听旧 broker 端口。
- 旧的 `--panes-computer-control-mcp` CLI 分支已从正常启动调度中注释停用。
- 旧的外部 driver 状态与设置命令不再注册给 WebView，正常前端无法再触发全局 MCP、broker 或外部 `cua-driver` 配置写入。
- 旧实现按项目规则保留为带说明的历史迁移代码；不自动清理用户既有全局 MCP 配置，也不把历史白名单转换为永久授权。

## 验证结果

| 验证项 | 实际结果 |
| --- | --- |
| 前端类型检查 | 静态类型检查退出码 0。 |
| Rust 电脑操作定向测试 | 此前 14 通过，0 失败，548 过滤；其中“设置状态不初始化 SDK”的历史断言已不再适用，需按启动初始化规则重新验证。 |
| 多语言资源 | 英文、简体中文、葡萄牙语 JSON 均可解析，叶子键集合一致。 |
| Windows GUI / 真实电脑操作 | 未执行。未启动 Panes GUI、真实引擎会话或测试窗口，未生成 `request_id`、窗口变化或进程列表证据。 |
| 本地验收构建 | 未执行。按项目规则，须先由用户完成实际功能测试并明确确认后才可生成 `Panes.exe`。 |

## 主清单回看与结论

- 原“授权后才初始化 SDK”的实现违反 SDK 就绪先于授权的边界，已列为本轮修正项；需要重新执行静态和 Windows 实机验证。
- TODO-014、TODO-015、TODO-016、TODO-017 的代码迁移已完成，均转为阶段 4 的实机回归项。
- 未执行 G1 和 G10 的 Windows GUI/进程链用例，不能把本阶段静态验证写成最终功能通过。
- 历史外部 MCP 代码目前不走正常启动或 WebView 命令路径；仍需在实际 Windows 环境确认不存在第二个 Panes、`--panes-computer-control-mcp`、黑框或“检测中”死循环。

## 下一阶段入口

进入阶段 4，按集成测试清单的 G1、G2、G7、G8、G9、G10 顺序执行真实 Windows GUI 回归。优先完成：功能开启后的 SDK 就绪状态、首次工具调用授权弹窗、Notepad 等安全目标窗口动作、关闭开关回收、三个引擎的真实调用，以及进程/黑框检查。
