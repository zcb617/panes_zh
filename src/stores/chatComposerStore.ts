import { create } from "zustand";
import {
  DEFAULT_CHAT_INPUT_MODE,
  DEFAULT_CHAT_INPUT_SEND_SHORTCUT,
  isChatInputMode,
  isChatInputSendShortcut,
  type ChatInputMode,
  type ChatInputSendShortcut,
} from "../lib/chatInputSettings";
import {
  isMessageSendMode,
  type MessageSendMode,
} from "../lib/chatInputSettings";
import type { ComposerRuntimeSnapshot } from "../lib/newThreadRuntime";
import type {
  ChatAttachment,
  ChatInputReference,
  ChatTextAnnotation,
} from "../types";
import {
  DEFAULT_LINK_OPEN_GESTURE,
  isLinkOpenGesture,
  type LinkOpenGesture,
} from "../lib/linkOpenSettings";

const CHAT_INPUT_SEND_SHORTCUT_STORAGE_KEY = "panes:chatInputSendShortcut";

const CHAT_INPUT_MODE_STORAGE_KEY = "panes:chatInputMode";

const MESSAGE_SEND_MODE_STORAGE_KEY = "panes:messageSendMode";

const THREAD_MESSAGE_SEND_MODES_STORAGE_KEY = "panes:threadMessageSendModes";

const LINK_OPEN_GESTURE_STORAGE_KEY = "panes:linkOpenGesture";

export interface PendingFlexibleMessage {
  id: string;
  text: string;
  attachments: ChatAttachment[];
  references: ChatInputReference[];
}
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

function readChatInputMode(): ChatInputMode {
  try {
    const stored = localStorage.getItem(CHAT_INPUT_MODE_STORAGE_KEY);
    return stored && isChatInputMode(stored)
      ? stored
      : DEFAULT_CHAT_INPUT_MODE;
  } catch {
    return DEFAULT_CHAT_INPUT_MODE;
  }
}

function persistChatInputMode(chatInputMode: ChatInputMode): void {
  try {
    localStorage.setItem(CHAT_INPUT_MODE_STORAGE_KEY, chatInputMode);
  } catch {
    // The mode still applies to this session if storage is unavailable.
  }
}

function readMessageSendMode(): MessageSendMode {
  try {
    const stored = localStorage.getItem(MESSAGE_SEND_MODE_STORAGE_KEY);
    return stored && isMessageSendMode(stored) ? stored : "classic";
  } catch {
    return "classic";
  }
}

function persistMessageSendMode(messageSendMode: MessageSendMode): void {
  try {
    localStorage.setItem(MESSAGE_SEND_MODE_STORAGE_KEY, messageSendMode);
  } catch {
    // The mode still applies to this session if storage is unavailable.
  }
}

function readThreadMessageSendModes(): Record<string, MessageSendMode> {
  try {
    const stored = localStorage.getItem(THREAD_MESSAGE_SEND_MODES_STORAGE_KEY);
    if (!stored) {
      return {};
    }

    const parsed: unknown = JSON.parse(stored);
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
      return {};
    }

    const messageSendModes: Record<string, MessageSendMode> = {};
    for (const [threadId, messageSendMode] of Object.entries(parsed)) {
      if (isMessageSendMode(messageSendMode)) {
        messageSendModes[threadId] = messageSendMode;
      }
    }
    return messageSendModes;
  } catch {
    return {};
  }
}

