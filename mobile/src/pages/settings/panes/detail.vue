<script setup lang="ts">
import { computed, ref } from "vue";
import { onLoad, onShow } from "@dcloudio/uni-app";
import { conversationStore } from "../../../stores/conversation";
import { panesConnectionManager } from "../../../stores/panes-connection";
import { panesDeviceStore } from "../../../stores/panes-device";
import { projectStore } from "../../../stores/project";
import { workspaceStore } from "../../../stores/workspace";

const panesId = ref("");
const name = ref("");
const device = computed(() => panesDeviceStore.getDevice(panesId.value));
const state = computed(() => panesConnectionManager.getState(panesId.value));

function saveName() {
  if (!device.value) return;
  panesDeviceStore.rename(panesId.value, name.value);
  name.value = panesDeviceStore.getDevice(panesId.value)?.name || name.value;
  uni.showToast({ title: '名称已保存', icon: 'success' });
}

function removeDevice() {
  if (!device.value) return;
  uni.showModal({ title: '解除绑定', content: `将只删除“${device.value.name}”及其本地缓存，其他 Panes 不会受影响。`, confirmText: '解除绑定', confirmColor: '#f2776e', success: (result) => {
    if (!result.confirm) return;
    panesConnectionManager.remove(panesId.value);
    workspaceStore.clear(panesId.value);
    projectStore.clear(panesId.value);
    conversationStore.clear(panesId.value);
    panesDeviceStore.remove(panesId.value);
    uni.navigateBack();
  } });
}

function openRepairPage() {
  uni.navigateTo({ url: `/pages/settings/panes/add?panesId=${encodeURIComponent(panesId.value)}` });
}

onLoad((query) => {
  panesId.value = String((query || {}).panesId || "");
  if (!panesDeviceStore.getDevice(panesId.value)) {
    uni.showToast({ title: 'Panes 不存在', icon: 'none' });
    uni.navigateBack();
    return;
  }
  name.value = panesDeviceStore.getDevice(panesId.value)?.name || "";
});

onShow(() => { if (panesId.value) name.value = panesDeviceStore.getDevice(panesId.value)?.name || name.value; });
</script>

<template>
  <scroll-view class="full-scroll" scroll-y>
    <view v-if="device" class="content-page">
      <view class="section-heading first"><view><text>设备信息</text><text>{{ device.name }}</text></view><text :class="state.peerOnline ? 'online-text' : ''">{{ state.peerOnline ? '在线' : '离线' }}</text></view>
      <view class="form-card"><text>显示名称</text><input v-model="name" class="name-input" maxlength="40"/><button class="mini-button" @tap="saveName">保存</button></view>
      <view class="settings-group"><view><text>设备标识</text><text>{{ device.tunnelId }}</text></view><view><text>Relay 地址</text><text>{{ device.endpoint }}</text></view><view><text>最后连接</text><text>{{ device.lastConnectedAt || '尚未连接' }}</text></view><view><text>当前状态</text><text>{{ state.peerOnline ? '桌面在线' : state.relayConnected ? '等待桌面上线' : '正在连接 Relay' }}</text></view></view>
      <button class="secondary-button action-button" @tap="panesDeviceStore.setActive(panesId)">设为当前 Panes</button>
      <button class="secondary-button action-button" @tap="panesConnectionManager.reconnect(panesId)">重新连接</button>
      <button class="secondary-button action-button" @tap="openRepairPage">重新配对</button>
      <button class="danger-button" @tap="removeDevice">解除绑定</button>
    </view>
  </scroll-view>
</template>

<style scoped>
.form-card { display: grid; margin-bottom: 14px; padding: 13px; grid-template-columns: 70px minmax(0, 1fr) 48px; align-items: center; gap: 8px; border: 1px solid var(--line); border-radius: 14px; background: var(--surface); font-size: 11px; }.name-input { width: 100%; height: 34px; padding: 0 8px; border-radius: 8px; color: var(--text); background: rgba(255,255,255,.05); font-size: 12px; }.action-button { margin-top: 12px; }.online-text { color: var(--accent); }
</style>
