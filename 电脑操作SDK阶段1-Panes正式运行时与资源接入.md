# 阶段 0 补充记录：Panes 正式运行时与资源接入

> 本文件是统一阶段 0 的历史补充记录。早期执行时曾暂用“阶段 1”文件名；统一编号后不再代表独立阶段。

- 状态：已完成，运行时层验证通过。
- 阶段目标：在 Panes 内按安装目录相对路径找到官方资源；按需初始化；提供统一的状态、初始化、工具列表、屏幕尺寸和关闭命令；不混用旧的 MCP 路由。
- 实际完成：
  - 新增 `src-tauri/src/computer_control_sdk.rs`，封装运行时资源定位、动态库加载、状态读取和生命周期操作。
  - 在 `src-tauri/src/lib.rs` 注册受管状态和 5 个应用命令：状态查询、初始化、工具列表、屏幕尺寸、关闭。
  - 在 `src-tauri/tauri.conf.json` 配置 `resources/cua-driver` 随应用资源发布。
  - 把官方 Windows x86_64 运行时文件放入 `src-tauri/resources/cua-driver/windows-x86_64/`。
  - 新增 `manifest.json`，记录官方来源、版本、压缩包哈希、动态库哈希和必需文件。
  - 保留原有 `src-tauri/src/commands/computer_control.rs`，本阶段没有把旧 MCP 实现混入新 SDK 路由。
  - 更新设计文档和集成测试清单，记录本阶段的范围和证据。
- 验证方式与结果：
  - Rust 静态检查：通过；仅有原有警告，没有新增错误。
  - 正式模块单元测试 `official_windows_release_runtime_smoke`：`1 passed, 0 failed`。
  - 单元测试实际验证：资源目录存在、清单存在、动态库存在、动态库哈希匹配、驱动初始化和关闭成功。
  - 前端构建：未执行成功，当前工作树没有安装前端依赖，类型检查命令不可用；这不是 SDK 运行时失败。
  - GUI 实际点击和授权弹窗：未执行。
  - macOS/Linux 运行时：未执行。
- 阶段结论：Panes 的 SDK 运行时接入层已经成立，能够从应用资源目录加载官方 Windows 包并完成最小生命周期调用；授权状态机、设置界面和模型工具调用仍未开始。
- 未完成项：尚未实现 `ComputerControlService` 业务层、按需授权申请、设置界面开关、应用授权记录、Codex/OpenCode/Claude Code 的工具适配，以及真实桌面验收。
- 下一阶段入口：先实现 Panes 自己的授权业务和 `ComputerControlService`，再接入具体的上层工具调用；每次调用前后继续新增对应阶段文件并更新索引。
