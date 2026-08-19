import { beforeEach, describe, expect, it, vi } from "vitest";

const mockIpc = vi.hoisted(() => ({
  getExecutionTarget: vi.fn(),
  listActivedClis: vi.fn(),
  getEngineInfo: vi.fn(),
  getChatProviderUsage: vi.fn(),
  engineHealth: vi.fn(),
}));

vi.mock("../lib/ipc", () => ({
  ipc: mockIpc,
}));

import { useEngineStore } from "./engineStore";

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((nextResolve) => {
    resolve = nextResolve;
  });
  return { promise, resolve };
}

describe("engineStore", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockIpc.getExecutionTarget.mockImplementation((workspaceId?: string | null) =>
      Promise.resolve(
        workspaceId === "workspace-ssh"
          ? {
              targetKey: "ssh:connection-a",
              kind: "ssh",
              displayName: "开发机 A",
              connectionId: "connection-a",
              hostName: "192.168.1.12",
              user: "tester",
              port: 22,
              projectPath: "/var/work/project",
              connectionStatus: "ok",
            }
          : {
              targetKey: "local",
              kind: "local",
              displayName: "本机",
              connectionStatus: "ok",
            },
      ),
    );
    mockIpc.getChatProviderUsage.mockResolvedValue([]);
    useEngineStore.setState({
      target: {
        targetKey: "local",
        kind: "local",
        displayName: "本机",
      },
      engines: [],
      // 旧实现：enginesByTarget: {},
      health: {},
      // 旧实现：healthByTarget: {},
      usage: {},
      // 旧实现：usageByTarget: {},
      healthLoading: {},
      // 旧实现：healthLoadingByTarget: {},
      engineCatalogLoading: {},
      // 旧实现：engineCatalogLoadingByTarget: {},
      usageLoading: {},
      // 旧实现：usageLoadingByTarget: {},
      // 旧实现：targetGenerations: {},
      loading: false,
      loadedOnce: false,
      activeWorkspaceId: null,
      error: undefined,
    });
  });

  it("loads engines without eagerly probing health", async () => {
    mockIpc.listActivedClis.mockResolvedValue([
      {
        id: "codex",
        name: "Codex",
        models: [],
        capabilities: {
          permissionModes: [],
          sandboxModes: [],
          approvalDecisions: [],
        },
      },
    ]);

    await useEngineStore.getState().load();

    expect(mockIpc.listActivedClis).toHaveBeenCalledTimes(1);
    expect(mockIpc.engineHealth).not.toHaveBeenCalled();
    expect(useEngineStore.getState().engines).toHaveLength(1);
  });

  it("reads the active CLI list from the backend on every load", async () => {
    mockIpc.listActivedClis.mockResolvedValue([
      {
        id: "codex",
        name: "Codex",
        models: [{ id: "gpt-live" }],
        capabilities: { permissionModes: [], sandboxModes: [], approvalDecisions: [] },
      },
    ]);

    await useEngineStore.getState().load(null);
    await useEngineStore.getState().load(null);

    expect(mockIpc.listActivedClis).toHaveBeenCalledTimes(2);
  });

  it("loads an SSH workspace engine catalog with workspace context", async () => {
    mockIpc.listActivedClis.mockResolvedValue([
      {
        id: "codex",
        name: "Codex",
        models: [{ id: "remote-model" }],
        capabilities: {
          permissionModes: [],
          sandboxModes: [],
          approvalDecisions: [],
        },
      },
    ]);

    await useEngineStore.getState().load("workspace-ssh");

    expect(mockIpc.listActivedClis).toHaveBeenCalledWith("workspace-ssh");
    expect(useEngineStore.getState().activeWorkspaceId).toBe("workspace-ssh");
    expect(useEngineStore.getState().engines[0]?.models[0]?.id).toBe("remote-model");
  });

  it("discards late CLI results after switching targets", async () => {
    const firstCatalog = deferred<Array<{ id: string; name: string; models: Array<{ id: string }>; capabilities: { permissionModes: never[]; sandboxModes: never[]; approvalDecisions: never[] } }>>();
    mockIpc.getExecutionTarget.mockImplementation((workspaceId?: string | null) =>
      Promise.resolve({
        targetKey: `ssh:${workspaceId}`,
        kind: "ssh",
        displayName: String(workspaceId),
        connectionId: String(workspaceId),
        connectionStatus: "ok",
      }),
    );
    mockIpc.listActivedClis.mockImplementation((workspaceId?: string | null) => {
      if (workspaceId === "workspace-a") {
        return firstCatalog.promise;
      }
      return Promise.resolve([
        {
          id: "codex",
          name: "Codex B",
          models: [{ id: "model-b" }],
          capabilities: { permissionModes: [], sandboxModes: [], approvalDecisions: [] },
        },
      ]);
    });

    const firstLoad = useEngineStore.getState().load("workspace-a");
    await Promise.resolve();
    await useEngineStore.getState().load("workspace-b");
    firstCatalog.resolve([
      {
        id: "codex",
        name: "Codex A",
        models: [{ id: "model-a" }],
        capabilities: { permissionModes: [], sandboxModes: [], approvalDecisions: [] },
      },
    ]);
    await firstLoad;

    expect(useEngineStore.getState().target?.targetKey).toBe("ssh:workspace-b");
    expect(useEngineStore.getState().engines[0]?.models[0]?.id).toBe("model-b");
    // 旧实现会把迟到结果归档到 enginesByTarget；前端不再保存其他目标的 CLI 状态。
  });

  it("does not restore invalidated target data from an older request", async () => {
    const request = deferred<{
      id: string;
      name: string;
      models: Array<{ id: string }>;
      capabilities: { permissionModes: never[]; sandboxModes: never[]; approvalDecisions: never[] };
    }>();
    useEngineStore.setState({
      activeWorkspaceId: "workspace-ssh",
      target: {
        targetKey: "ssh:connection-a",
        kind: "ssh",
        displayName: "开发机 A",
        connectionId: "connection-a",
      },
      engines: [],
    });
    mockIpc.getEngineInfo.mockReturnValueOnce(request.promise);

    const refresh = useEngineStore.getState().refreshEngineCatalog("codex");
    useEngineStore.getState().invalidateConnection("connection-a");
    request.resolve({
      id: "codex",
      name: "Codex",
      models: [{ id: "stale-model" }],
      capabilities: { permissionModes: [], sandboxModes: [], approvalDecisions: [] },
    });
    await refresh;

    // 旧实现还会断言 enginesByTarget 已删除；该缓存结构现已不存在。
    expect(useEngineStore.getState().engines).toEqual([]);
  });

  it("loads engine health on demand", async () => {
    mockIpc.engineHealth.mockResolvedValue({
      id: "codex",
      available: true,
      details: "ready",
      warnings: [],
      checks: [],
      fixes: [],
    });

    const health = await useEngineStore.getState().ensureHealth("codex");

    expect(mockIpc.engineHealth).toHaveBeenCalledWith("codex");
    expect(health?.available).toBe(true);
    expect(useEngineStore.getState().health.codex?.details).toBe("ready");
  });

  it("reads successful CLI health from the backend on every request", async () => {
    mockIpc.engineHealth.mockResolvedValue({
      id: "codex",
      available: true,
      details: "ready",
      warnings: [],
      checks: [],
      fixes: [],
    });

    await useEngineStore.getState().ensureHealth("codex");
    await useEngineStore.getState().ensureHealth("codex");

    expect(mockIpc.engineHealth).toHaveBeenCalledTimes(2);
  });

  it("consumes a successful health result by restoring an empty model catalog", async () => {
    useEngineStore.setState({
      activeWorkspaceId: "workspace-ssh",
      target: {
        targetKey: "ssh:connection-a",
        kind: "ssh",
        displayName: "开发机 A",
        connectionId: "connection-a",
      },
      engines: [
        {
          id: "claude",
          name: "Claude",
          models: [],
          capabilities: {
            permissionModes: [],
            sandboxModes: [],
            approvalDecisions: [],
          },
        },
      ],
      /*
      旧实现还要同步设置 enginesByTarget：
      enginesByTarget: {
        "ssh:connection-a": [],
      },
      */
    });
    mockIpc.engineHealth.mockResolvedValue({
      id: "claude",
      available: true,
      details: "ready",
      warnings: [],
      checks: [],
      fixes: [],
    });
    mockIpc.getEngineInfo.mockResolvedValue({
      id: "claude",
      name: "Claude",
      models: [
        {
          id: "opus[1m]",
          displayName: "Opus (1M context)",
          description: "Remote Claude model",
          hidden: false,
          isDefault: true,
          inputModalities: ["text"],
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
    });

    const health = await useEngineStore.getState().ensureHealth("claude");

    expect(health?.available).toBe(true);
    expect(mockIpc.getEngineInfo).toHaveBeenCalledWith("claude", "workspace-ssh");
    expect(useEngineStore.getState().engines[0]?.models[0]?.id).toBe("opus[1m]");
    expect(useEngineStore.getState().engineCatalogLoading).toEqual({});
  });

  it("does not cache thrown health errors and allows retries", async () => {
    mockIpc.engineHealth
      .mockRejectedValueOnce(new Error("temporary failure"))
      .mockResolvedValueOnce({
        id: "codex",
        available: true,
        details: "ready",
        warnings: [],
        checks: [],
        fixes: [],
      });

    const first = await useEngineStore.getState().ensureHealth("codex");
    const second = await useEngineStore.getState().ensureHealth("codex");

    expect(first).toBeNull();
    expect(second?.available).toBe(true);
    expect(mockIpc.engineHealth).toHaveBeenCalledTimes(2);
    expect(useEngineStore.getState().health.codex?.details).toBe("ready");
  });

  it("marks Codex available when a runtime update arrives", () => {
    useEngineStore.setState({
      target: {
        targetKey: "local",
        kind: "local",
        displayName: "本机",
      },
      health: {
        codex: {
          id: "codex",
          available: false,
          details: "Engine discovery failed: codex missing",
          warnings: [],
          checks: ["codex --version"],
          fixes: [],
        },
      },
      /* 旧实现还要把同一状态复制到 healthByTarget.local。 */
    });

    useEngineStore.getState().applyRuntimeUpdate({
      engineId: "codex",
      protocolDiagnostics: {
        methodAvailability: [
          {
            method: "app/list",
            status: "available",
          },
        ],
        experimentalFeatures: [],
        collaborationModes: [],
        apps: [],
        skills: [],
        pluginMarketplaces: [],
        mcpServers: [],
        fetchedAt: "2026-03-06T00:00:00Z",
        stale: false,
      },
    });

    const codex = useEngineStore.getState().health.codex;
    expect(codex?.available).toBe(true);
    expect(codex?.details).toBeUndefined();
    expect(codex?.protocolDiagnostics?.fetchedAt).toBe("2026-03-06T00:00:00Z");
  });
});
