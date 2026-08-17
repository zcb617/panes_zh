import {
  FlaskConical,
  GitBranch,
  Puzzle,
  RotateCcw,
  Scissors,
  Search,
  Server,
  Sparkles,
  UserCircle,
  Zap,
} from "lucide-react";
import type { CliSlashMenuAdapter, CliSlashMenuContext } from "../contracts/slash-menu";
import type { CliSlashCommand } from "../contracts/slash-command";

function commandGroup(context: CliSlashMenuContext): string {
  return context.t("slashCommands.groups.commands");
}

export const codexSlashMenuAdapter: CliSlashMenuAdapter = {
  id: "codex",
  buildSlashCommands(context) {
    const commands: CliSlashCommand[] = [
      {
        id: "review",
        name: "review",
        description: context.t("reviewPicker.subtitle"),
        icon: Search,
        group: commandGroup(context),
        disabled: !context.canManageActiveCodexThread,
        action: { type: "panel", panel: { type: "review" } },
      },
      {
        id: "fork",
        name: "fork",
        description: context.t("threadPicker.forkDescription"),
        icon: GitBranch,
        group: commandGroup(context),
        disabled: !context.canUseNativeCodexHistoryTools,
        action: { type: "panel", panel: { type: "fork" } },
      },
      {
        id: "rollback",
        name: "rollback",
        description: context.t("threadPicker.rollbackDescription"),
        icon: RotateCcw,
        group: commandGroup(context),
        disabled: !context.canUseNativeCodexHistoryTools,
        action: { type: "panel", panel: { type: "rollback" } },
      },
      {
        id: "compact",
        name: "compact",
        description: context.t("threadPicker.compactDescription"),
        icon: Scissors,
        group: commandGroup(context),
        disabled: !context.canManageActiveCodexThread,
        action: { type: "panel", panel: { type: "compact" } },
      },
      {
        id: "fast",
        name: "fast",
        description: context.t("configPicker.serviceTierDescription"),
        icon: Zap,
        group: commandGroup(context),
        action: { type: "fast" },
      },
      {
        id: "personality",
        name: "personality",
        description: context.t("configPicker.personalityDescription"),
        icon: UserCircle,
        group: commandGroup(context),
        action: { type: "panel", panel: { type: "personality" } },
      },
      {
        id: "experimental",
        name: "experimental",
        description: context.t("slashCommands.panels.experimental.description"),
        icon: FlaskConical,
        group: commandGroup(context),
        action: { type: "panel", panel: { type: "experimental" } },
      },
    ];

    const skillGroup = context.t("slashCommands.groups.skills");
    const pluginGroup = context.t("slashCommands.groups.plugins");
    const mcpGroup = context.t("slashCommands.groups.mcp");
    const skills = context.isSshWorkspace
      ? context.codexSkills.map((skill) => ({
          id: `skill:${skill.path}`,
          name: skill.name,
          description: skill.description || skill.scope,
          icon: Sparkles,
          group: skillGroup,
          searchTerms: [skill.path, skill.scope],
          action: {
            type: "reference" as const,
            reference: { type: "skill" as const, name: skill.name, path: skill.path },
          },
        }))
      : context.extensionItems
          .filter((item) => item.kind === "skill")
          .map((skill) => ({
            id: `skill:${skill.id}`,
            name: skill.name,
            description: skill.description || skill.scope,
            icon: Sparkles,
            group: skillGroup,
            searchTerms: [skill.id, skill.path ?? "", skill.scope],
            action: {
              type: "reference" as const,
              reference: {
                type: "skill" as const,
                name: skill.name,
                path: skill.path || skill.id,
              },
            },
          }));
    const plugins = context.isSshWorkspace
      ? context.codexPlugins.map((plugin) => ({
          id: `plugin:${plugin.id}`,
          name: plugin.name,
          description: plugin.description || plugin.id,
          icon: Puzzle,
          group: pluginGroup,
          searchTerms: [plugin.id, plugin.developerName ?? ""],
          action: { type: "panel" as const, panel: { type: "plugins" as const } },
        }))
      : context.extensionItems
          .filter((item) => item.kind === "plugin")
          .map((plugin) => ({
            id: `plugin:${plugin.id}`,
            name: plugin.name,
            description: plugin.description || plugin.marketplace || plugin.scope,
            icon: Puzzle,
            group: pluginGroup,
            searchTerms: [
              plugin.id,
              plugin.marketplace ?? "",
              plugin.source ?? "",
              plugin.scope,
            ],
            action: { type: "panel" as const, panel: { type: "plugins" as const } },
          }));
    const mcpServers = context.isSshWorkspace
      ? context.codexMcpServers.map((server) => ({
          id: `mcp:${server.name}`,
          name: server.name,
          description: `${server.authStatus} · ${server.toolCount} tools`,
          icon: Server,
          group: mcpGroup,
          searchTerms: [server.authStatus],
          action: { type: "panel" as const, panel: { type: "mcp" as const } },
        }))
      : context.extensionItems
          .filter((item) => item.kind === "mcp")
          .map((server) => ({
            id: `mcp:${server.id}`,
            name: server.name,
            description: server.warning || server.description || server.health,
            icon: Server,
            group: mcpGroup,
            searchTerms: [server.health, server.authState ?? "", server.scope],
            action: { type: "panel" as const, panel: { type: "mcp" as const } },
          }));

    if (context.inputMode === "classic") {
      return [...commands, ...skills, ...plugins, ...mcpServers];
    }

    return [
      ...commands,
      {
        id: "skills",
        name: "skills",
        description: context.t("slashCommands.panels.skills.description"),
        icon: Sparkles,
        action: { type: "panel", panel: { type: "skills" } },
      },
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
