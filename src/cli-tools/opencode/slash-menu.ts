import { GitBranch, Server, SquareCode, UserCircle } from "lucide-react";
import type { CliSlashMenuAdapter, CliSlashMenuContext } from "../contracts/slash-menu";
import type { CliSlashCommand } from "../contracts/slash-command";

export const openCodeSlashMenuAdapter: CliSlashMenuAdapter = {
  id: "opencode",
  buildSlashCommands(context) {
    const commandGroup = context.t("slashCommands.groups.commands");
    const mcpGroup = context.t("slashCommands.groups.mcp");
    const commands: CliSlashCommand[] = [
      {
        id: "agents",
        name: "agents",
        description: context.t("slashCommands.panels.openCodeAgents.description"),
        icon: UserCircle,
        group: commandGroup,
        action: { type: "panel", panel: { type: "agents" } },
      },
      {
        id: "commands",
        name: "commands",
        description: context.t("slashCommands.panels.openCodeCommands.description"),
        icon: SquareCode,
        group: commandGroup,
        action: { type: "panel", panel: { type: "commands" } },
      },
      {
        id: "sessions",
        name: "sessions",
        description: context.t("slashCommands.panels.openCodeSessions.description"),
        icon: GitBranch,
        group: commandGroup,
        action: { type: "panel", panel: { type: "sessions" } },
      },
      ...context.openCodeCommands.map((command) => ({
        id: `opencode-command:${command.name}`,
        name: command.name,
        description:
          command.description ||
          (command.hints.length > 0
            ? command.hints.join(" ")
            : context.t("slashCommands.panels.openCodeCommands.insertDescription")),
        icon: command.subtask ? GitBranch : SquareCode,
        group: commandGroup,
        action: { type: "insert" as const, text: `/${command.name} ` },
      })),
    ];
    const mcpServers = context.openCodeMcpServers.map((server) => ({
      id: `mcp:${server.name}`,
      name: server.name,
      description: server.detail || server.status,
      icon: Server,
      group: mcpGroup,
      searchTerms: [server.status],
      action: { type: "panel" as const, panel: { type: "mcp" as const } },
    }));

    if (context.inputMode === "classic") {
      return [...commands, ...mcpServers];
    }

    return [
      ...commands,
      {
        id: "mcp",
        name: "MCP",
        description: context.t("slashCommands.panels.mcp.description"),
        icon: Server,
        action: { type: "panel", panel: { type: "mcp" } },
      },
    ];
  },
};
