import { beforeEach, describe, expect, it } from "vitest";
import { DEFAULT_CHAT_INPUT_SEND_SHORTCUT } from "../lib/chatInputSettings";
import { useChatComposerStore } from "./chatComposerStore";
import { DEFAULT_LINK_OPEN_GESTURE } from "../lib/linkOpenSettings";

describe("chatComposerStore", () => {
  beforeEach(() => {
    useChatComposerStore.setState({
      runtimeByWorkspace: {},
      draftByWorkspace: {},
      attachmentsByWorkspace: {},
      sendShortcut: DEFAULT_CHAT_INPUT_SEND_SHORTCUT,
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

  it("updates the configured send shortcut", () => {
    useChatComposerStore.getState().setSendShortcut("enter");

    expect(useChatComposerStore.getState().sendShortcut).toBe("enter");
  });

  it("updates the configured link open gesture", () => {
    useChatComposerStore.getState().setLinkOpenGesture("click");

    expect(useChatComposerStore.getState().linkOpenGesture).toBe("click");
  });
});
