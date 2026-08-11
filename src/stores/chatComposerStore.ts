import { create } from "zustand";
import {
  DEFAULT_CHAT_INPUT_MODE,
  DEFAULT_CHAT_INPUT_SEND_SHORTCUT,
  isChatInputMode,
  isChatInputSendShortcut,
  type ChatInputMode,
  type ChatInputSendShortcut,
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
  sendShortcut: ChatInputSendShortcut;
  chatInputMode: ChatInputMode;
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
  setSendShortcut: (sendShortcut: ChatInputSendShortcut) => void;
  setChatInputMode: (chatInputMode: ChatInputMode) => void;
  setLinkOpenGesture: (linkOpenGesture: LinkOpenGesture) => void;
}

export const useChatComposerStore = create<ChatComposerState>((set) => ({
  runtimeByWorkspace: {},
  draftByWorkspace: {},
  attachmentsByWorkspace: {},
  textAnnotationsByWorkspace: {},
  referencesByWorkspace: {},
  sendShortcut: readChatInputSendShortcut(),
  chatInputMode: readChatInputMode(),
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
  setSendShortcut: (sendShortcut) => {
    persistChatInputSendShortcut(sendShortcut);
    set({ sendShortcut });
  },
  setChatInputMode: (chatInputMode) => {
    persistChatInputMode(chatInputMode);
    set({ chatInputMode });
  },
  setLinkOpenGesture: (linkOpenGesture) => {
    persistLinkOpenGesture(linkOpenGesture);
    set({ linkOpenGesture });
  },
}));
