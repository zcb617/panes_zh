import { reactive } from "vue";
import type { Thread } from "../types";
import { panesConnectionManager } from "./panes-connection";

const threadsByProject = reactive<Record<string, Thread[]>>({});
const loadingByProject = reactive<Record<string, boolean>>({});

// 会话状态变化通过同一条设备级连接进入，已加载项目中的线程保持最新标题和状态。
panesConnectionManager.subscribe((panesId, event) => {
  if (event.event !== "thread.updated") return;
  const thread = event.payload.thread as Thread | undefined;
  if (!thread?.id || !thread.workspaceId) return;
  const key = projectKey(panesId, thread.workspaceId);
  const threads = threadsByProject[key];
  if (!threads) return;
  const index = threads.findIndex((item) => item.id === thread.id);
  if (index >= 0) threads.splice(index, 1, thread);
  else threads.unshift(thread);
});

function projectKey(panesId: string, workspaceId: string) {
  return `${panesId}:${workspaceId}`;
}

export const projectStore = {
  threadsByProject,
  loadingByProject,
  getThreads(panesId: string, workspaceId: string) {
    return threadsByProject[projectKey(panesId, workspaceId)] || [];
  },
  async load(panesId: string, workspaceId: string, force = false) {
    const key = projectKey(panesId, workspaceId);
    if (!force && threadsByProject[key]) return threadsByProject[key];
    loadingByProject[key] = true;
    try {
      const threads = await panesConnectionManager.request<Thread[]>(panesId, "thread.list", { workspace_id: workspaceId });
      threadsByProject[key] = threads;
      return threads;
    } finally {
      loadingByProject[key] = false;
    }
  },
  async create(panesId: string, workspaceId: string) {
    const existing = this.getThreads(panesId, workspaceId)[0];
    const metadata = existing?.engineMetadata || {};
    const created = await panesConnectionManager.request<Thread>(panesId, "thread.create", {
      workspace_id: workspaceId,
      engine_id: existing?.engineId || "codex",
      model_id: existing?.modelId || "gpt-5.4",
      reasoning_effort: typeof metadata.reasoningEffort === "string" ? metadata.reasoningEffort : "high",
      service_tier: typeof metadata.serviceTier === "string" ? metadata.serviceTier : undefined,
    });
    const key = projectKey(panesId, workspaceId);
    const previous = threadsByProject[key] || [];
    threadsByProject[key] = [created, ...previous.filter((item) => item.id !== created.id)];
    return created;
  },
  upsert(panesId: string, thread: Thread) {
    const key = projectKey(panesId, thread.workspaceId);
    const threads = threadsByProject[key];
    if (!threads) return;
    const index = threads.findIndex((item) => item.id === thread.id);
    if (index >= 0) threads.splice(index, 1, thread);
    else threads.unshift(thread);
  },
  clear(panesId: string) {
    Object.keys(threadsByProject)
      .filter((key) => key.startsWith(`${panesId}:`))
      .forEach((key) => {
        delete threadsByProject[key];
        delete loadingByProject[key];
      });
  },
};
