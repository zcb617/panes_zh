import { reactive } from "vue";
import type { EngineInfo, Workspace } from "../types";
import { panesConnectionManager } from "./panes-connection";

const itemsByPanesId = reactive<Record<string, Workspace[]>>({});
const enginesByPanesId = reactive<Record<string, EngineInfo[]>>({});
const loadingByPanesId = reactive<Record<string, boolean>>({});

export const workspaceStore = {
  itemsByPanesId,
  enginesByPanesId,
  loadingByPanesId,
  getItems(panesId: string) {
    return itemsByPanesId[panesId] || [];
  },
  getEngines(panesId: string) {
    return enginesByPanesId[panesId] || [];
  },
  async load(panesId: string, force = false) {
    if (!force && itemsByPanesId[panesId]) return itemsByPanesId[panesId];
    loadingByPanesId[panesId] = true;
    try {
      // 保留原有并行请求实现，便于追溯此次真机故障的根因。
      /*
      const [workspaces, engines] = await Promise.all([
        panesConnectionManager.request<Workspace[]>(panesId, "workspace.list"),
        panesConnectionManager.request<EngineInfo[]>(panesId, "engine.list"),
      ]);
      itemsByPanesId[panesId] = workspaces;
      enginesByPanesId[panesId] = engines;
      return workspaces;
      */
      const workspaces = await panesConnectionManager.request<Workspace[]>(panesId, "workspace.list");
      itemsByPanesId[panesId] = workspaces;
      try {
        const engines = await panesConnectionManager.request<EngineInfo[]>(panesId, "engine.list");
        enginesByPanesId[panesId] = engines;
      } catch (error) {
        // 项目列表与引擎列表互不依赖；后者失败时保留已经成功加载的项目。
        console.warn("加载引擎列表失败", error);
      }
      return workspaces;
    } finally {
      loadingByPanesId[panesId] = false;
    }
  },
  async loadEngines(panesId: string) {
    if (enginesByPanesId[panesId]) return enginesByPanesId[panesId];
    const engines = await panesConnectionManager.request<EngineInfo[]>(panesId, "engine.list");
    enginesByPanesId[panesId] = engines;
    return engines;
  },
  clear(panesId: string) {
    delete itemsByPanesId[panesId];
    delete enginesByPanesId[panesId];
    delete loadingByPanesId[panesId];
  },
};
