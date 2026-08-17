import { create } from "zustand";
import type {
  ChatProviderUsage,
  EngineHealth,
  EngineInfo,
  EngineRuntimeUpdatedEvent,
  ExecutionTarget,
} from "../types";
import { ipc } from "../lib/ipc";

interface EngineState {
  target: ExecutionTarget | null;
  engines: EngineInfo[];
  enginesByTarget: Record<string, EngineInfo[]>;
  health: Record<string, EngineHealth>;
  healthByTarget: Record<string, Record<string, EngineHealth>>;
  usage: Record<string, ChatProviderUsage>;
  usageByTarget: Record<string, Record<string, ChatProviderUsage>>;
  healthLoading: Record<string, boolean>;
  healthLoadingByTarget: Record<string, Record<string, boolean>>;
  engineCatalogLoading: Record<string, boolean>;
  engineCatalogLoadingByTarget: Record<string, Record<string, boolean>>;
  usageLoading: Record<string, boolean>;
  usageLoadingByTarget: Record<string, Record<string, boolean>>;
  targetGenerations: Record<string, number>;
  loading: boolean;
  loadedOnce: boolean;
  activeWorkspaceId: string | null;
  error?: string;
  load: (workspaceId?: string | null) => Promise<void>;
  refreshEngineCatalog: (engineId: string) => Promise<EngineInfo | null>;
  ensureHealth: (
    engineId: string,
    options?: { force?: boolean },
  ) => Promise<EngineHealth | null>;
  ensureUsage: (
    engineId: string,
    options?: { force?: boolean },
  ) => Promise<ChatProviderUsage | null>;
  invalidateConnection: (connectionId: string) => void;
  mergeHealth: (reports: EngineHealth[]) => void;
  applyRuntimeUpdate: (event: EngineRuntimeUpdatedEvent) => void;
}

let pendingHealthRequests: Partial<Record<string, Promise<EngineHealth | null>>> = {};
let pendingEngineCatalogRequests: Partial<Record<string, Promise<EngineInfo | null>>> = {};
let pendingUsageRequests: Partial<Record<string, Promise<ChatProviderUsage | null>>> = {};
let engineLoadSequence = 0;

// 阶段计划 5 前使用 workspaceId 作为请求键；现在统一使用后端返回的稳定 targetKey。
// function targetRequestKey(workspaceId: string | null, engineId: string): string {
//   return `${workspaceId ?? "local"}:${engineId}`;
// }
function targetRequestKey(targetKey: string, engineId: string): string {
  return `${targetKey}:${engineId}`;
}

