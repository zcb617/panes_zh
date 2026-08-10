<script setup lang="ts">
import { computed, nextTick, ref } from "vue";
import { onHide, onLoad, onShow, onUnload } from "@dcloudio/uni-app";
// marked 16 会把 Unicode 属性正则打进 app-service.js，部分 App 运行时无法解析并导致启动白屏。
// import { marked } from "marked";
import { RemoteClient } from "../../remote";
import type { ChatAttachment, ConnectionState, DesktopStatus, Message, MessageWindow, PairingConfig, RemoteEvent, Thread, Workspace } from "../../types";

declare const plus: any;

type Screen = "pair" | "projects" | "threads" | "chat" | "settings";
const STORAGE_KEY = "panes-mobile:pairing:v1";
const client = new RemoteClient();
const pairing = ref<PairingConfig | null>(null);
const connection = ref<ConnectionState>({ relayConnected: false, peerOnline: false, lastError: null });
const desktop = ref<DesktopStatus | null>(null);
const screen = ref<Screen>("pair");
const priorScreen = ref<Screen>("projects");
const workspaces = ref<Workspace[]>([]);
const threads = ref<Thread[]>([]);
const messages = ref<Message[]>([]);
const selectedWorkspaceId = ref<string | null>(null);
const selectedThreadId = ref<string | null>(null);
const nextCursor = ref<MessageWindow["nextCursor"]>(null);
const loading = ref(false);
const loadingOlder = ref(false);
const sending = ref(false);
const creatingThread = ref(false);
// const recognizingSpeech = ref(false);
const draft = ref("");
const composerInputHeight = ref(50);
const attachments = ref<ChatAttachment[]>([]);
const pairingText = ref("");
const pairingError = ref<string | null>(null);
const notice = ref<string | null>(null);
const chatScrollTop = ref(0);
let noticeTimer: ReturnType<typeof setTimeout> | null = null;

const selectedWorkspace = computed(() => workspaces.value.find((item) => item.id === selectedWorkspaceId.value) ?? null);
const selectedThread = computed(() => threads.value.find((item) => item.id === selectedThreadId.value) ?? null);
const activeTurn = computed(() => selectedThread.value?.status === "streaming");
const attachmentUploading = computed(() => attachments.value.some((item) => item.uploading));
const hasComposerContent = computed(() => Boolean(draft.value.trim()) || attachments.value.length > 0);
const runtimeLabel = computed(() => {
  const thread = selectedThread.value;
  if (!thread) return "GPT-5.4 高";
  const effort = typeof thread.engineMetadata?.reasoningEffort === "string"
    ? thread.engineMetadata.reasoningEffort
    : "";
  const effortLabel = effort === "xhigh" ? "极高" : effort === "high" ? "高" : effort === "medium" ? "中" : effort === "low" ? "低" : "";
  return `${thread.modelId}${effortLabel ? ` ${effortLabel}` : ""}`;
});
const accessLabel = computed(() => {
  const metadata = selectedThread.value?.engineMetadata;
  return metadata?.sandboxMode === "danger-full-access" || metadata?.approvalPolicy === "never"
    ? "完全访问权限"
    : "标准访问权限";
});
const pageTitle = computed(() => {
  if (screen.value === "projects") return "项目";
  if (screen.value === "threads") return selectedWorkspace.value?.name || "项目会话";
  if (screen.value === "chat") return selectedThread.value?.title || "会话";
  return "连接设置";
});
const connectionLabel = computed(() => connection.value.peerOnline ? "桌面在线" : connection.value.relayConnected ? "等待桌面上线" : "正在重连");
const renderedMessages = computed(() => messages.value.map((message) => {
  let content = message.content || "";
  if (!content && Array.isArray(message.blocks)) {
    content = message.blocks.map((block) => {
      if (typeof block.content === "string") return block.content;
      if (typeof block.summary === "string") return `> ${block.summary}`;
      if (typeof block.message === "string") return block.message;
      return "";
    }).filter(Boolean).join("\n\n");
  }
  // const escaped = (content || (message.status === "streaming" ? "正在生成…" : "")).replace(/&/g, "&amp;").replace(/</g, "&lt;");
  // const html = String(marked.parse(escaped, { async: false, breaks: true, gfm: true }))
  //   .replace(/\s(?:href|src)=(['"])\s*(?:javascript|data|vbscript):.*?\1/gi, "")
  //   .replace(/\son[a-z]+=(['"]).*?\1/gi, "");
  const text = content || (message.status === "streaming" ? "正在生成…" : "");
  return { ...message, text };
}));

