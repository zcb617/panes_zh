import { reactive } from "vue";
import type { ChatAttachment, Message, RemoteEvent } from "../types";
import { panesConnectionManager } from "./panes-connection";
// 旧的整段会话快照会同时携带项目列表信息；该快照推送已停用。
// import { projectStore } from "./project";

interface ConversationState {
  messages: Message[];
  // 旧模板仍会读取这两个字段；完整取数后始终为空和 false，不再触发分页。
  nextCursor: null;
  draft: string;
  attachments: ChatAttachment[];
  loading: boolean;
  loadingOlder: boolean;
  sending: boolean;
  messageRevision: number;
}

interface UnreadConversationState {
  // 未读完成消息数量。
  count: number;
  // 最后一条完成消息摘要。
  summary: string;
  // 最后一条完成消息状态。
  status: Message["status"];
  // 最后一条完成消息标识。
  lastMessageId: string;
  // 最后一条完成消息到达时间。
  updatedAt: string;
}

const stateByConversation = reactive<Record<string, ConversationState>>({});
const unreadMap = reactive<Record<string, UnreadConversationState>>({});
// 手机端页面实例会在返回上一页时卸载；该运行期 Map 跨实例保存会话滚动位置，不写入持久化存储。
export const conversationScrollTopMap = new Map<string, number>();
const activeConversation = reactive({
  // 当前可见会话所属的 Panes。
  panesId: "",
  // 当前可见会话 ID。
  threadId: "",
  // 当前会话是否停在底部。
  atBottom: true,
});

function conversationKey(panesId: string, threadId: string) {
  return `${panesId}:${threadId}`;
}

function createState(): ConversationState {
  return {
    messages: [],
    nextCursor: null,
    draft: "",
    attachments: [],
    loading: false,
    loadingOlder: false,
    sending: false,
    messageRevision: 0,
  };
}

/* 重构初版直接用实时快照覆盖本地历史，且首次只请求 50 条；保留原实现以便追溯。
function applyWindow(state: ConversationState, window: MessageWindow) {
  state.messages = window.messages;
  state.nextCursor = window.nextCursor;
}
*/
/*
function mergeWindow(state: ConversationState, window: MessageWindow, replaceCursor: boolean) {
  const hadMessages = state.messages.length > 0;
  const byId = new Map(state.messages.map((message) => [message.id, message]));
  window.messages.forEach((message) => byId.set(message.id, message));
  state.messages = [...byId.values()].sort((left, right) => {
    const createdAt = left.createdAt.localeCompare(right.createdAt);
    return createdAt || left.id.localeCompare(right.id);
  });
  if (replaceCursor || !hadMessages) state.nextCursor = window.nextCursor;
}
*/
/*
type MessageWindowSource = "initial" | "older" | "snapshot";

function mergeWindow(state: ConversationState, window: MessageWindow, source: MessageWindowSource) {
  if (source === "initial") {
    state.messages = window.messages.slice();
    state.nextCursor = window.nextCursor;
    return;
  }

  const incomingIds = new Set(window.messages.map((message) => message.id));
  if (source === "older") {
    // 服务端已按 created_at、rowid 返回稳定顺序；不能再按 UUID 重排。
    state.messages = [...window.messages, ...state.messages.filter((message) => !incomingIds.has(message.id))];
    state.nextCursor = window.nextCursor;
    return;
  }

  const incomingById = new Map(window.messages.map((message) => [message.id, message]));
  const knownIds = new Set(state.messages.map((message) => message.id));
  state.messages = [
    ...state.messages.map((message) => incomingById.get(message.id) || message),
    ...window.messages.filter((message) => !knownIds.has(message.id)),
  ];
  if (state.messages.length === window.messages.length) state.nextCursor = window.nextCursor;
}
*/

