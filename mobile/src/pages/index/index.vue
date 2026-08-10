<script setup lang="ts">
import { computed } from "vue";
import { onLoad, onShow, onUnload } from "@dcloudio/uni-app";
import { panesConnectionManager } from "../../stores/panes-connection";
import { panesDeviceStore } from "../../stores/panes-device";
import { conversationStore } from "../../stores/conversation";
import { workspaceStore } from "../../stores/workspace";

const devices = panesDeviceStore.devices;
const activePanesId = panesDeviceStore.activePanesId;
const activeDevice = panesDeviceStore.activeDevice;
const stateByPanesId = panesConnectionManager.stateByPanesId;
const workspaces = computed(() => activePanesId.value ? workspaceStore.getItems(activePanesId.value) : []);
const loading = computed(() => activePanesId.value ? Boolean(workspaceStore.loadingByPanesId[activePanesId.value]) : false);
const unreadTotal = computed(() => devices.value.reduce((total, device) => total + conversationStore.getUnreadTotal(device.panesId), 0));
const unreadDeviceTotal = computed(() => devices.value.filter((device) => conversationStore.getUnreadTotal(device.panesId) > 0).length);
let unsubscribeState: (() => void) | undefined;

function unreadForPanes(panesId: string) {
  return conversationStore.getUnreadTotal(panesId);
}

async function selectPanes(panesId: string) {
  panesDeviceStore.setActive(panesId);
  const state = panesConnectionManager.getState(panesId);
  if (!state.relayConnected) panesConnectionManager.connect(panesId);
  if (state.peerOnline) await workspaceStore.load(panesId, true);
}

async function refreshProjects() {
  if (!activePanesId.value) return;
  const state = panesConnectionManager.getState(activePanesId.value);
  if (!state.peerOnline) {
    panesConnectionManager.reconnect(activePanesId.value);
    return;
  }
  await workspaceStore.load(activePanesId.value, true);
}

function openProject(workspaceId: string) {
  if (!activePanesId.value) return;
  uni.navigateTo({
    url: `/pages/project/index?panesId=${encodeURIComponent(activePanesId.value)}&workspaceId=${encodeURIComponent(workspaceId)}`,
  });
}

function openSettings() {
  uni.navigateTo({ url: "/pages/settings/index" });
}

function openPanesSettings() {
  uni.navigateTo({ url: "/pages/settings/panes/index" });
}

onLoad(() => {
  unsubscribeState = panesConnectionManager.subscribeState((panesId, state, previous) => {
    if (panesId === activePanesId.value && !previous.peerOnline && state.peerOnline) {
      void workspaceStore.load(panesId, true);
    }
  });
});

onShow(() => {
  if (activePanesId.value && panesConnectionManager.getState(activePanesId.value).peerOnline) {
    void workspaceStore.load(activePanesId.value);
  }
});

onUnload(() => {
  unsubscribeState?.();
  unsubscribeState = undefined;
});
</script>

