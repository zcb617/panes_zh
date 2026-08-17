import { describe, expect, it } from "vitest";
import type { CliSlashMenuContext } from "./contracts/slash-menu";
import { resolveCliSlashMenuAdapter } from "./registry";

function menuIds(cliId: "codex" | "opencode" | "claude"): string[] {
  const context = {
    inputMode: "classic",
    t: (key: string) => key,
    isSshWorkspace: true,
    extensionItems: [
      {
        id: "/srv/project/.claude/skills/review/SKILL.md",
        kind: "skill",
        name: "review",
        scope: "project",
      },
      { id: "claude-plugin", kind: "plugin", name: "Claude Plugin", scope: "user" },
      { id: "claude-mcp", kind: "mcp", name: "Claude MCP", scope: "project" },
    ],
    codexSkills: [{ name: "codex-skill", path: "/srv/project/skill", scope: "project" }],
    codexPlugins: [{ id: "codex-plugin", name: "Codex Plugin" }],
    codexMcpServers: [{ name: "Codex MCP", authStatus: "authenticated", toolCount: 1 }],
    canManageActiveCodexThread: true,
    canUseNativeCodexHistoryTools: true,
    openCodeCommands: [{ name: "deploy", hints: [] }],
    openCodeMcpServers: [{ name: "OpenCode MCP", status: "connected" }],
  } as unknown as CliSlashMenuContext;

  return resolveCliSlashMenuAdapter(cliId)!
    .buildSlashCommands(context)
    .map((command) => command.id);
}

describe("CLI slash menus", () => {
  it("only returns Codex commands and the current Codex target catalog", () => {
    const ids = menuIds("codex");
    expect(ids).toContain("review");
    expect(ids).toContain("skill:/srv/project/skill");
    expect(ids).not.toContain("agents");
    expect(ids).not.toContain("plugin:claude-plugin");
  });

  it("only returns OpenCode commands and the current OpenCode target catalog", () => {
    const ids = menuIds("opencode");
    expect(ids).toContain("agents");
    expect(ids).toContain("opencode-command:deploy");
    expect(ids).not.toContain("review");
    expect(ids).not.toContain("skill:/srv/project/skill");
  });

  it("only returns Claude Code extensions from the current target catalog", () => {
    const ids = menuIds("claude");
    expect(ids).toContain("skill:/srv/project/.claude/skills/review/SKILL.md");
    expect(ids).toContain("plugin:claude-plugin");
    expect(ids).toContain("mcp:claude-mcp");
    expect(ids).not.toContain("review");
    expect(ids).not.toContain("opencode-command:deploy");
  });
});
