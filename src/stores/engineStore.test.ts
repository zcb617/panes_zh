import { beforeEach, describe, expect, it, vi } from "vitest";

const mockIpc = vi.hoisted(() => ({
  getExecutionTarget: vi.fn(),
  listEngines: vi.fn(),
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
      enginesByTarget: {},
      health: {},
      healthByTarget: {},
      usage: {},
      usageByTarget: {},
      healthLoading: {},
      healthLoadingByTarget: {},
      engineCatalogLoading: {},
      engineCatalogLoadingByTarget: {},
      usageLoading: {},
      usageLoadingByTarget: {},
      targetGenerations: {},
      loading: false,
      loadedOnce: false,
      activeWorkspaceId: null,
      error: undefined,
    });
  });

  it("loads engines without eagerly probing health", async () => {
    mockIpc.listEngines.mockResolvedValue([
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

    expect(mockIpc.listEngines).toHaveBeenCalledTimes(1);
    expect(mockIpc.engineHealth).not.toHaveBeenCalled();
    expect(useEngineStore.getState().engines).toHaveLength(1);
  });

  it("loads an SSH workspace engine catalog with workspace context", async () => {
    mockIpc.listEngines.mockResolvedValue([
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

    expect(mockIpc.listEngines).toHaveBeenCalledWith("workspace-ssh");
    expect(useEngineStore.getState().activeWorkspaceId).toBe("workspace-ssh");
    expect(useEngineStore.getState().engines[0]?.models[0]?.id).toBe("remote-model");
  });

  it("archives late engine results under their original target", async () => {
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
    mockIpc.listEngines.mockImplementation((workspaceId?: string | null) => {
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
    expect(
      useEngineStore.getState().enginesByTarget["ssh:workspace-a"]?.[0]?.models[0]?.id,
    ).toBe("model-a");
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
      enginesByTarget: { "ssh:connection-a": [] },
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

    expect(useEngineStore.getState().enginesByTarget["ssh:connection-a"]).toBeUndefined();
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
      enginesByTarget: {
        "ssh:connection-a": [
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
      },
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
      healthByTarget: {
        local: {
          codex: {
            id: "codex",
            available: false,
            details: "Engine discovery failed: codex missing",
            warnings: [],
            checks: ["codex --version"],
            fixes: [],
          },
        },
      },
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