function showNotice(message: string) {
  notice.value = message.replace(/^Error:\s*/, "");
  if (noticeTimer !== null) clearTimeout(noticeTimer);
  noticeTimer = setTimeout(() => { notice.value = null; }, 4200);
}

async function loadWorkspaces() {
  if (!connection.value.peerOnline) return;
  loading.value = true;
  try { workspaces.value = await client.request<Workspace[]>("workspace.list"); }
  catch (error) { showNotice(String(error)); }
  finally { loading.value = false; }
}

async function loadThreads(workspaceId: string) {
  if (!connection.value.peerOnline) return;
  loading.value = true;
  try { threads.value = await client.request<Thread[]>("thread.list", { workspace_id: workspaceId }); }
  catch (error) { showNotice(String(error)); }
  finally { loading.value = false; }
}

async function loadMessages(threadId: string) {
  if (!connection.value.peerOnline) return;
  loading.value = true;
  try {
    const result = await client.request<MessageWindow>("message.list", { thread_id: threadId, limit: 50 });
    messages.value = result.messages;
    nextCursor.value = result.nextCursor;
    await client.request("thread.subscribe", { thread_id: threadId });
    await nextTick();
    chatScrollTop.value += 100000;
  } catch (error) { showNotice(String(error)); }
  finally { loading.value = false; }
}

client.onState = (state) => {
  const cameOnline = !connection.value.peerOnline && state.peerOnline;
  connection.value = state;
  if (!state.peerOnline) desktop.value = null;
  if (!cameOnline) return;
  void client.request<DesktopStatus>("desktop.get_status").then((status) => {
    desktop.value = status;
    if (screen.value === "projects" || workspaces.value.length === 0) void loadWorkspaces();
    if (screen.value === "threads" && selectedWorkspaceId.value) void loadThreads(selectedWorkspaceId.value);
    if (screen.value === "chat" && selectedThreadId.value) void loadMessages(selectedThreadId.value);
  }).catch((error) => { connection.value = { ...connection.value, lastError: String(error) }; });
};

client.onEvent = (event: RemoteEvent) => {
  if (event.event !== "thread.snapshot") return;
  const updatedThread = event.payload.thread as Thread | null;
  const window = event.payload.messages as MessageWindow | null;
  if (updatedThread) {
    const index = threads.value.findIndex((item) => item.id === updatedThread.id);
    if (index >= 0) threads.value.splice(index, 1, updatedThread);
  }
  if (updatedThread?.id === selectedThreadId.value && window?.messages) {
    messages.value = window.messages;
    nextCursor.value = window.nextCursor;
    void nextTick(() => { chatScrollTop.value += 100000; });
  }
};

client.onPaired = (config) => {
  pairing.value = config;
  uni.setStorageSync(STORAGE_KEY, config);
  showNotice("手机与桌面 Panes 配对成功");
};

