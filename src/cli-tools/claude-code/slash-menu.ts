import { Puzzle, Server, Sparkles } from "lucide-react";
import type { CliSlashMenuAdapter, CliSlashMenuContext } from "../contracts/slash-menu";
import type { CliSlashCommand } from "../contracts/slash-command";

export const claudeCodeSlashMenuAdapter: CliSlashMenuAdapter = {
  id: "claude",
  buildSlashCommands(context) {
    const skills = context.extensionItems
      .filter((item) => item.kind === "skill")
      .map((skill) => ({
        id: `skill:${skill.id}`,
        name: skill.name,
        description: skill.description || skill.scope,
        icon: Sparkles,
        group: context.t("slashCommands.groups.skills"),
        searchTerms: [skill.id, skill.path ?? "", skill.scope],
        action: { type: "insert" as const, text: `/${skill.name} ` },
      }));
    const plugins = context.extensionItems
      .filter((item) => item.kind === "plugin")
      .map((plugin) => ({
        id: `plugin:${plugin.id}`,
        name: plugin.name,
        description: plugin.description || plugin.marketplace || plugin.scope,
        icon: Puzzle,
        group: context.t("slashCommands.groups.plugins"),
        searchTerms: [plugin.id, plugin.marketplace ?? "", plugin.source ?? "", plugin.scope],
        action: { type: "panel" as const, panel: { type: "plugins" as const } },
      }));
    const mcpServers = context.extensionItems
      .filter((item) => item.kind === "mcp")
      .map((server) => ({
        id: `mcp:${server.id}`,
        name: server.name,
        description: server.warning || server.description || server.health,
        icon: Server,
        group: context.t("slashCommands.groups.mcp"),
        searchTerms: [server.health, server.authState ?? "", server.scope],
        action: { type: "panel" as const, panel: { type: "mcp" as const } },
      }));

    if (context.inputMode === "classic") {
      return [...skills, ...plugins, ...mcpServers];
    }

    const commands: CliSlashCommand[] = [
      {
        id: "skills",
        name: "skills",
        description: context.t("slashCommands.panels.skills.description"),
        icon: Sparkles,
        action: { type: "panel", panel: { type: "skills" } },
      },
      {
        id: "plugins",
        name: "plugins",
        description: context.t("slashCommands.panels.plugins.title"),
        icon: Puzzle,
        action: { type: "panel", panel: { type: "plugins" } },
      },
      {
        id: "mcp",
        name: "MCP",
        description: context.t("slashCommands.panels.mcp.description"),
        icon: Server,
        action: { type: "panel", panel: { type: "mcp" } },
      },
    ];
    return commands;
  },
};
