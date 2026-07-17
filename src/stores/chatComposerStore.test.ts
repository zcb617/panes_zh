import { beforeEach, describe, expect, it } from "vitest";
import {
  DEFAULT_CHAT_INPUT_MODE,
  DEFAULT_CHAT_INPUT_SEND_SHORTCUT,
} from "../lib/chatInputSettings";
import { useChatComposerStore } from "./chatComposerStore";
import { DEFAULT_LINK_OPEN_GESTURE } from "../lib/linkOpenSettings";

describe("chatComposerStore", () => {
  beforeEach(() => {
    useChatComposerStore.setState({
      runtimeByWorkspace: {},
      draftByWorkspace: {},
      attachmentsByWorkspace: {},
      referencesByWorkspace: {},
      sendShortcut: DEFAULT_CHAT_INPUT_SEND_SHORTCUT,
      chatInputMode: DEFAULT_CHAT_INPUT_MODE,
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

  it("updates the configured link open gesture", () => {
    useChatComposerStore.getState().setLinkOpenGesture("click");

    expect(useChatComposerStore.getState().linkOpenGesture).toBe("click");
  });
});