function acceptPairing(raw: string) {
  pairingError.value = null;
  try {
    const parsed = JSON.parse(raw.trim()) as Partial<PairingConfig>;
    if (parsed.version !== 1 || typeof parsed.endpoint !== "string" || typeof parsed.tunnel_id !== "string"
      || typeof parsed.relay_credential !== "string" || typeof parsed.pairing_token !== "string") {
      throw new Error("二维码不是有效的 Panes Mobile 配对信息");
    }
    const endpoint = parsed.endpoint.trim();
    const endpointParts = /^(wss?):\/\/(\[[^\]]+\]|[^/:?#]+)(?::\d+)?(?:[/?#]|$)/i.exec(endpoint);
    if (!endpointParts) throw new Error("配对地址不是有效的 WebSocket 地址");
    const protocol = endpointParts[1].toLowerCase();
    const hostname = endpointParts[2].toLowerCase();
    const local = protocol === "ws" && ["127.0.0.1", "localhost", "[::1]"].includes(hostname);
    if (protocol !== "wss" && !local) throw new Error("正式配对地址必须使用 WSS 加密连接");
    if (!parsed.tunnel_id || parsed.relay_credential.length < 32 || parsed.pairing_token.length < 32) throw new Error("配对凭据不完整，请刷新二维码");
    if (parsed.expires_at && new Date(parsed.expires_at).getTime() <= Date.now()) throw new Error("配对二维码已经过期，请刷新后重试");
    const config: PairingConfig = {
      version: 1,
      endpoint,
      tunnel_id: parsed.tunnel_id,
      relay_credential: parsed.relay_credential,
      pairing_token: parsed.pairing_token,
      expires_at: parsed.expires_at,
    };
    pairing.value = config;
    uni.setStorageSync(STORAGE_KEY, config);
    pairingText.value = "";
    screen.value = "projects";
    client.connect(config);
  } catch (error) { pairingError.value = error instanceof Error ? error.message : String(error); }
}

function startScanner() {
  pairingError.value = null;
  uni.scanCode({
    onlyFromCamera: true,
    scanType: ["qrCode"],
    success: (result) => acceptPairing(result.result),
    fail: (error) => {
      if (!error.errMsg?.includes("cancel")) pairingError.value = `无法扫描二维码：${error.errMsg || "请检查相机权限"}`;
    },
  });
}

async function selectWorkspace(workspace: Workspace) {
  selectedWorkspaceId.value = workspace.id;
  selectedThreadId.value = null;
  threads.value = [];
  screen.value = "threads";
  await loadThreads(workspace.id);
}

async function createNewThread() {
  if (!selectedWorkspaceId.value || creatingThread.value || !connection.value.peerOnline) return;
  creatingThread.value = true;
  try {
    const template = threads.value[0] ?? null;
    const metadata = template?.engineMetadata;
    const reasoningEffort = typeof metadata?.reasoningEffort === "string"
      ? metadata.reasoningEffort
      : template ? undefined : "high";
    const serviceTier = typeof metadata?.serviceTier === "string" ? metadata.serviceTier : undefined;
    const created = await client.request<Thread>("thread.create", {
      workspace_id: selectedWorkspaceId.value,
      engine_id: template?.engineId || "codex",
      model_id: template?.modelId || "gpt-5.4",
      reasoning_effort: reasoningEffort,
      service_tier: serviceTier,
    });
    threads.value = [created, ...threads.value.filter((item) => item.id !== created.id)];
    await selectThread(created);
  } catch (error) { showNotice(String(error)); }
  finally { creatingThread.value = false; }
}

async function selectThread(thread: Thread) {
  selectedThreadId.value = thread.id;
  messages.value = [];
  screen.value = "chat";
  await loadMessages(thread.id);
}

async function loadOlderMessages() {
  if (!selectedThreadId.value || !nextCursor.value || loadingOlder.value) return;
  loadingOlder.value = true;
  try {
    const result = await client.request<MessageWindow>("message.list", { thread_id: selectedThreadId.value, cursor: nextCursor.value, limit: 50 });
    const known = new Set(messages.value.map((message) => message.id));
    messages.value = [...result.messages.filter((message) => !known.has(message.id)), ...messages.value];
    nextCursor.value = result.nextCursor;
  } catch (error) { showNotice(String(error)); }
  finally { loadingOlder.value = false; }
}

function resizeComposerInput(event: { detail?: { height?: number } }) {
  const measuredHeight = Number(event.detail?.height);
  if (!Number.isFinite(measuredHeight)) return;
  const nextHeight = Math.max(50, Math.min(154, Math.ceil(measuredHeight)));
  const heightGrowth = Math.max(0, nextHeight - composerInputHeight.value);
  composerInputHeight.value = nextHeight;
  if (heightGrowth > 0) void nextTick(() => { chatScrollTop.value += heightGrowth; });
}

function chooseAttachments() {
  if (!connection.value.peerOnline || attachmentUploading.value || attachments.value.length >= 6) return;
  uni.chooseImage({
    count: Math.max(1, 6 - attachments.value.length),
    sizeType: ["original"],
    sourceType: ["album", "camera"],
    success: async (result) => {
      const paths = (result.tempFilePaths || []).map((item) => String(item));
      if (paths.length === 0) return;
      for (let pathIndex = 0; pathIndex < paths.length; pathIndex += 1) {
        const filePath = paths[pathIndex];
        const localId = `mobile-${Date.now()}-${pathIndex}-${Math.random().toString(16).slice(2)}`;
        const placeholder: ChatAttachment = {
          id: localId,
          fileName: `图片-${attachments.value.length + 1}`,
          filePath: "",
          sizeBytes: 0,
          mimeType: "image/jpeg",
          uploading: true,
        };
        attachments.value.push(placeholder);
        try {
          if (typeof plus === "undefined" || !plus.io) throw new Error("当前运行环境无法读取附件");
          const localFile = await new Promise<{ dataUrl: string; fileName: string; sizeBytes: number }>((resolve, reject) => {
            plus.io.resolveLocalFileSystemURL(filePath, (entry: any) => {
              entry.file((file: any) => {
                if (Number(file.size) > 10 * 1024 * 1024) {
                  reject(new Error("单个附件不能超过 10 MB"));
                  return;
                }
                const reader = new plus.io.FileReader();
                reader.onloadend = () => resolve({
                  dataUrl: String(reader.result || ""),
                  fileName: String(file.name || entry.name || `图片-${pathIndex + 1}.jpg`),
                  sizeBytes: Number(file.size) || 0,
                });
                reader.onerror = () => reject(new Error("读取附件失败"));
                reader.readAsDataURL(file);
              }, () => reject(new Error("读取附件信息失败")));
            }, () => reject(new Error("无法打开所选附件")));
          });
          const dataMatch = /^data:([^;,]+);base64,/i.exec(localFile.dataUrl);
          const commaIndex = localFile.dataUrl.indexOf(",");
          if (!dataMatch || commaIndex < 0) throw new Error("附件数据格式无效");
          const mimeType = dataMatch[1].toLowerCase();
          const dataBase64 = localFile.dataUrl.slice(commaIndex + 1);
          const chunkSize = 256 * 1024;
          const chunkCount = Math.ceil(dataBase64.length / chunkSize);
          const uploadId = `${localId}-${Date.now()}`;
          let uploaded: ChatAttachment | null = null;
          for (let chunkIndex = 0; chunkIndex < chunkCount; chunkIndex += 1) {
            uploaded = await client.request<ChatAttachment>("attachment.upload", {
              upload_id: uploadId,
              file_name: localFile.fileName,
              mime_type: mimeType,
              chunk_index: chunkIndex,
              chunk_count: chunkCount,
              data_base64: dataBase64.slice(chunkIndex * chunkSize, (chunkIndex + 1) * chunkSize),
            });
          }
          if (!uploaded?.filePath) throw new Error("附件上传没有返回文件路径");
          const attachmentIndex = attachments.value.findIndex((item) => item.id === localId);
          if (attachmentIndex >= 0) {
            attachments.value.splice(attachmentIndex, 1, {
              ...uploaded,
              id: localId,
              fileName: localFile.fileName,
              sizeBytes: localFile.sizeBytes || uploaded.sizeBytes,
              uploading: false,
            });
          }
        } catch (error) {
          const attachmentIndex = attachments.value.findIndex((item) => item.id === localId);
          if (attachmentIndex >= 0) attachments.value.splice(attachmentIndex, 1);
          showNotice(String(error));
        }
      }
    },
    fail: (error) => {
      const failureMessage = String(error.errMsg || "").trim();
      const normalizedFailure = failureMessage.toLowerCase();
      const userCancelled = normalizedFailure.includes("cancel")
        || normalizedFailure.includes("canceled")
        || normalizedFailure.includes("cancelled")
        || normalizedFailure.includes("no image selected")
        || normalizedFailure.includes("no images selected")
        || normalizedFailure.includes("no media selected")
        || normalizedFailure.includes("no photo")
        || normalizedFailure.includes("did not select")
        || failureMessage.includes("未选择")
        || failureMessage.includes("没有选择")
        || failureMessage.includes("没有选")
        || failureMessage.includes("没选")
        || failureMessage.includes("取消");
      if (!userCancelled) showNotice(`无法选择附件：${failureMessage || "请检查相册权限"}`);
    },
  });
}

async function sendMessage() {
  const text = draft.value.trim();
  if (!selectedThreadId.value || sending.value || activeTurn.value || attachmentUploading.value) return;
  const selectedAttachments = attachments.value.slice();
  if (!text && selectedAttachments.length === 0) return;
  const previousInputHeight = composerInputHeight.value;
  draft.value = "";
  composerInputHeight.value = 50;
  attachments.value = [];
  sending.value = true;
  try {
    await client.request("message.send", {
      thread_id: selectedThreadId.value,
      message: text,
      attachments: selectedAttachments.map((item) => ({
        fileName: item.fileName,
        filePath: item.filePath,
        sizeBytes: item.sizeBytes,
        mimeType: item.mimeType,
      })),
    });
    chatScrollTop.value += 100000;
  } catch (error) {
    draft.value = text;
    composerInputHeight.value = previousInputHeight;
    attachments.value = selectedAttachments;
    showNotice(String(error));
  }
  finally { sending.value = false; }
}

function stopTurn() {
  if (selectedThreadId.value) void client.request("turn.stop", { thread_id: selectedThreadId.value }).catch((error) => showNotice(String(error)));
}

function goBack() {
  if (screen.value === "chat") {
    if (selectedThreadId.value && connection.value.peerOnline) void client.request("thread.unsubscribe", { thread_id: selectedThreadId.value }).catch(() => undefined);
    selectedThreadId.value = null;
    screen.value = "threads";
  } else if (screen.value === "threads") {
    selectedWorkspaceId.value = null;
    screen.value = "projects";
  } else if (screen.value === "settings") screen.value = priorScreen.value;
}

function openSettings() { priorScreen.value = screen.value; screen.value = "settings"; }
function removePairing() {
  uni.showModal({ title: "解除手机绑定", content: "将清除这台手机保存的桌面连接凭据。", confirmText: "解除绑定", confirmColor: "#f2776e", success: (result) => {
    if (!result.confirm) return;
    client.disconnect();
    uni.removeStorageSync(STORAGE_KEY);
    pairing.value = null;
    desktop.value = null;
    workspaces.value = [];
    threads.value = [];
    messages.value = [];
    screen.value = "pair";
  } });
}

function formatTime(value: string) {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return `${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")} ${String(date.getHours()).padStart(2, "0")}:${String(date.getMinutes()).padStart(2, "0")}`;
}

onLoad(() => {
  const saved = uni.getStorageSync(STORAGE_KEY) as PairingConfig | string | null;
  if (!saved) return;
  try {
    const config = (typeof saved === "string" ? JSON.parse(saved) : saved) as PairingConfig;
    if (config.version !== 1 || !config.endpoint || !config.tunnel_id || !config.relay_credential || (!config.device_credential && !config.pairing_token)) throw new Error("invalid pairing");
    pairing.value = config;
    screen.value = "projects";
    client.connect(config);
  } catch { uni.removeStorageSync(STORAGE_KEY); }
});

onShow(() => {
  client.resume();
});

onHide(() => {
  client.suspend();
});

onUnload(() => {
  client.disconnect();
  if (noticeTimer !== null) clearTimeout(noticeTimer);
});
</script>

<template>
  <view class="mobile-shell" :class="{ pairing: screen === 'pair' }">
    <view v-if="screen !== 'pair'" class="app-header">
      <button v-if="screen !== 'projects'" class="header-action" @tap="goBack">返回</button><view v-else class="brand-mark">P</view>
      <view class="header-title"><text>{{ pageTitle }}</text><text :class="{ online: connection.peerOnline }">{{ connectionLabel }}</text></view>
      <button class="header-action right" @tap="openSettings">设置</button>
    </view>

    <scroll-view v-if="screen === 'pair'" class="full-scroll" scroll-y><view class="pair-page">
      <view class="pair-brand"><view class="pair-logo">P</view><text class="pair-name">PANES</text><text class="pair-title">把桌面项目装进口袋</text><text class="pair-copy">扫描桌面 Panes 的配对二维码，随时查看项目、继续会话并接收实时输出。</text></view>
      <button class="primary-button" @tap="startScanner">扫描配对二维码</button>
      <view class="divider"><view/><text>或粘贴配对内容</text><view/></view>
      <textarea v-model="pairingText" class="pair-input" :maxlength="-1" placeholder="粘贴桌面 Panes 复制的 JSON 配对内容"/>
      <button class="secondary-button" :disabled="!pairingText.trim()" @tap="acceptPairing(pairingText)">连接桌面 Panes</button>
      <text v-if="pairingError" class="form-error">{{ pairingError }}</text><text class="security">公网通信使用 WSS/TLS 加密</text>
    </view></scroll-view>

    <scroll-view v-else-if="screen === 'projects'" class="content-scroll" scroll-y><view class="content-page">
      <view class="status-card" :class="{ online: connection.peerOnline }"><view class="status-dot"/><view class="status-copy"><text>{{ connection.peerOnline ? '桌面 Panes 已在线' : connection.relayConnected ? '等待桌面 Panes 上线' : '正在连接公网服务' }}</text><text>{{ desktop ? `桌面版本 v${desktop.version}` : connection.lastError || '连接恢复后会自动刷新项目' }}</text></view><button class="mini-button refresh-button" aria-label="刷新项目" :disabled="!connection.peerOnline || loading" @tap="loadWorkspaces"><!-- <text class="refresh-icon">↻</text> --><!-- <view class="toolbar-icon toolbar-icon-refresh"/> --><uni-icons class="official-toolbar-icon" type="refreshempty" :size="20" color="#8d97a7"/></button></view>
      <view class="section-heading"><view><text>工作区</text><text>我的项目</text></view><text>{{ workspaces.length }}</text></view>
      <view v-if="loading && !workspaces.length" class="empty-state"><view class="loader"/><text>正在加载项目…</text></view>
      <view v-else-if="!connection.peerOnline" class="empty-state"><text>—</text><text>桌面离线，当前不能读取项目</text></view>
      <view v-else-if="!workspaces.length" class="empty-state"><text>0</text><text>桌面 Panes 中还没有工作区</text></view>
      <view v-else class="card-list"><view v-for="workspace in workspaces" :key="workspace.id" class="nav-card" @tap="selectWorkspace(workspace)"><view class="card-icon">项</view><view class="card-copy"><text>{{ workspace.name || '未命名项目' }}</text><text>{{ workspace.rootPath }}</text><text>最近打开 {{ formatTime(workspace.lastOpenedAt) }}</text></view><text class="arrow">›</text></view></view>
    </view></scroll-view>

    <scroll-view v-else-if="screen === 'threads'" class="content-scroll" scroll-y><view class="content-page">
      <view class="section-heading first"><view><text>项目会话</text><text>{{ selectedWorkspace?.name }}</text></view><view class="thread-heading-actions"><button class="mini-button create-thread-button" aria-label="新建会话" :disabled="creatingThread || loading || !connection.peerOnline" @tap="createNewThread"><!-- <text class="thread-add-icon">＋</text> --><!-- <view class="thread-add-glyph"><view/><view/></view> --><!-- <view class="toolbar-icon toolbar-icon-add"/> --><!-- <text class="refresh-icon thread-add-standard-icon">＋</text> --><uni-icons class="official-toolbar-icon" type="plusempty" :size="20" color="#8d97a7"/></button><button class="mini-button refresh-button thread-refresh-button" aria-label="刷新会话" :disabled="loading || !selectedWorkspaceId" @tap="selectedWorkspaceId && loadThreads(selectedWorkspaceId)"><!-- <text class="thread-refresh-icon">↻</text> --><!-- <view class="thread-refresh-glyph"><view class="thread-refresh-ring"/><view class="thread-refresh-arrow"/></view> --><!-- <text class="refresh-icon thread-refresh-standard-icon">↻</text> --><!-- <view class="toolbar-icon toolbar-icon-refresh"/> --><uni-icons class="official-toolbar-icon" type="refreshempty" :size="20" color="#8d97a7"/></button></view></view>
      <view v-if="loading && !threads.length" class="empty-state"><view class="loader"/><text>正在加载会话…</text></view>
      <view v-else-if="!threads.length" class="empty-state"><text>0</text><text>此项目还没有会话</text></view>
      <view v-else class="card-list"><view v-for="thread in threads" :key="thread.id" class="nav-card" @tap="selectThread(thread)"><view class="card-icon">话</view><view class="card-copy"><text>{{ thread.title || '新会话' }}</text><text>{{ thread.engineId }} · {{ thread.modelId }}</text><text>{{ thread.messageCount }} 条消息 · {{ formatTime(thread.lastActivityAt) }}</text></view><text class="thread-status" :class="thread.status">{{ thread.status === 'streaming' ? '运行中' : thread.status === 'error' ? '出错' : '空闲' }}</text></view></view>
    </view></scroll-view>

    <view v-else-if="screen === 'chat'" class="chat-page"><scroll-view class="chat-scroll" scroll-y scroll-with-animation :scroll-top="chatScrollTop"><view class="chat-content">
      <button v-if="nextCursor" class="load-older" :disabled="loadingOlder" @tap="loadOlderMessages">{{ loadingOlder ? '正在加载…' : '加载更早消息' }}</button>
      <view v-for="message in renderedMessages" :key="message.id" class="message" :class="message.role"><text class="message-role">{{ message.role === 'user' ? '你' : 'Panes' }}</text><text class="markdown" selectable>{{ message.text }}</text><text v-if="message.status === 'streaming'" class="streaming">正在生成</text></view>
    </view></scroll-view><view class="composer">
      <view class="composer-meta"><text class="composer-chip">{{ runtimeLabel }}</text><text class="composer-chip">{{ accessLabel }}</text></view>
      <scroll-view v-if="attachments.length" class="composer-attachments" scroll-x><view class="composer-attachment-track"><view v-for="(attachment, index) in attachments" :key="attachment.id" class="composer-attachment"><view class="attachment-thumb">图</view><view class="attachment-copy"><text>{{ attachment.fileName }}</text><text>{{ attachment.uploading ? '正在上传…' : `${Math.max(1, Math.ceil(attachment.sizeBytes / 1024))} KB` }}</text></view><button class="attachment-remove" aria-label="移除附件" :disabled="attachment.uploading" @tap.stop="attachments.splice(index, 1)">×</button></view></view></scroll-view>
      <view class="composer-row"><button class="attachment-button" aria-label="选择附件" :disabled="!connection.peerOnline || attachmentUploading || attachments.length >= 6" @tap="chooseAttachments">＋</button><view class="composer-field"><textarea v-model="draft" class="composer-input" auto-height :style="{ height: `${composerInputHeight}px` }" :disabled="!connection.peerOnline || activeTurn" :maxlength="-1" confirm-type="send" :placeholder="connection.peerOnline ? activeTurn ? '当前回复尚未完成' : selectedWorkspace ? `在 ${selectedWorkspace.name} 上工作` : '给 Panes 发消息…' : '桌面离线，不能发送消息'" @linechange="resizeComposerInput" @confirm="sendMessage"/><button class="composer-action" :class="{ ready: hasComposerContent, stop: activeTurn }" :disabled="sending || !connection.peerOnline || attachmentUploading" @tap="activeTurn ? stopTurn() : sendMessage()"><text v-if="activeTurn" class="stop-icon">■</text><text v-else-if="hasComposerContent" class="send-arrow">↑</text><view v-else class="waveform-icon"><text/><text/><text/><text/></view></button></view></view>
    </view></view>

    <scroll-view v-else class="content-scroll" scroll-y><view class="content-page"><view class="settings-hero"><view class="pair-logo small">P</view><view><text>这台桌面 Panes</text><text>{{ pairing?.tunnel_id }}</text></view></view><view class="settings-group"><view><text>公网入口</text><text>{{ pairing?.endpoint }}</text></view><view><text>Relay 连接</text><text>{{ connection.relayConnected ? '已连接' : '重连中' }}</text></view><view><text>桌面状态</text><text>{{ connection.peerOnline ? '在线' : '离线' }}</text></view></view><button class="danger-button" @tap="removePairing">解除绑定并清除凭据</button></view></scroll-view>
    <view v-if="notice" class="toast"><text>{{ notice }}</text></view>
  </view>
</template>