<template>
  <view class="mobile-shell home-shell">
    <view class="app-header">
      <view class="brand-mark">P</view>
      <view class="header-title">
        <text>我的 Panes</text>
        <text :class="panesConnectionManager.getState(activePanesId || '').peerOnline ? 'online' : ''">
          {{ activeDevice ? (panesConnectionManager.getState(activeDevice.panesId).peerOnline ? '当前 Panes 在线' : '当前 Panes 离线') : '尚未添加 Panes' }}
        </text>
      </view>
      <button class="header-action right" @tap="openSettings">设置</button>
    </view>

    <scroll-view class="content-scroll" scroll-y>
      <view class="content-page">
        <view v-if="devices.length" class="device-strip">
          <button
            v-for="device in devices"
            :key="device.panesId"
            class="device-card"
            :class="{ active: device.panesId === activePanesId }"
            @tap="selectPanes(device.panesId)"
          >
            <text class="device-mark">P</text>
            <text class="device-name">{{ device.name }}</text>
            <text class="device-code">{{ device.tunnelId.slice(0, 8) }}</text>
            <text :class="stateByPanesId[device.panesId]?.peerOnline ? 'device-status online' : 'device-status'">
              {{ stateByPanesId[device.panesId]?.peerOnline ? '在线' : '离线' }}
            </text>
            <text v-if="unreadForPanes(device.panesId)" class="device-unread">{{ unreadForPanes(device.panesId) > 99 ? '99+' : unreadForPanes(device.panesId) }}</text>
          </button>
        </view>

        <view v-if="!devices.length" class="empty-state no-device">
          <text class="empty-logo">P</text>
          <text>尚未添加 Panes</text>
          <text>添加桌面 Panes 后，即可查看项目与会话。</text>
          <button class="primary-button compact-button" @tap="openPanesSettings">前往设置</button>
        </view>

        <template v-else>
          <view class="section-heading">
            <view><text>当前 PANES 的项目</text><text>{{ activeDevice?.name || '项目' }}</text></view>
            <!-- 重构初版改成了文字按钮；保留该写法以便追溯本次视觉回归。 -->
            <!-- <button class="mini-button" :disabled="loading" @tap="refreshProjects">刷新</button> -->
            <button class="mini-button refresh-button" aria-label="刷新项目" :disabled="loading" @tap="refreshProjects"><uni-icons class="official-toolbar-icon" type="refreshempty" :size="20" color="#8d97a7"/></button>
          </view>
          <view v-if="unreadTotal" class="home-unread-banner"><text class="home-unread-dot">!</text><view><text>所有 Panes 共 {{ unreadTotal }} 条会话新消息</text><text>涉及 {{ unreadDeviceTotal }} 台 Panes；进入对应会话后会同步完整历史并清除提示。</text></view></view>
          <view v-if="loading && !workspaces.length" class="empty-state"><view class="loader"/><text>正在加载项目…</text></view>
          <view v-else-if="!panesConnectionManager.getState(activePanesId || '').peerOnline" class="empty-state">
            <text>当前 Panes 离线</text>
            <text>连接恢复后会保留已加载的项目列表。</text>
            <button class="secondary-button compact-button" @tap="refreshProjects">重新连接</button>
          </view>
          <view v-else-if="!workspaces.length" class="empty-state"><text>0</text><text>桌面 Panes 中还没有项目</text></view>
          <view v-else class="card-list">
            <button v-for="workspace in workspaces" :key="workspace.id" class="nav-card" @tap="openProject(workspace.id)">
              <view class="card-icon">项</view>
              <view class="card-copy"><text>{{ workspace.name || '未命名项目' }}</text><text>{{ workspace.rootPath }}</text><text>最近打开 {{ workspace.lastOpenedAt }}</text></view>
              <text class="arrow">›</text>
            </button>
          </view>
        </template>
      </view>
    </scroll-view>
  </view>
</template>

<style scoped>
.home-shell { height: 100vh; }
.device-strip { display: flex; padding: 4px 0 18px; gap: 10px; overflow-x: auto; white-space: nowrap; }
.device-card { display: flex; width: 88px; min-width: 88px; min-height: 108px; padding: 10px 8px; flex-direction: column; align-items: center; border: 1px solid var(--line); border-radius: 14px; background: var(--surface); }
.device-card.active { border-color: var(--accent); background: var(--soft); box-shadow: inset 0 0 0 1px rgba(70, 211, 154, .12); }
.device-mark { display: flex; width: 32px; height: 32px; align-items: center; justify-content: center; border-radius: 9px; color: var(--accent); background: rgba(70, 211, 154, .14); font-size: 15px; font-weight: 800; }
.device-name { width: 100%; margin-top: 8px; overflow: hidden; font-size: 11px; font-weight: 700; text-overflow: ellipsis; white-space: nowrap; }.device-code { width: 100%; margin-top: 3px; overflow: hidden; color: #687383; font-family: monospace; font-size: 8px; text-overflow: ellipsis; white-space: nowrap; }
.device-status { margin-top: 5px; color: var(--muted); font-size: 9px; }.device-status.online { color: var(--accent); }
.device-unread { display: flex; min-width: 22px; height: 18px; margin-top: 5px; padding: 0 5px; align-items: center; justify-content: center; border-radius: 9px; color: #fff; background: #e85d6a; font-size: 9px; font-weight: 800; }
.no-device { min-height: 420px; }.empty-logo { display: flex; width: 62px; height: 62px; align-items: center; justify-content: center; border: 1px solid rgba(70, 211, 154, .3); border-radius: 18px; color: var(--accent); background: var(--soft); font-size: 25px; font-weight: 800; }
.compact-button { width: 180px; min-height: 42px; margin-top: 12px; }
.home-unread-banner { display: flex; margin: 0 0 14px; padding: 11px 13px; align-items: center; gap: 10px; border: 1px solid rgba(232, 93, 106, .3); border-radius: 13px; color: var(--text); background: rgba(232, 93, 106, .1); }
.home-unread-banner view { display: flex; min-width: 0; flex-direction: column; gap: 3px; }
.home-unread-banner view text:first-child { font-size: 12px; font-weight: 700; }
.home-unread-banner view text:last-child { color: var(--muted); font-size: 10px; }
.home-unread-dot { display: flex; width: 22px; height: 22px; flex: none; align-items: center; justify-content: center; border-radius: 50%; color: #fff; background: #e85d6a; font-size: 12px; font-weight: 800; }
/* 初版图标按钮的局部尺寸规则已由 refresh-button 的既有页面规则取代，保留以便追溯。 */
/* .section-heading .icon-action { display: flex; width: 34px; min-width: 34px; height: 34px; padding: 0; align-items: center; justify-content: center; } */
</style>
