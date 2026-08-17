# Panes 项目长期笔记

## 斜杠菜单统一调用链（2026-08-17 落地）

按下 `/` 只有一个菜单。前端只调一次 `ipc.getCliExtensions(selectedEngineId, activeWorkspaceId)`；后端 `CliToolFactory::create(cliId)` → `CliTool::get_extensions(context)` → 返回统一 `ExtensionItemDto[]`。前端 `src/cli-tools/build-slash-commands.ts` 的 `buildSlashCommandsFromExtensions` 按 kind/insert_text/panel 解析。不再有三套适配器/三套扩展状态/三个分散 IPC（list_codex_skills 等）。详见 `docs/斜杠菜单功能改造.md` 和 `docs/多 CLI 工具统一接口架构设计.md`。

前后端解析协议：`ExtensionItemDto` 含 `insert_text/panel/group/disabled/search_terms`。选中行为优先级：insert_text 非空→插入文本；kind=skill→插 reference；panel=fast→fast 切换；其他 panel→开面板。classic 模式显示全部，非 classic 只显示 kind=command。

## 多 CLI 工具架构

Codex/OpenCode/Claude Code 各自实现 `CliTool` trait（`src-tauri/src/cli_tools.rs`），调用方只依赖接口。`CliToolFactory`（`cli_tools/factory.rs`）按 cliId 返回实现。SSH 目标不回退本机。Rust 用 `dyn Trait`，等价 Java interface。