function persistThreadMessageSendModes(
  messageSendModesByThread: Record<string, MessageSendMode>,
): void {
  try {
    localStorage.setItem(
      THREAD_MESSAGE_SEND_MODES_STORAGE_KEY,
      JSON.stringify(messageSendModesByThread),
    );
  } catch {
    // The thread override still applies to this session if storage is unavailable.
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
  textAnnotationsByWorkspace: Record<string, ChatTextAnnotation[]>;
  referencesByWorkspace: Record<string, ChatInputReference[]>;
  pendingFlexibleMessagesByWorkspace: Record<string, PendingFlexibleMessage[]>;
  pendingMessageSendModeByWorkspace: Record<string, MessageSendMode>;
  sendShortcut: ChatInputSendShortcut;
  chatInputMode: ChatInputMode;
  messageSendMode: MessageSendMode;
  messageSendModeByThread: Record<string, MessageSendMode>;
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
  setWorkspaceTextAnnotations: (
    workspaceId: string,
    annotations: ChatTextAnnotation[],
  ) => void;
  clearWorkspaceTextAnnotations: (workspaceId: string) => void;
  setWorkspaceReferences: (
    workspaceId: string,
    references: ChatInputReference[],
  ) => void;
  clearWorkspaceReferences: (workspaceId: string) => void;
  addPendingFlexibleMessage: (
    workspaceId: string,
    message: PendingFlexibleMessage,
  ) => void;
  clearPendingFlexibleMessages: (workspaceId: string) => void;
  setPendingMessageSendMode: (workspaceId: string, messageSendMode: MessageSendMode) => void;
  clearPendingMessageSendMode: (workspaceId: string) => void;
  setSendShortcut: (sendShortcut: ChatInputSendShortcut) => void;
  setChatInputMode: (chatInputMode: ChatInputMode) => void;
  setMessageSendMode: (messageSendMode: MessageSendMode) => void;
  setThreadMessageSendMode: (threadId: string, messageSendMode: MessageSendMode) => void;
  setLinkOpenGesture: (linkOpenGesture: LinkOpenGesture) => void;
}

export const useChatComposerStore = create<ChatComposerState>((set) => ({
  runtimeByWorkspace: {},
  draftByWorkspace: {},
  attachmentsByWorkspace: {},
  textAnnotationsByWorkspace: {},
  referencesByWorkspace: {},
  pendingFlexibleMessagesByWorkspace: {},
  pendingMessageSendModeByWorkspace: {},
  sendShortcut: readChatInputSendShortcut(),
  chatInputMode: readChatInputMode(),
  messageSendMode: readMessageSendMode(),
  messageSendModeByThread: readThreadMessageSendModes(),
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
  setWorkspaceTextAnnotations: (workspaceId, annotations) =>
    set((state) => {
      if (annotations.length === 0) {
        const { [workspaceId]: _removed, ...rest } = state.textAnnotationsByWorkspace;
        return { textAnnotationsByWorkspace: rest };
      }
      return {
        textAnnotationsByWorkspace: {
          ...state.textAnnotationsByWorkspace,
          [workspaceId]: [...annotations],
        },
      };
    }),
  clearWorkspaceTextAnnotations: (workspaceId) =>
    set((state) => {
      const { [workspaceId]: _removed, ...rest } = state.textAnnotationsByWorkspace;
      return { textAnnotationsByWorkspace: rest };
    }),
  setWorkspaceReferences: (workspaceId, references) =>
    set((state) => {
      if (references.length === 0) {
        const { [workspaceId]: _removed, ...rest } = state.referencesByWorkspace;
        return { referencesByWorkspace: rest };
      }
      return {
        referencesByWorkspace: {
          ...state.referencesByWorkspace,
          [workspaceId]: [...references],
        },
      };
    }),
  clearWorkspaceReferences: (workspaceId) =>
    set((state) => {
      const { [workspaceId]: _removed, ...rest } = state.referencesByWorkspace;
      return { referencesByWorkspace: rest };
    }),
  addPendingFlexibleMessage: (workspaceId, message) =>
    set((state) => ({
      pendingFlexibleMessagesByWorkspace: {
        ...state.pendingFlexibleMessagesByWorkspace,
        [workspaceId]: [
          ...(state.pendingFlexibleMessagesByWorkspace[workspaceId] ?? []),
          message,
        ],
      },
    })),
  clearPendingFlexibleMessages: (workspaceId) =>
    set((state) => {
      const { [workspaceId]: _removed, ...rest } = state.pendingFlexibleMessagesByWorkspace;
      return { pendingFlexibleMessagesByWorkspace: rest };
    }),
  setPendingMessageSendMode: (workspaceId, messageSendMode) =>
    set((state) => ({
      pendingMessageSendModeByWorkspace: {
        ...state.pendingMessageSendModeByWorkspace,
        [workspaceId]: messageSendMode,
      },
    })),
  clearPendingMessageSendMode: (workspaceId) =>
    set((state) => {
      const { [workspaceId]: _removed, ...rest } = state.pendingMessageSendModeByWorkspace;
      return { pendingMessageSendModeByWorkspace: rest };
    }),
  setSendShortcut: (sendShortcut) => {
    persistChatInputSendShortcut(sendShortcut);
    set({ sendShortcut });
  },
  setChatInputMode: (chatInputMode) => {
    persistChatInputMode(chatInputMode);
    set({ chatInputMode });
  },
  setMessageSendMode: (messageSendMode) => {
    persistMessageSendMode(messageSendMode);
    set({ messageSendMode });
  },
  setThreadMessageSendMode: (threadId, messageSendMode) =>
    set((state) => {
      const messageSendModeByThread = {
        ...state.messageSendModeByThread,
        [threadId]: messageSendMode,
      };
      persistThreadMessageSendModes(messageSendModeByThread);
      return { messageSendModeByThread };
    }),
  setLinkOpenGesture: (linkOpenGesture) => {
    persistLinkOpenGesture(linkOpenGesture);
    set({ linkOpenGesture });
  },
}));
