<script setup lang="ts">
import { computed, ref } from "vue";
import { onLoad, onShow, onUnload } from "@dcloudio/uni-app";
import { panesConnectionManager } from "../../stores/panes-connection";
import { panesDeviceStore } from "../../stores/panes-device";
import { conversationStore } from "../../stores/conversation";
import { projectStore } from "../../stores/project";
import { workspaceStore } from "../../stores/workspace";

const panesId = ref("");
const workspaceId = ref("");
const creating = ref(false);
const threads = computed(() => panesId.value && workspaceId.value ? projectStore.getThreads(panesId.value, workspaceId.value) : []);
const workspace = computed(() => workspaceStore.getItems(panesId.value).find((item) => item.id === workspaceId.value));
const loading = computed(() => panesId.value && workspaceId.value ? Boolean(projectStore.loadingByProject[`${panesId.value}:${workspaceId.value}`]) : false);
let unsubscribeState: (() => void) | undefined;

function unreadCount(threadId: string) {
  return conversationStore.getUnreadCount(panesId.value, threadId);
}

async function refresh() {
  if (!panesId.value || !workspaceId.value || !panesConnectionManager.getState(panesId.value).peerOnline) return;
  if (!workspace.value) {
    try {
      await workspaceStore.load(panesId.value);
    } catch (error) {
      // 会话列表不依赖项目名称；名称加载失败时继续请求会话。
      console.warn("加载项目名称失败", error);
    }
  }
  await projectStore.load(panesId.value, workspaceId.value, true);
}

async function createConversation() {
  if (!panesId.value || !workspaceId.value || creating.value) return;
  creating.value = true;
  try {
    const thread = await projectStore.create(panesId.value, workspaceId.value);
    uni.navigateTo({ url: `/pages/conversation/index?panesId=${encodeURIComponent(panesId.value)}&workspaceId=${encodeURIComponent(workspaceId.value)}&threadId=${encodeURIComponent(thread.id)}` });
  } catch (error) {
    uni.showToast({ title: error instanceof Error ? error.message : '无法新建会话', icon: 'none' });
  } finally {
    creating.value = false;
  }
}

function openConversation(threadId: string) {
  uni.navigateTo({ url: `/pages/conversation/index?panesId=${encodeURIComponent(panesId.value)}&workspaceId=${encodeURIComponent(workspaceId.value)}&threadId=${encodeURIComponent(threadId)}` });
}

onLoad((query) => {
  const options = query || {};
  panesId.value = String(options.panesId || "");
  workspaceId.value = String(options.workspaceId || "");
  if (!panesDeviceStore.getDevice(panesId.value) || !workspaceId.value) {
    uni.showToast({ title: '项目参数无效', icon: 'none' });
    uni.navigateBack();
    return;
  }
  unsubscribeState = panesConnectionManager.subscribeState((changedPanesId, state, previous) => {
    if (changedPanesId === panesId.value && !previous.peerOnline && state.peerOnline) void refresh();
  });
  if (!panesConnectionManager.getState(panesId.value).relayConnected) panesConnectionManager.connect(panesId.value);
  void refresh();
});

onShow(() => { void refresh(); });

onUnload(() => {
  unsubscribeState?.();
  unsubscribeState = undefined;
});
</script>

<template>
  <scroll-view class="full-scroll" scroll-y>
    <view class="content-page">
      <view class="section-heading first"><view><text>项目会话</text><!-- 重构初版用 workspaceId 作为名称回退，导致 UUID 直接显示。 --><!-- <text>{{ workspace?.name || workspaceId }}</text> --><text>{{ workspace?.name || '当前项目' }}</text></view><!-- 初版使用 project-actions，未复用既有按钮上下文。 --><!-- <view class="project-actions"> --><view class="thread-heading-actions">
        <!-- 重构初版改成了文字按钮；保留该写法以便追溯本次视觉回归。 -->
        <!-- <button class="mini-button" :disabled="loading" @tap="refresh">刷新</button><button class="mini-button" :disabled="creating" @tap="createConversation">新建</button> -->
        <button class="mini-button create-thread-button" hover-class="create-thread-button-pressed" aria-label="新建会话" :disabled="creating || !panesConnectionManager.getState(panesId).peerOnline" @tap="createConversation"><uni-icons class="official-toolbar-icon" type="plusempty" :size="20" color="#8d97a7"/></button>
        <button class="mini-button refresh-button thread-refresh-button" hover-class="thread-refresh-button-pressed" aria-label="刷新会话" :disabled="loading || !workspaceId" @tap="refresh"><uni-icons class="official-toolbar-icon" type="refreshempty" :size="20" color="#8d97a7"/></button>
      </view></view>
      <view v-if="loading && !threads.length" class="empty-state"><view class="loader"/><text>正在加载会话…</text></view>
      <view v-else-if="!panesConnectionManager.getState(panesId).peerOnline" class="empty-state"><text>当前 Panes 离线</text><text>请返回首页恢复连接。</text></view>
      <view v-else-if="!threads.length" class="empty-state"><text>0</text><text>此项目还没有会话</text><button class="secondary-button compact-button" @tap="createConversation">新建会话</button></view>
      <view v-else class="card-list"><button v-for="thread in threads" :key="thread.id" class="nav-card" @tap="openConversation(thread.id)"><view class="card-icon">话</view><view class="card-copy"><text>{{ thread.title || '新会话' }}</text><text>{{ thread.engineId }} · {{ thread.modelId }}</text><text>{{ thread.messageCount }} 条消息 · {{ thread.lastActivityAt }}</text></view><!-- 未读徽标脱离网格，固定跟随当前会话卡片。 --><text v-if="unreadCount(thread.id)" class="thread-unread">{{ unreadCount(thread.id) > 99 ? '99+' : unreadCount(thread.id) }}</text><text class="thread-status" :class="thread.status">{{ thread.status === 'streaming' ? '运行中' : thread.status === 'error' ? '出错' : '空闲' }}</text></button></view>
    </view>
  </scroll-view>
</template>

<style scoped>
/* 初版 project-actions 与 icon-action 的局部样式已由 thread-heading-actions 的完整组件上下文替代。 */
/* .project-actions { display: flex; gap: 8px; }.project-actions .icon-action { display: flex; width: 34px; min-width: 34px; height: 34px; padding: 0; align-items: center; justify-content: center; }.compact-button { width: 180px; min-height: 42px; margin-top: 12px; } */
.compact-button { width: 180px; min-height: 42px; margin-top: 12px; }
/* 每条会话卡片都是徽标的定位上下文，避免多条会话的徽标相互串位。 */
.nav-card { position: relative; }
/* 原网格子项样式保留在注释中，便于追溯未读数参与布局的旧实现。 */
/* .thread-unread { display: flex; min-width: 22px; height: 22px; margin-right: 7px; padding: 0 5px; align-items: center; justify-content: center; border-radius: 11px; color: #fff; background: #e85d6a; font-size: 10px; font-weight: 800; } */
/* 未读数作为卡片内部浮层，不占用标题、元信息和运行状态的网格空间。 */
.thread-unread { position: absolute; top: 8px; right: 8px; display: flex; min-width: 22px; height: 22px; margin-right: 0; padding: 0 5px; align-items: center; justify-content: center; border-radius: 11px; color: #fff; background: #e85d6a; font-size: 10px; font-weight: 800; }
</style>
