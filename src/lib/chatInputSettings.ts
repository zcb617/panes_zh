export const CHAT_INPUT_SEND_SHORTCUTS = ["shift-enter", "enter"] as const;

export type ChatInputSendShortcut = (typeof CHAT_INPUT_SEND_SHORTCUTS)[number];

export const DEFAULT_CHAT_INPUT_SEND_SHORTCUT: ChatInputSendShortcut = "shift-enter";

export function isChatInputSendShortcut(value: string): value is ChatInputSendShortcut {
  return (CHAT_INPUT_SEND_SHORTCUTS as readonly string[]).includes(value);
}

export const CHAT_INPUT_MODES = ["default", "classic"] as const;

export type ChatInputMode = (typeof CHAT_INPUT_MODES)[number];

export const DEFAULT_CHAT_INPUT_MODE: ChatInputMode = "default";

export function isChatInputMode(value: string): value is ChatInputMode {
  return (CHAT_INPUT_MODES as readonly string[]).includes(value);
}
