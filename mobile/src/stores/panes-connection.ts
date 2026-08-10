import { reactive } from "vue";
import { RemoteClient } from "../remote";
import type { ConnectionState, PairingConfig, RemoteEvent } from "../types";
import { panesDeviceStore } from "./panes-device";

type RemoteEventListener = (panesId: string, event: RemoteEvent) => void;
type ConnectionStateListener = (panesId: string, state: ConnectionState, previous: ConnectionState) => void;

const clients = new Map<string, RemoteClient>();
const stateByPanesId = reactive<Record<string, ConnectionState>>({});
const eventListeners = new Set<RemoteEventListener>();
const stateListeners = new Set<ConnectionStateListener>();
let initialized = false;

function defaultState(): ConnectionState {
  return { relayConnected: false, peerOnline: false, lastError: null };
}

function createClient(panesId: string) {
  const client = new RemoteClient();
  client.onState = (state) => {
    const previous = stateByPanesId[panesId] || defaultState();
    stateByPanesId[panesId] = state;
    if (!previous.peerOnline && state.peerOnline) panesDeviceStore.markConnected(panesId);
    stateListeners.forEach((listener) => listener(panesId, state, previous));
  };
  client.onPaired = (config) => panesDeviceStore.updatePairedCredential(panesId, config);
  client.onEvent = (event) => eventListeners.forEach((listener) => listener(panesId, event));
  clients.set(panesId, client);
  stateByPanesId[panesId] = defaultState();
  return client;
}

export const panesConnectionManager = {
  stateByPanesId,
  initialize() {
    if (initialized) return;
    initialized = true;
    panesDeviceStore.devices.value.forEach((device) => this.connect(device.panesId));
  },
  connect(panesId: string) {
    const config = panesDeviceStore.getRemoteConfig(panesId);
    if (!config) return;
    const client = clients.get(panesId) || createClient(panesId);
    client.connect(config);
  },
  reconnect(panesId: string) {
    this.disconnect(panesId);
    this.connect(panesId);
  },
  disconnect(panesId: string) {
    clients.get(panesId)?.disconnect();
  },
  remove(panesId: string) {
    clients.get(panesId)?.disconnect();
    clients.delete(panesId);
    delete stateByPanesId[panesId];
  },
  resumeAll() {
    panesDeviceStore.devices.value.forEach((device) => {
      const client = clients.get(device.panesId);
      if (client) client.resume();
      else this.connect(device.panesId);
    });
  },
  keepAliveOnHide() {
    // 保持连接，避免相册、相机和系统文件选择器打开时中断正在进行的附件上传。
  },
  getState(panesId: string) {
    return stateByPanesId[panesId] || defaultState();
  },
  request<T>(panesId: string, method: string, payload: Record<string, unknown> = {}) {
    const client = clients.get(panesId);
    if (!client) return Promise.reject(new Error("Panes 连接尚未初始化")) as Promise<T>;
    return client.request<T>(method, payload);
  },
  subscribe(listener: RemoteEventListener) {
    eventListeners.add(listener);
    return () => eventListeners.delete(listener);
  },
  subscribeState(listener: ConnectionStateListener) {
    stateListeners.add(listener);
    return () => stateListeners.delete(listener);
  },
  applyRepairedConfig(panesId: string, config: PairingConfig) {
    panesDeviceStore.addOrReplace(config, panesId);
    this.reconnect(panesId);
  },
};
