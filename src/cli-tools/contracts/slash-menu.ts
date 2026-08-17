import type { TFunction } from "i18next";
import type { ChatInputMode } from "../../lib/chatInputSettings";
import type {
  CodexMcpServer,
  CodexPlugin,
  CodexSkill,
  ExtensionItem,
  OpenCodeCommand,
  OpenCodeMcpServer,
} from "../../types";
import type { CliSlashCommand } from "./slash-command";

export interface CliSlashMenuContext {
  inputMode: ChatInputMode;
  t: TFunction;
  isSshWorkspace: boolean;
  extensionItems: ExtensionItem[];
  codexSkills: CodexSkill[];
  codexPlugins: CodexPlugin[];
  codexMcpServers: CodexMcpServer[];
  canManageActiveCodexThread: boolean;
  canUseNativeCodexHistoryTools: boolean;
  openCodeCommands: OpenCodeCommand[];
  openCodeMcpServers: OpenCodeMcpServer[];
}

export interface CliSlashMenuAdapter {
  readonly id: "codex" | "opencode" | "claude";
  buildSlashCommands(context: CliSlashMenuContext): CliSlashCommand[];
}