panesConnectionManager.subscribe((panesId, event: RemoteEvent) => {
  if (event.event !== "thread.message.completed") return;
  const payload = event.payload as { threadId?: unknown; messageId?: unknown; message?: unknown };
  const message = payload.message as Message | undefined;
  if (!message?.id || message.role !== "assistant" || typeof payload.threadId !== "string" || payload.threadId !== message.threadId) return;
  if (typeof payload.messageId === "string" && payload.messageId !== message.id) return;
  // getState 会创建占位状态，因此目标会话尚未打开时也能保留完成消息。
  const state = conversationStore.getState(panesId, message.threadId);
  const index = state.messages.findIndex((item) => item.id === message.id);
  const isNewMessage = index < 0;
  if (index >= 0) state.messages.splice(index, 1, message);
  else state.messages.push(message);
  state.messageRevision += 1;
  const active = activeConversation.panesId === panesId && activeConversation.threadId === message.threadId;
  if (!active && isNewMessage) {
    const key = conversationKey(panesId, message.threadId);
    const existing = unreadMap[key];
    const content = typeof message.content === "string" ? message.content.trim() : "";
    unreadMap[key] = {
      count: (existing?.count || 0) + 1,
      summary: content ? content.slice(0, 120) : "助手已完成一条消息",
      status: message.status,
      lastMessageId: message.id,
      updatedAt: message.createdAt || new Date().toISOString(),
    };
  }
});

