import { beforeEach, describe, expect, it } from "vitest";
import {
  DEFAULT_CHAT_INPUT_MODE,
  DEFAULT_CHAT_INPUT_SEND_SHORTCUT,
} from "../lib/chatInputSettings";
import { DEFAULT_MESSAGE_SEND_MODE } from "../lib/chatInputSettings";
import { useChatComposerStore } from "./chatComposerStore";
import { DEFAULT_LINK_OPEN_GESTURE } from "../lib/linkOpenSettings";

describe("chatComposerStore", () => {
  beforeEach(() => {
    useChatComposerStore.setState({
      runtimeByWorkspace: {},
      draftByWorkspace: {},
      attachmentsByWorkspace: {},
      textAnnotationsByWorkspace: {},
      referencesByWorkspace: {},
      pendingFlexibleMessagesBySession: {},
      pendingMessageSendModeByWorkspace: {},
      sendShortcut: DEFAULT_CHAT_INPUT_SEND_SHORTCUT,
      chatInputMode: DEFAULT_CHAT_INPUT_MODE,
      messageSendMode: DEFAULT_MESSAGE_SEND_MODE,
      messageSendModeByThread: {},
      linkOpenGesture: DEFAULT_LINK_OPEN_GESTURE,
    });
  });

  it("keeps a workspace draft while the chat panel is unmounted", () => {
    const store = useChatComposerStore.getState();

    store.setWorkspaceDraft("workspace-a", "Continue this thought");

    expect(useChatComposerStore.getState().draftByWorkspace).toEqual({
      "workspace-a": "Continue this thought",
    });

    useChatComposerStore.getState().clearWorkspaceDraft("workspace-a");

    expect(useChatComposerStore.getState().draftByWorkspace).toEqual({});
  });

  it("keeps workspace attachments while the chat panel is unmounted", () => {
    const attachments = [
      {
        id: "clipboard-image",
        fileName: "clipboard.png",
        filePath: "C:/tmp/clipboard.png",
        sizeBytes: 42,
        mimeType: "image/png",
      },
    ];

    useChatComposerStore
      .getState()
      .setWorkspaceAttachments("workspace-a", attachments);

    expect(useChatComposerStore.getState().attachmentsByWorkspace).toEqual({
      "workspace-a": attachments,
    });

    useChatComposerStore.getState().clearWorkspaceAttachments("workspace-a");

    expect(useChatComposerStore.getState().attachmentsByWorkspace).toEqual({});
  });

  it("keeps workspace references while the chat panel is unmounted", () => {
    const references = [
      { type: "skill" as const, name: "task-anchor", path: "/skills/task-anchor" },
      { type: "mention" as const, name: "Docs", path: "app://docs" },
    ];

    useChatComposerStore
      .getState()
      .setWorkspaceReferences("workspace-a", references);

    expect(useChatComposerStore.getState().referencesByWorkspace).toEqual({
      "workspace-a": references,
    });

    useChatComposerStore.getState().clearWorkspaceReferences("workspace-a");

    expect(useChatComposerStore.getState().referencesByWorkspace).toEqual({});
  });

  it("updates the configured send shortcut", () => {
    useChatComposerStore.getState().setSendShortcut("enter");

    expect(useChatComposerStore.getState().sendShortcut).toBe("enter");
  });

  it("updates the configured chat input mode", () => {
    useChatComposerStore.getState().setChatInputMode("classic");

    expect(useChatComposerStore.getState().chatInputMode).toBe("classic");
  });

  it("updates the configured message send mode", () => {
    useChatComposerStore.getState().setMessageSendMode("flexible");

    expect(useChatComposerStore.getState().messageSendMode).toBe("flexible");
  });

  it("keeps a message send mode override for an existing thread", () => {
    useChatComposerStore.getState().setThreadMessageSendMode("thread-a", "flexible");

    expect(useChatComposerStore.getState().messageSendModeByThread).toEqual({
      "thread-a": "flexible",
    });
  });

  it("keeps the selected message send mode until a new thread is created", () => {
    useChatComposerStore
      .getState()
      .setPendingMessageSendMode("workspace-a", "flexible");

    expect(useChatComposerStore.getState().pendingMessageSendModeByWorkspace).toEqual({
      "workspace-a": "flexible",
    });

    useChatComposerStore.getState().clearPendingMessageSendMode("workspace-a");

    expect(useChatComposerStore.getState().pendingMessageSendModeByWorkspace).toEqual({});
  });

  it("isolates pending flexible messages between threads in the same workspace", () => {
    const firstMessage = {
      id: "pending-message-a",
      text: "Wait until I finish the details.",
      attachments: [],
      references: [],
    };
    const secondMessage = {
      id: "pending-message-b",
      text: "This belongs to another thread.",
      attachments: [],
      references: [],
    };

    useChatComposerStore
      .getState()
      .addPendingFlexibleMessage("thread:thread-a", firstMessage);
    useChatComposerStore
      .getState()
      .addPendingFlexibleMessage("thread:thread-b", secondMessage);

    expect(useChatComposerStore.getState().pendingFlexibleMessagesBySession).toEqual({
      "thread:thread-a": [firstMessage],
      "thread:thread-b": [secondMessage],
    });

    useChatComposerStore
      .getState()
      .clearPendingFlexibleMessages("thread:thread-a");

    expect(useChatComposerStore.getState().pendingFlexibleMessagesBySession).toEqual({
      "thread:thread-b": [secondMessage],
    });
  });

  it("removes only the selected pending flexible message", () => {
    const firstMessage = {
      id: "pending-message-1",
      text: "First held message",
      attachments: [],
      references: [],
    };
    const secondMessage = {
      id: "pending-message-2",
      text: "Second held message",
      attachments: [],
      references: [],
    };

    useChatComposerStore
      .getState()
      .addPendingFlexibleMessage("thread:thread-a", firstMessage);
    useChatComposerStore
      .getState()
      .addPendingFlexibleMessage("thread:thread-a", secondMessage);
    useChatComposerStore
      .getState()
      .removePendingFlexibleMessage("thread:thread-a", firstMessage.id);

    expect(useChatComposerStore.getState().pendingFlexibleMessagesBySession).toEqual({
      "thread:thread-a": [secondMessage],
    });
  });

  it("updates the configured link open gesture", () => {
    useChatComposerStore.getState().setLinkOpenGesture("click");

    expect(useChatComposerStore.getState().linkOpenGesture).toBe("click");
  });
});
