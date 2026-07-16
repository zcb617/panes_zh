import { beforeEach, describe, expect, it, vi } from "vitest";
import type { EngineInfo, Thread } from "../types";

const mockIpc = vi.hoisted(() => ({
  attachCodexRemoteThread: vi.fn(),
  listCodexRemoteThreads: vi.fn(),
  listThreads: vi.fn(),
}));

const mockEngineState = vi.hoisted(() => ({
  engines: [] as EngineInfo[],
}));

vi.mock("../lib/ipc", () => ({
  ipc: mockIpc,
}));

vi.mock("./engineStore", () => ({
  useEngineStore: {
    getState: () => mockEngineState,
  },
}));

import { useThreadStore } from "./threadStore";

function makeThread(id: string): Thread {
  return {
    id,
    workspaceId: "workspace-1",
    repoId: null,
    engineId: "codex",
    modelId: "gpt-5.6",
    engineThreadId: `engine-${id}`,
    title: id,
    status: "idle",
    messageCount: 0,
    totalTokens: 0,
    createdAt: new Date(0).toISOString(),
    lastActivityAt: new Date(0).toISOString(),
  };
}

function makeCodexEngine(): EngineInfo {
  return {
    id: "codex",
    name: "Codex",
    models: [
      {
        id: "gpt-5.6",
        displayName: "GPT-5.6",
        description: "",
        hidden: false,
        isDefault: true,
        inputModalities: [],
        attachmentModalities: [],
        supportsPersonality: false,
        defaultReasoningEffort: "medium",
        supportedReasoningEfforts: [],
      },
    ],
    capabilities: {
      permissionModes: [],
      sandboxModes: [],
      approvalDecisions: [],
    },
  };
}

describe("threadStore remote Codex discovery", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.stubGlobal("localStorage", {
      getItem: vi.fn(() => null),
      setItem: vi.fn(),
      removeItem: vi.fn(),
      clear: vi.fn(),
    });
    mockEngineState.engines = [makeCodexEngine()];
    mockIpc.attachCodexRemoteThread.mockResolvedValue(makeThread("attached"));
    mockIpc.listThreads.mockResolvedValue([makeThread("local")]);
    useThreadStore.setState({
      threads: [],
      threadsByWorkspace: {},
      archivedThreadsByWorkspace: {},
      activeThreadId: null,
      loading: false,
      error: undefined,
    });
  });

  it("attaches every unlinked remote Codex thread before refreshing a workspace", async () => {
    mockIpc.listCodexRemoteThreads
      .mockResolvedValueOnce({
        threads: [
          {
            engineThreadId: "remote-1",
            title: "Remote one",
            preview: "",
            cwd: "/workspace",
            createdAt: new Date(0).toISOString(),
            updatedAt: new Date(0).toISOString(),
            modelProvider: "openai",
            sourceKind: "appServer",
            statusType: "idle",
            activeFlags: [],
            archived: false,
            localThreadId: null,
          },
          {
            engineThreadId: "already-linked",
            title: "Existing",
            preview: "",
            cwd: "/workspace",
            createdAt: new Date(0).toISOString(),
            updatedAt: new Date(0).toISOString(),
            modelProvider: "openai",
            sourceKind: "appServer",
            statusType: "idle",
            activeFlags: [],
            archived: false,
            localThreadId: "local-existing",
          },
        ],
        nextCursor: "page-2",
      })
      .mockResolvedValueOnce({
        threads: [
          {
            engineThreadId: "remote-2",
            title: "Remote two",
            preview: "",
            cwd: "/workspace",
            createdAt: new Date(0).toISOString(),
            updatedAt: new Date(0).toISOString(),
            modelProvider: "openai",
            sourceKind: "appServer",
            statusType: "idle",
            activeFlags: [],
            archived: false,
            localThreadId: null,
          },
        ],
        nextCursor: null,
      });

    await useThreadStore.getState().refreshThreads("workspace-1");

    expect(mockIpc.attachCodexRemoteThread).toHaveBeenNthCalledWith(
      1,
      "workspace-1",
      "remote-1",
      "gpt-5.6",
    );
    expect(mockIpc.attachCodexRemoteThread).toHaveBeenNthCalledWith(
      2,
      "workspace-1",
      "remote-2",
      "gpt-5.6",
    );
    expect(mockIpc.attachCodexRemoteThread).toHaveBeenCalledTimes(2);
    expect(mockIpc.listThreads).toHaveBeenCalledWith("workspace-1");
    expect(useThreadStore.getState().threads).toEqual([makeThread("local")]);
  });

  it("keeps local threads visible when remote Codex discovery fails", async () => {
    const warning = vi.spyOn(console, "warn").mockImplementation(() => {});
    mockIpc.listCodexRemoteThreads.mockRejectedValueOnce(new Error("Codex unavailable"));

    await useThreadStore.getState().refreshThreads("workspace-1");

    expect(mockIpc.listThreads).toHaveBeenCalledWith("workspace-1");
    expect(useThreadStore.getState().threads).toEqual([makeThread("local")]);
    expect(useThreadStore.getState().error).toBeUndefined();
    warning.mockRestore();
  });
});