export const conversationStore = {
  stateByConversation,
  unreadMap,
  activeConversation,
  getState(panesId: string, threadId: string) {
    const key = conversationKey(panesId, threadId);
    if (!stateByConversation[key]) stateByConversation[key] = createState();
    return stateByConversation[key];
  },
  setActive(panesId: string, threadId: string, atBottom = true) {
    activeConversation.panesId = panesId;
    activeConversation.threadId = threadId;
    activeConversation.atBottom = atBottom;
  },
  setActiveViewport(atBottom: boolean) {
    activeConversation.atBottom = atBottom;
  },
  clearActive(panesId: string, threadId: string) {
    if (activeConversation.panesId !== panesId || activeConversation.threadId !== threadId) return;
    activeConversation.panesId = "";
    activeConversation.threadId = "";
    activeConversation.atBottom = true;
  },
  getUnread(panesId: string, threadId: string) {
    return unreadMap[conversationKey(panesId, threadId)] || null;
  },
  getUnreadCount(panesId: string, threadId: string) {
    return unreadMap[conversationKey(panesId, threadId)]?.count || 0;
  },
  getUnreadTotal(panesId: string) {
    return Object.entries(unreadMap)
      .filter(([key]) => key.startsWith(`${panesId}:`))
      .reduce((total, [, item]) => total + item.count, 0);
  },
  clearUnreadAfterSync(panesId: string, threadId: string) {
    delete unreadMap[conversationKey(panesId, threadId)];
  },
  async open(panesId: string, threadId: string) {
    const state = this.getState(panesId, threadId);
    state.loading = true;
    try {
      // 旧版页面级订阅已停用；保留旧调用说明，设备级连接始终接收事件。
      // await panesConnectionManager.request(panesId, "thread.subscribe", { thread_id: threadId });
      // message.list 是进入会话后清除未读的全量同步边界。
      const result = await panesConnectionManager.request<{ messages: Message[] }>(panesId, "message.list", { thread_id: threadId });
      const incoming = Array.isArray(result.messages) ? result.messages : [];
      const incomingIds = new Set(incoming.map((message) => message.id));
      const preserved = state.messages.filter((message) => !incomingIds.has(message.id));
      // 后台已经按 created_at、rowid 返回稳定顺序；手机端必须原样显示，不能用 UUID 或其他字段二次排序。
      // preserved 只可能是本次请求期间新收到、尚未出现在返回结果中的消息，应追加在后台列表之后。
      state.messages = [...incoming, ...preserved];
      state.messageRevision += 1;
      this.clearUnreadAfterSync(panesId, threadId);
      /*
      const loadedCursors = new Set<string>();
      while (state.nextCursor) {
        const cursorKey = `${state.nextCursor.createdAt}:${state.nextCursor.rowId ?? ""}:${state.nextCursor.id}`;
        if (loadedCursors.has(cursorKey)) throw new Error("会话历史分页游标重复");
        loadedCursors.add(cursorKey);
        const olderWindow = await panesConnectionManager.request<MessageWindow>(panesId, "message.list", {
          thread_id: threadId,
          cursor: state.nextCursor,
          limit: 200,
        });
        mergeWindow(state, olderWindow, "older");
      }
      */
      return state;
    } catch (error) {
      throw error;
    } finally {
      state.loading = false;
    }
  },
  /*
  async loadOlder(panesId: string, threadId: string) {
    const state = this.getState(panesId, threadId);
    if (!state.nextCursor || state.loadingOlder) return;
    state.loadingOlder = true;
    try {
      const window = await panesConnectionManager.request<MessageWindow>(panesId, "message.list", {
        thread_id: threadId,
        cursor: state.nextCursor,
        limit: 50,
      });
      mergeWindow(state, window, "older");
    } finally {
      state.loadingOlder = false;
    }
  },
  */
  async loadOlder(_panesId: string, _threadId: string) {
    // 会话已在首次打开时完整取得；保留旧模板入口但不再发起请求。
  },
  async send(panesId: string, threadId: string, modelId: string, reasoningEffort: string) {
    const state = this.getState(panesId, threadId);
    if (state.sending || state.attachments.some((item) => item.uploading)) return;
    const message = state.draft.trim();
    const attachments = state.attachments.slice();
    if (!message && attachments.length === 0) return;
    state.draft = "";
    state.attachments = [];
    state.sending = true;
    const localMessage: Message = {
      id: `mobile-user-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 10)}`,
      threadId,
      role: "user",
      content: message,
      status: "completed",
      createdAt: new Date().toISOString(),
      localOnly: true,
    };
    // 用户消息必须先本地回显，不能等待桌面端完成事件。
    state.messages.push(localMessage);
    state.messageRevision += 1;
    try {
      const result = await panesConnectionManager.request<{ message?: Message }>(panesId, "message.send", {
        thread_id: threadId,
        message,
        model_id: modelId || undefined,
        reasoning_effort: reasoningEffort || undefined,
        attachments: attachments.map((item) => ({
          fileName: item.fileName,
          filePath: item.filePath,
          sizeBytes: item.sizeBytes,
          mimeType: item.mimeType,
        })),
      });
      if (result?.message?.id) {
        const index = state.messages.findIndex((item) => item.id === localMessage.id);
        if (index >= 0) state.messages.splice(index, 1, { ...result.message, localOnly: false });
        state.messageRevision += 1;
      }
    } catch (error) {
      const index = state.messages.findIndex((item) => item.id === localMessage.id);
      if (index >= 0) state.messages.splice(index, 1);
      state.messageRevision += 1;
      state.draft = message;
      state.attachments = attachments;
      throw error;
    } finally {
      state.sending = false;
    }
  },
  async close(panesId: string, threadId: string) {
    this.clearActive(panesId, threadId);
    // 旧版 thread.unsubscribe 已停用；设备级接收通道不随页面离开而取消注册。
    // await panesConnectionManager.request(panesId, "thread.unsubscribe", { thread_id: threadId });
  },
  clear(panesId: string) {
    Object.keys(stateByConversation)
      .filter((key) => key.startsWith(`${panesId}:`))
      .forEach((key) => delete stateByConversation[key]);
    Object.keys(unreadMap)
      .filter((key) => key.startsWith(`${panesId}:`))
      .forEach((key) => delete unreadMap[key]);
    // 清理 Panes 会话缓存时同步丢弃该 Panes 的滚动位置，避免重新配对后恢复旧会话位置。
    [...conversationScrollTopMap.keys()]
      .filter((key) => key.startsWith(`${panesId}:`))
      .forEach((key) => conversationScrollTopMap.delete(key));
    if (activeConversation.panesId === panesId) {
      activeConversation.panesId = "";
      activeConversation.threadId = "";
      activeConversation.atBottom = true;
    }
  },
};
