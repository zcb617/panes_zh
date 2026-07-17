import { create } from "zustand";
import {
  DEFAULT_CHAT_INPUT_SEND_SHORTCUT,
  isChatInputSendShortcut,
  type ChatInputSendShortcut,
} from "../lib/chatInputSettings";
import type { ComposerRuntimeSnapshot } from "../lib/newThreadRuntime";
import type { ChatAttachment } from "../types";
import {
  DEFAULT_LINK_OPEN_GESTURE,
  isLinkOpenGesture,
  type LinkOpenGesture,
} from "../lib/linkOpenSettings";

const CHAT_INPUT_SEND_SHORTCUT_STORAGE_KEY = "panes:chatInputSendShortcut";

const LINK_OPEN_GESTURE_STORAGE_KEY = "panes:linkOpenGesture";
function readChatInputSendShortcut(): ChatInputSendShortcut {
  try {
    const stored = localStorage.getItem(CHAT_INPUT_SEND_SHORTCUT_STORAGE_KEY);
    return stored && isChatInputSendShortcut(stored)
      ? stored
      : DEFAULT_CHAT_INPUT_SEND_SHORTCUT;
  } catch {
    return DEFAULT_CHAT_INPUT_SEND_SHORTCUT;
  }
}

function persistChatInputSendShortcut(sendShortcut: ChatInputSendShortcut): void {
  try {
    localStorage.setItem(CHAT_INPUT_SEND_SHORTCUT_STORAGE_KEY, sendShortcut);
  } catch {
    // The shortcut still applies to this session if storage is unavailable.
  }
}

function readLinkOpenGesture(): LinkOpenGesture {
  try {
    const stored = localStorage.getItem(LINK_OPEN_GESTURE_STORAGE_KEY);
    return stored && isLinkOpenGesture(stored)
      ? stored
      : DEFAULT_LINK_OPEN_GESTURE;
  } catch {
    return DEFAULT_LINK_OPEN_GESTURE;
  }
}

function persistLinkOpenGesture(linkOpenGesture: LinkOpenGesture): void {
  try {
    localStorage.setItem(LINK_OPEN_GESTURE_STORAGE_KEY, linkOpenGesture);
  } catch {
    // The setting still applies to this session if storage is unavailable.
  }
}

interface ChatComposerState {
  runtimeByWorkspace: Record<string, ComposerRuntimeSnapshot>;
  draftByWorkspace: Record<string, string>;
  attachmentsByWorkspace: Record<string, ChatAttachment[]>;
  sendShortcut: ChatInputSendShortcut;
  linkOpenGesture: LinkOpenGesture;
  setWorkspaceRuntime: (
    workspaceId: string,
    runtime: ComposerRuntimeSnapshot,
  ) => void;
  clearWorkspaceRuntime: (workspaceId: string) => void;
  setWorkspaceDraft: (workspaceId: string, draft: string) => void;
  clearWorkspaceDraft: (workspaceId: string) => void;
  setWorkspaceAttachments: (workspaceId: string, attachments: ChatAttachment[]) => void;
  clearWorkspaceAttachments: (workspaceId: string) => void;
  setSendShortcut: (sendShortcut: ChatInputSendShortcut) => void;
  setLinkOpenGesture: (linkOpenGesture: LinkOpenGesture) => void;
}

export const useChatComposerStore = create<ChatComposerState>((set) => ({
  runtimeByWorkspace: {},
  draftByWorkspace: {},
  attachmentsByWorkspace: {},
  sendShortcut: readChatInputSendShortcut(),
  linkOpenGesture: readLinkOpenGesture(),
  setWorkspaceRuntime: (workspaceId, runtime) =>
    set((state) => ({
      runtimeByWorkspace: {
        ...state.runtimeByWorkspace,
        [workspaceId]: runtime,
      },
    })),
  clearWorkspaceRuntime: (workspaceId) =>
    set((state) => {
      const { [workspaceId]: _removed, ...rest } = state.runtimeByWorkspace;
      return {
        runtimeByWorkspace: rest,
      };
    }),
  setWorkspaceDraft: (workspaceId, draft) =>
    set((state) => {
      if (!draft) {
        const { [workspaceId]: _removed, ...rest } = state.draftByWorkspace;
        return { draftByWorkspace: rest };
      }
      return {
        draftByWorkspace: {
          ...state.draftByWorkspace,
          [workspaceId]: draft,
        },
      };
    }),
  clearWorkspaceDraft: (workspaceId) =>
    set((state) => {
      const { [workspaceId]: _removed, ...rest } = state.draftByWorkspace;
      return { draftByWorkspace: rest };
    }),
  setWorkspaceAttachments: (workspaceId, attachments) =>
    set((state) => {
      if (attachments.length === 0) {
        const { [workspaceId]: _removed, ...rest } = state.attachmentsByWorkspace;
        return { attachmentsByWorkspace: rest };
      }
      return {
        attachmentsByWorkspace: {
          ...state.attachmentsByWorkspace,
          [workspaceId]: [...attachments],
        },
      };
    }),
  clearWorkspaceAttachments: (workspaceId) =>
    set((state) => {
      const { [workspaceId]: _removed, ...rest } = state.attachmentsByWorkspace;
      return { attachmentsByWorkspace: rest };
    }),
  setSendShortcut: (sendShortcut) => {
    persistChatInputSendShortcut(sendShortcut);
    set({ sendShortcut });
  },
  setLinkOpenGesture: (linkOpenGesture) => {
    persistLinkOpenGesture(linkOpenGesture);
    set({ linkOpenGesture });
  },
}));