export const useEngineStore = create<EngineState>((set, get) => ({
  target: null,
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
  load: async (workspaceId = null) => {
    const normalizedWorkspaceId = workspaceId ?? null;
    const sequence = ++engineLoadSequence;
    const previousTarget = get().target;
    set({
      target: null,
      engines: [],
      health: {},
      usage: {},
      healthLoading: {},
      engineCatalogLoading: {},
      usageLoading: {},
      loading: true,
      activeWorkspaceId: normalizedWorkspaceId,
      error: undefined,
    });
    try {
      const target = await ipc.getExecutionTarget(normalizedWorkspaceId);
      const targetIdentityChanged =
        previousTarget?.targetKey === target.targetKey &&
        (previousTarget.connectionStatus !== target.connectionStatus ||
          previousTarget.hostName !== target.hostName ||
          previousTarget.port !== target.port ||
          previousTarget.user !== target.user);
      if (targetIdentityChanged && target.connectionId) {
        get().invalidateConnection(target.connectionId);
      }

      const targetGeneration = get().targetGenerations[target.targetKey] ?? 0;
      const cachedEngines = get().enginesByTarget[target.targetKey];
      if (
        sequence === engineLoadSequence &&
        get().activeWorkspaceId === normalizedWorkspaceId
      ) {
        set((state) => ({
          target,
          engines: cachedEngines ?? [],
          health: state.healthByTarget[target.targetKey] ?? {},
          usage: state.usageByTarget[target.targetKey] ?? {},
          usageLoading: state.usageLoadingByTarget[target.targetKey] ?? {},
          healthLoading: state.healthLoadingByTarget[target.targetKey] ?? {},
          engineCatalogLoading:
            state.engineCatalogLoadingByTarget[target.targetKey] ?? {},
          loading: cachedEngines === undefined,
          loadedOnce: cachedEngines !== undefined || state.loadedOnce,
          error: undefined,
        }));
      }
      if (cachedEngines !== undefined) {
        return;
      }

      const engines = normalizedWorkspaceId
        ? await ipc.listEngines(normalizedWorkspaceId)
        : await ipc.listEngines();
      set((state) => {
        if ((state.targetGenerations[target.targetKey] ?? 0) !== targetGeneration) {
          return state;
        }
        const enginesByTarget = {
          ...state.enginesByTarget,
          [target.targetKey]: engines,
        };
        if (
          sequence !== engineLoadSequence ||
          state.activeWorkspaceId !== normalizedWorkspaceId ||
          state.target?.targetKey !== target.targetKey
        ) {
          return { enginesByTarget };
        }
        return {
          engines,
          enginesByTarget,
          health: state.healthByTarget[target.targetKey] ?? {},
          healthLoading: state.healthLoadingByTarget[target.targetKey] ?? {},
          engineCatalogLoading:
            state.engineCatalogLoadingByTarget[target.targetKey] ?? {},
          loading: false,
          loadedOnce: true,
          error: undefined,
        };
      });
    } catch (error) {
      if (
        sequence !== engineLoadSequence ||
        get().activeWorkspaceId !== normalizedWorkspaceId
      ) {
        return;
      }
      const message = String(error);
      set({
        loading: false,
        loadedOnce: true,
        error: message,
        health: {
          codex: {
            id: "codex",
            available: false,
            details: `Engine discovery failed: ${message}`,
            warnings: [],
            checks: [],
            fixes: [],
          },
        },
      });
    }
  },
  refreshEngineCatalog: async (engineId) => {
    const workspaceId = get().activeWorkspaceId;
    const targetKey = get().target?.targetKey ?? "local";
    const targetGeneration = get().targetGenerations[targetKey] ?? 0;
    const requestKey = targetRequestKey(targetKey, engineId);
    const pendingRequest = pendingEngineCatalogRequests[requestKey];
    if (pendingRequest) {
      return pendingRequest;
    }

    set((state) => {
      const loadingForTarget = {
        ...(state.engineCatalogLoadingByTarget[targetKey] ?? {}),
        [engineId]: true,
      };
      return {
        engineCatalogLoadingByTarget: {
          ...state.engineCatalogLoadingByTarget,
          [targetKey]: loadingForTarget,
        },
        engineCatalogLoading:
          state.target?.targetKey === targetKey
            ? loadingForTarget
            : state.engineCatalogLoading,
      };
    });

    const request = (async () => {
      try {
        const engine = workspaceId
          ? await ipc.getEngineInfo(engineId, workspaceId)
          : await ipc.getEngineInfo(engineId);
        set((state) => {
          if ((state.targetGenerations[targetKey] ?? 0) !== targetGeneration) {
            return state;
          }
          const targetEngines = state.enginesByTarget[targetKey] ?? [];
          const engineIndex = targetEngines.findIndex((item) => item.id === engine.id);
          const engines = engineIndex >= 0
            ? targetEngines.map((item) => (item.id === engine.id ? engine : item))
            : [...targetEngines, engine];
          const targetHealth = state.healthByTarget[targetKey] ?? {};
          const health = {
            ...targetHealth,
            [engine.id]: {
              id: engine.id,
              available: true,
              details: targetHealth[engine.id]?.details,
              warnings: targetHealth[engine.id]?.warnings ?? [],
              checks: targetHealth[engine.id]?.checks ?? [],
              fixes: targetHealth[engine.id]?.fixes ?? [],
              protocolDiagnostics: targetHealth[engine.id]?.protocolDiagnostics,
            },
          };
          const activeTarget = state.target?.targetKey === targetKey;
          return {
            enginesByTarget: {
              ...state.enginesByTarget,
              [targetKey]: engines,
            },
            healthByTarget: {
              ...state.healthByTarget,
              [targetKey]: health,
            },
            engines: activeTarget ? engines : state.engines,
            health: activeTarget ? health : state.health,
            error:
              activeTarget && state.error?.startsWith(`${engineId}:`)
                ? undefined
                : state.error,
          };
        });
        return engine;
      } catch (error) {
        const message = String(error);
        set((state) => {
          if ((state.targetGenerations[targetKey] ?? 0) !== targetGeneration) {
            return state;
          }
          const targetHealth = {
            ...(state.healthByTarget[targetKey] ?? {}),
            [engineId]: {
              id: engineId,
              available: false,
              details: message,
              warnings: [],
              checks: [],
              fixes: [],
            },
          };
          const activeTarget = state.target?.targetKey === targetKey;
          return {
            healthByTarget: {
              ...state.healthByTarget,
              [targetKey]: targetHealth,
            },
            error: activeTarget ? `${engineId}: ${message}` : state.error,
            health: activeTarget ? targetHealth : state.health,
          };
        });
        return null;
      } finally {
        if ((get().targetGenerations[targetKey] ?? 0) === targetGeneration) {
          delete pendingEngineCatalogRequests[requestKey];
        }
        set((state) => {
          if ((state.targetGenerations[targetKey] ?? 0) !== targetGeneration) {
            return state;
          }
          const { [engineId]: _ignored, ...rest } =
            state.engineCatalogLoadingByTarget[targetKey] ?? {};
          return {
            engineCatalogLoadingByTarget: {
              ...state.engineCatalogLoadingByTarget,
              [targetKey]: rest,
            },
            engineCatalogLoading:
              state.target?.targetKey === targetKey
                ? rest
                : state.engineCatalogLoading,
          };
        });
      }
    })();

    pendingEngineCatalogRequests[requestKey] = request;
    return request;
  },
  ensureHealth: async (engineId, options) => {
    const workspaceId = get().activeWorkspaceId;
    const targetKey = get().target?.targetKey ?? "local";
    const targetGeneration = get().targetGenerations[targetKey] ?? 0;
    const requestKey = targetRequestKey(targetKey, engineId);
    const existing = get().healthByTarget[targetKey]?.[engineId];
    if (existing && !options?.force) {
      return existing;
    }

    if (pendingHealthRequests[requestKey]) {
      return pendingHealthRequests[requestKey];
    }

    set((state) => {
      const loadingForTarget = {
        ...(state.healthLoadingByTarget[targetKey] ?? {}),
        [engineId]: true,
      };
      return {
        healthLoadingByTarget: {
          ...state.healthLoadingByTarget,
          [targetKey]: loadingForTarget,
        },
        healthLoading:
          state.target?.targetKey === targetKey
            ? loadingForTarget
            : state.healthLoading,
      };
    });

    const request = (async () => {
      try {
        const health = workspaceId
          ? await ipc.engineHealth(engineId, workspaceId)
          : await ipc.engineHealth(engineId);
        set((state) => {
          if ((state.targetGenerations[targetKey] ?? 0) !== targetGeneration) {
            return state;
          }
          const targetHealth = {
            ...(state.healthByTarget[targetKey] ?? {}),
            [health.id]: health,
          };
          const { [engineId]: _ignored, ...rest } =
            state.healthLoadingByTarget[targetKey] ?? {};
          const activeTarget = state.target?.targetKey === targetKey;
          return {
            healthByTarget: {
              ...state.healthByTarget,
              [targetKey]: targetHealth,
            },
            healthLoadingByTarget: {
              ...state.healthLoadingByTarget,
              [targetKey]: rest,
            },
            health: activeTarget ? targetHealth : state.health,
            healthLoading: activeTarget ? rest : state.healthLoading,
          };
        });
        const engine = get().enginesByTarget[targetKey]?.find(
          (item) => item.id === engineId,
        );
        if (
          health.available &&
          engine &&
          engine.models.length === 0 &&
          get().target?.targetKey === targetKey
        ) {
          await get().refreshEngineCatalog(engineId);
        }
        return health;
      } catch (error) {
        const message = String(error);
        set((state) => {
          if ((state.targetGenerations[targetKey] ?? 0) !== targetGeneration) {
            return state;
          }
          const { [engineId]: _ignored, ...rest } =
            state.healthLoadingByTarget[targetKey] ?? {};
          const activeTarget = state.target?.targetKey === targetKey;
          return {
            healthLoadingByTarget: {
              ...state.healthLoadingByTarget,
              [targetKey]: rest,
            },
            healthLoading: activeTarget ? rest : state.healthLoading,
            error: activeTarget ? `${engineId}: ${message}` : state.error,
          };
        });
        return null;
      } finally {
        if ((get().targetGenerations[targetKey] ?? 0) === targetGeneration) {
          delete pendingHealthRequests[requestKey];
        }
      }
    })();

    pendingHealthRequests[requestKey] = request;
    return request;
  },
  ensureUsage: async (engineId, options) => {
    const workspaceId = get().activeWorkspaceId;
    const targetKey = get().target?.targetKey ?? "local";
    const targetGeneration = get().targetGenerations[targetKey] ?? 0;
    const requestKey = targetRequestKey(targetKey, engineId);
    const existing = get().usageByTarget[targetKey]?.[engineId];
    if (existing && !options?.force) {
      return existing;
    }
    if (pendingUsageRequests[requestKey]) {
      return pendingUsageRequests[requestKey];
    }

    set((state) => {
      const loadingForTarget = {
        ...(state.usageLoadingByTarget[targetKey] ?? {}),
        [engineId]: true,
      };
      return {
        usageLoadingByTarget: {
          ...state.usageLoadingByTarget,
          [targetKey]: loadingForTarget,
        },
        usageLoading:
          state.target?.targetKey === targetKey
            ? loadingForTarget
            : state.usageLoading,
      };
    });
    const request = (async () => {
      try {
        const providers = await ipc.getChatProviderUsage(workspaceId, engineId);
        const provider = providers.find((item) => item.engineId === engineId) ?? null;
        if (provider) {
          set((state) => {
            if ((state.targetGenerations[targetKey] ?? 0) !== targetGeneration) {
              return state;
            }
            const targetUsage = {
              ...(state.usageByTarget[targetKey] ?? {}),
              [engineId]: provider,
            };
            return {
              usageByTarget: {
                ...state.usageByTarget,
                [targetKey]: targetUsage,
              },
              usage:
                state.target?.targetKey === targetKey
                  ? targetUsage
                  : state.usage,
            };
          });
        }
        return provider;
      } catch (error) {
        if (
          get().target?.targetKey === targetKey &&
          (get().targetGenerations[targetKey] ?? 0) === targetGeneration
        ) {
          set({ error: `${engineId}: ${String(error)}` });
        }
        return null;
      } finally {
        if ((get().targetGenerations[targetKey] ?? 0) === targetGeneration) {
          delete pendingUsageRequests[requestKey];
        }
        set((state) => {
          if ((state.targetGenerations[targetKey] ?? 0) !== targetGeneration) {
            return state;
          }
          const { [engineId]: _ignored, ...rest } =
            state.usageLoadingByTarget[targetKey] ?? {};
          return {
            usageLoadingByTarget: {
              ...state.usageLoadingByTarget,
              [targetKey]: rest,
            },
            usageLoading:
              state.target?.targetKey === targetKey
                ? rest
                : state.usageLoading,
          };
        });
      }
    })();
    pendingUsageRequests[requestKey] = request;
    return request;
  },
  invalidateConnection: (connectionId) => {
    const targetKey = `ssh:${connectionId}`;
    for (const requestKey of Object.keys(pendingHealthRequests)) {
      if (requestKey.startsWith(`${targetKey}:`)) {
        delete pendingHealthRequests[requestKey];
      }
    }
    for (const requestKey of Object.keys(pendingEngineCatalogRequests)) {
      if (requestKey.startsWith(`${targetKey}:`)) {
        delete pendingEngineCatalogRequests[requestKey];
      }
    }
    for (const requestKey of Object.keys(pendingUsageRequests)) {
      if (requestKey.startsWith(`${targetKey}:`)) {
        delete pendingUsageRequests[requestKey];
      }
    }
    set((state) => {
      const { [targetKey]: _engines, ...enginesByTarget } = state.enginesByTarget;
      const { [targetKey]: _health, ...healthByTarget } = state.healthByTarget;
      const { [targetKey]: _usage, ...usageByTarget } = state.usageByTarget;
      const { [targetKey]: _healthLoading, ...healthLoadingByTarget } =
        state.healthLoadingByTarget;
      const { [targetKey]: _catalogLoading, ...engineCatalogLoadingByTarget } =
        state.engineCatalogLoadingByTarget;
      const { [targetKey]: _usageLoading, ...usageLoadingByTarget } =
        state.usageLoadingByTarget;
      const active = state.target?.targetKey === targetKey;
      return {
        targetGenerations: {
          ...state.targetGenerations,
          [targetKey]: (state.targetGenerations[targetKey] ?? 0) + 1,
        },
        enginesByTarget,
        healthByTarget,
        usageByTarget,
        healthLoadingByTarget,
        engineCatalogLoadingByTarget,
        usageLoadingByTarget,
        engines: active ? [] : state.engines,
        health: active ? {} : state.health,
        usage: active ? {} : state.usage,
        healthLoading: active ? {} : state.healthLoading,
        engineCatalogLoading: active ? {} : state.engineCatalogLoading,
        usageLoading: active ? {} : state.usageLoading,
      };
    });
  },
  mergeHealth: (reports) =>
    set((state) => {
      if (reports.length === 0) {
        return state;
      }

      const targetKey = state.target?.targetKey ?? "local";
      const nextHealth = { ...(state.healthByTarget[targetKey] ?? {}) };
      const nextHealthLoading = {
        ...(state.healthLoadingByTarget[targetKey] ?? {}),
      };
      for (const report of reports) {
        nextHealth[report.id] = report;
        delete nextHealthLoading[report.id];
      }

      return {
        health: nextHealth,
        healthByTarget: {
          ...state.healthByTarget,
          [targetKey]: nextHealth,
        },
        healthLoading: nextHealthLoading,
        healthLoadingByTarget: {
          ...state.healthLoadingByTarget,
          [targetKey]: nextHealthLoading,
        },
      };
    }),
  applyRuntimeUpdate: ({ engineId, protocolDiagnostics }) =>
    set((state) => {
      // 全局 Codex runtime 事件来自本机 engine registry，不能污染当前 SSH 目标。
      const targetKey = "local";
      const localHealth = state.healthByTarget[targetKey] ?? {};
      const current = localHealth[engineId];
      const nextHealth: EngineHealth = current
        ? {
            ...current,
            available: true,
            details: current.available ? current.details : undefined,
            protocolDiagnostics: protocolDiagnostics ?? current.protocolDiagnostics,
          }
        : {
            id: engineId,
            available: true,
            warnings: [],
            checks: [],
            fixes: [],
            protocolDiagnostics,
          };

      const { [engineId]: _ignored, ...rest } =
        state.healthLoadingByTarget[targetKey] ?? {};
      const nextLocalHealth = {
        ...localHealth,
        [engineId]: nextHealth,
      };
      const localActive = state.target?.targetKey === targetKey;

      return {
        healthByTarget: {
          ...state.healthByTarget,
          [targetKey]: nextLocalHealth,
        },
        healthLoadingByTarget: {
          ...state.healthLoadingByTarget,
          [targetKey]: rest,
        },
        health: localActive ? nextLocalHealth : state.health,
        healthLoading: localActive ? rest : state.healthLoading,
      };
    }),
}));
