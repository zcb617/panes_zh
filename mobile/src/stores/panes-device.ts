import { computed, ref } from "vue";
import type { MobilePanesSettings, PairedPanes, PairingConfig } from "../types";

const SETTINGS_STORAGE_KEY = "panes-mobile:settings:v2";
const LEGACY_PAIRING_STORAGE_KEY = "panes-mobile:pairing:v1";
const devices = ref<PairedPanes[]>([]);
const activePanesId = ref<string | null>(null);
let initialized = false;

function saveSettings() {
  const settings: MobilePanesSettings = {
    devices: devices.value,
    activePanesId: activePanesId.value,
  };
  uni.setStorageSync(SETTINGS_STORAGE_KEY, settings);
}

function toPairedPanes(config: PairingConfig, existing?: PairedPanes): PairedPanes {
  const now = new Date().toISOString();
  return {
    panesId: existing?.panesId || config.tunnel_id,
    name: existing?.name || `Panes ${config.tunnel_id.slice(0, 8)}`,
    endpoint: config.endpoint,
    tunnelId: config.tunnel_id,
    relayCredential: config.relay_credential,
    deviceCredential: config.device_credential,
    deviceId: config.device_id || existing?.deviceId,
    pairingToken: config.pairing_token,
    expiresAt: config.expires_at,
    pairedAt: existing?.pairedAt || now,
    lastConnectedAt: existing?.lastConnectedAt,
  };
}

function isPairingConfig(value: unknown): value is PairingConfig {
  if (!value || typeof value !== "object") return false;
  const config = value as Partial<PairingConfig>;
  return config.version === 1
    && typeof config.endpoint === "string"
    && typeof config.tunnel_id === "string"
    && typeof config.relay_credential === "string"
    && (typeof config.device_credential === "string" || typeof config.pairing_token === "string");
}

function readSettings(): MobilePanesSettings | null {
  const saved = uni.getStorageSync(SETTINGS_STORAGE_KEY) as MobilePanesSettings | string | null;
  if (!saved) return null;
  try {
    const settings = typeof saved === "string" ? JSON.parse(saved) as MobilePanesSettings : saved;
    if (!Array.isArray(settings.devices)) return null;
    const validDevices = settings.devices.filter((item): item is PairedPanes => Boolean(
      item && item.panesId && item.endpoint && item.tunnelId && item.relayCredential
        && (item.deviceCredential || item.pairingToken),
    ));
    return {
      devices: validDevices,
      activePanesId: validDevices.some((item) => item.panesId === settings.activePanesId)
        ? settings.activePanesId
        : validDevices[0]?.panesId ?? null,
    };
  } catch {
    return null;
  }
}

function migrateLegacyPairing() {
  const legacy = uni.getStorageSync(LEGACY_PAIRING_STORAGE_KEY) as PairingConfig | string | null;
  if (!legacy) return;
  try {
    const config = typeof legacy === "string" ? JSON.parse(legacy) as PairingConfig : legacy;
    if (!isPairingConfig(config)) return;
    const device = toPairedPanes(config);
    devices.value = [device];
    activePanesId.value = device.panesId;
    saveSettings();
  } catch {
    // 损坏的旧配置不能阻止 uni-app 启动；其余有效配置会继续保存在 v2 数据中。
  } finally {
    uni.removeStorageSync(LEGACY_PAIRING_STORAGE_KEY);
  }
}

function toRemoteConfig(device: PairedPanes): PairingConfig {
  return {
    version: 1,
    endpoint: device.endpoint,
    tunnel_id: device.tunnelId,
    relay_credential: device.relayCredential,
    device_credential: device.deviceCredential,
    device_id: device.deviceId,
    pairing_token: device.pairingToken,
    expires_at: device.expiresAt,
  };
}

export const panesDeviceStore = {
  devices,
  activePanesId,
  activeDevice: computed(() => devices.value.find((item) => item.panesId === activePanesId.value) ?? null),
  initialize() {
    if (initialized) return;
    initialized = true;
    const settings = readSettings();
    if (settings) {
      devices.value = settings.devices;
      activePanesId.value = settings.activePanesId;
      return;
    }
    migrateLegacyPairing();
  },
  getDevice(panesId: string) {
    return devices.value.find((item) => item.panesId === panesId) ?? null;
  },
  getRemoteConfig(panesId: string) {
    const device = devices.value.find((item) => item.panesId === panesId);
    return device ? toRemoteConfig(device) : null;
  },
  addOrReplace(config: PairingConfig, preferredPanesId?: string) {
    const existingIndex = devices.value.findIndex((item) => item.panesId === (preferredPanesId || config.tunnel_id));
    const existing = existingIndex >= 0 ? devices.value[existingIndex] : undefined;
    const device = toPairedPanes(config, existing ? { ...existing, panesId: preferredPanesId || existing.panesId } : undefined);
    if (existingIndex >= 0) devices.value.splice(existingIndex, 1, device);
    else devices.value.push(device);
    activePanesId.value = device.panesId;
    saveSettings();
    return device;
  },
  setActive(panesId: string) {
    if (!devices.value.some((item) => item.panesId === panesId)) return;
    activePanesId.value = panesId;
    saveSettings();
  },
  rename(panesId: string, name: string) {
    const device = devices.value.find((item) => item.panesId === panesId);
    if (!device) return;
    device.name = name.trim() || device.name;
    saveSettings();
  },
  updatePairedCredential(panesId: string, config: PairingConfig) {
    const device = devices.value.find((item) => item.panesId === panesId);
    if (!device) return;
    Object.assign(device, toPairedPanes(config, device));
    saveSettings();
  },
  markConnected(panesId: string) {
    const device = devices.value.find((item) => item.panesId === panesId);
    if (!device) return;
    device.lastConnectedAt = new Date().toISOString();
    saveSettings();
  },
  remove(panesId: string) {
    const index = devices.value.findIndex((item) => item.panesId === panesId);
    if (index < 0) return;
    devices.value.splice(index, 1);
    if (activePanesId.value === panesId) activePanesId.value = devices.value[0]?.panesId ?? null;
    saveSettings();
  },
};
