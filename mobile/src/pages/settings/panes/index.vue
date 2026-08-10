<script setup lang="ts">
import { panesConnectionManager } from "../../../stores/panes-connection";
import { panesDeviceStore } from "../../../stores/panes-device";

const devices = panesDeviceStore.devices;
const activePanesId = panesDeviceStore.activePanesId;
const stateByPanesId = panesConnectionManager.stateByPanesId;

function openDetail(panesId: string) {
  uni.navigateTo({ url: `/pages/settings/panes/detail?panesId=${encodeURIComponent(panesId)}` });
}

function openAddPage() {
  uni.navigateTo({ url: "/pages/settings/panes/add" });
}
</script>

<template>
  <scroll-view class="full-scroll" scroll-y>
    <view class="content-page">
      <view class="section-heading first"><view><text>设备管理</text><text>我的 Panes</text></view><text>{{ devices.length }}</text></view>
      <view v-if="!devices.length" class="empty-state"><text>尚未添加 Panes</text><text>扫码或粘贴配对内容即可添加桌面设备。</text></view>
      <view v-else class="card-list"><button v-for="device in devices" :key="device.panesId" class="nav-card" @tap="openDetail(device.panesId)"><view class="card-icon">P</view><view class="card-copy"><text>{{ device.name }}<text v-if="device.panesId === activePanesId" class="active-label">当前</text></text><text>{{ device.tunnelId }}</text><text :class="stateByPanesId[device.panesId]?.peerOnline ? 'online-text' : ''">{{ stateByPanesId[device.panesId]?.peerOnline ? '在线' : '离线' }}</text></view><text class="arrow">›</text></button></view>
      <button class="primary-button add-button" @tap="openAddPage">添加 Panes</button>
    </view>
  </scroll-view>
</template>

<style scoped>
.add-button { margin-top: 20px; }.active-label { margin-left: 6px; padding: 2px 5px; border-radius: 5px; color: var(--accent); background: var(--soft); font-size: 8px; }.online-text { color: var(--accent) !important; }
</style>
