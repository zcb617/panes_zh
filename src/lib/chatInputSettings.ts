export const CHAT_INPUT_SEND_SHORTCUTS = ["shift-enter", "enter"] as const;

export type ChatInputSendShortcut = (typeof CHAT_INPUT_SEND_SHORTCUTS)[number];

export const DEFAULT_CHAT_INPUT_SEND_SHORTCUT: ChatInputSendShortcut = "shift-enter";

export function isChatInputSendShortcut(value: string): value is ChatInputSendShortcut {
  return (CHAT_INPUT_SEND_SHORTCUTS as readonly string[]).includes(value);
}
