import { claudeCodeSlashMenuAdapter } from "./claude-code/slash-menu";
import type { CliSlashMenuAdapter } from "./contracts/slash-menu";
import { codexSlashMenuAdapter } from "./codex/slash-menu";
import { openCodeSlashMenuAdapter } from "./opencode/slash-menu";

const slashMenuAdapters: Record<CliSlashMenuAdapter["id"], CliSlashMenuAdapter> = {
  codex: codexSlashMenuAdapter,
  opencode: openCodeSlashMenuAdapter,
  claude: claudeCodeSlashMenuAdapter,
};

export function resolveCliSlashMenuAdapter(
  cliId: string | null | undefined,
): CliSlashMenuAdapter | null {
  if (cliId !== "codex" && cliId !== "opencode" && cliId !== "claude") {
    return null;
  }
  return slashMenuAdapters[cliId];
}
