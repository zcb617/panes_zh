import { create } from "zustand";
import { check, type DownloadEvent, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";

export const DEFAULT_AUTO_UPDATE_INTERVAL_MINUTES = 30;

const AUTO_UPDATE_INTERVAL_STORAGE_KEY = "panes:auto-update-interval-minutes";

type UpdateStatus =
  | "idle"
  | "checking"
  | "available"
  | "downloading"
  | "downloaded"
  | "ready"
  | "error";
type DownloadPhase = "idle" | "downloading" | "installing";
type UpdateCheckMode = "manual" | "automatic";
type UpdateDownloadSource = UpdateCheckMode | null;

function readAutoUpdateIntervalMinutes(): number {
  try {
    const stored = localStorage.getItem(AUTO_UPDATE_INTERVAL_STORAGE_KEY);
    if (stored === null) return DEFAULT_AUTO_UPDATE_INTERVAL_MINUTES;

    const value = Number.parseInt(stored, 10);
    return Number.isFinite(value) && value >= 0
      ? value
      : DEFAULT_AUTO_UPDATE_INTERVAL_MINUTES;
  } catch {
    return DEFAULT_AUTO_UPDATE_INTERVAL_MINUTES;
  }
}

function normalizeAutoUpdateIntervalMinutes(value: number): number {
  if (!Number.isFinite(value)) return DEFAULT_AUTO_UPDATE_INTERVAL_MINUTES;
  return Math.max(0, Math.floor(value));
}

function saveAutoUpdateIntervalMinutes(value: number): void {
  try {
    localStorage.setItem(AUTO_UPDATE_INTERVAL_STORAGE_KEY, String(value));
  } catch {
    // Ignore storage failures; the setting remains available for this session.
  }
}

interface UpdateState {
  status: UpdateStatus;
  version: string | null;
  error: string | null;
  lastCheckedAt: number | null;
  downloadPhase: DownloadPhase;
  downloadedBytes: number;
  totalBytes: number | null;
  update: Update | null;
  downloadSource: UpdateDownloadSource;
  autoUpdateIntervalMinutes: number;
  /** True after user clicks "Not now" — hides dot until next app launch */
  snoozed: boolean;

  checkForUpdate: (mode?: UpdateCheckMode) => Promise<void>;
  downloadAndInstall: () => Promise<void>;
  installDownloadedUpdate: () => Promise<void>;
  setAutoUpdateIntervalMinutes: (minutes: number) => void;
  resetToIdle: () => void;
  snooze: () => void;
}

export const useUpdateStore = create<UpdateState>((set, get) => {
  async function downloadUpdate(
    update: Update,
    source: UpdateDownloadSource,
    installAfterDownload: boolean,
  ): Promise<void> {
    set({
      status: "downloading",
      version: update.version,
      update,
      downloadSource: source,
      error: null,
      downloadPhase: "downloading",
      downloadedBytes: 0,
      totalBytes: null,
    });

    const onEvent = (event: DownloadEvent) => {
      if (event.event === "Started") {
        set({
          downloadPhase: "downloading",
          downloadedBytes: 0,
          totalBytes: event.data.contentLength ?? null,
        });
        return;
      }
      if (event.event === "Progress") {
        set((state) => ({
          downloadedBytes: state.downloadedBytes + event.data.chunkLength,
        }));
        return;
      }

      set((state) => ({
        downloadPhase: installAfterDownload ? "installing" : "downloading",
        downloadedBytes: state.totalBytes ?? state.downloadedBytes,
      }));
    };

    if (installAfterDownload) {
      await update.downloadAndInstall(onEvent);
      set({ status: "ready" });
      await relaunch();
      return;
    }

    await update.download(onEvent);
    set({ status: "downloaded", downloadPhase: "idle" });
  }

  return {
    status: "idle",
    version: null,
    error: null,
    lastCheckedAt: null,
    downloadPhase: "idle",
    downloadedBytes: 0,
    totalBytes: null,
    update: null,
    downloadSource: null,
    autoUpdateIntervalMinutes: readAutoUpdateIntervalMinutes(),
    snoozed: false,

    checkForUpdate: async (mode = "manual") => {
      const currentStatus = get().status;
      if (["checking", "downloading", "downloaded", "ready"].includes(currentStatus)) {
        return;
      }
      if (mode === "automatic" && currentStatus === "available") {
        return;
      }

      set({
        status: "checking",
        error: null,
        update: null,
        downloadSource: mode,
        downloadPhase: "idle",
        downloadedBytes: 0,
        totalBytes: null,
      });

      let update: Update | null;
      try {
        update = await check();
      } catch {
        // Silent on network errors — no degradation if the endpoint is unreachable.
        set({
          status: "idle",
          update: null,
          downloadSource: null,
          downloadPhase: "idle",
        });
        return;
      }

      if (!update) {
        set({
          status: "idle",
          version: null,
          update: null,
          downloadSource: null,
          lastCheckedAt: Date.now(),
          downloadPhase: "idle",
        });
        return;
      }

      set({ version: update.version, lastCheckedAt: Date.now() });

      if (mode === "automatic") {
        try {
          await downloadUpdate(update, "automatic", false);
        } catch (err) {
          set({
            status: "error",
            error: err instanceof Error ? err.message : "Update failed",
            downloadPhase: "idle",
          });
        }
        return;
      }

      set({
        status: "available",
        update,
        downloadSource: "manual",
      });
    },

    downloadAndInstall: async () => {
      const currentState = get();
      if (["downloading", "downloaded", "ready"].includes(currentState.status)) {
        return;
      }

      let update = currentState.update;
      set({
        status: "downloading",
        error: null,
        downloadSource: "manual",
        downloadPhase: "downloading",
        downloadedBytes: 0,
        totalBytes: null,
      });

      try {
        update ??= await check();
        if (!update) {
          set({
            status: "idle",
            version: null,
            update: null,
            downloadSource: null,
            downloadPhase: "idle",
            downloadedBytes: 0,
            totalBytes: null,
          });
          return;
        }
        await downloadUpdate(update, "manual", true);
      } catch (err) {
        set({
          status: "error",
          error: err instanceof Error ? err.message : "Update failed",
          downloadPhase: "idle",
        });
      }
    },

    installDownloadedUpdate: async () => {
      const { status, update } = get();
      if (status !== "downloaded" || !update) return;

      set({
        status: "downloading",
        error: null,
        downloadPhase: "installing",
      });
      try {
        await update.install();
        set({ status: "ready" });
        await relaunch();
      } catch (err) {
        set({
          status: "error",
          error: err instanceof Error ? err.message : "Update failed",
          downloadPhase: "idle",
        });
      }
    },

    setAutoUpdateIntervalMinutes: (minutes) => {
      const normalized = normalizeAutoUpdateIntervalMinutes(minutes);
      saveAutoUpdateIntervalMinutes(normalized);
      set({ autoUpdateIntervalMinutes: normalized });
    },

    resetToIdle: () => {
      set({
        status: "idle",
        version: null,
        error: null,
        update: null,
        downloadSource: null,
        downloadPhase: "idle",
        downloadedBytes: 0,
        totalBytes: null,
      });
    },

    snooze: () => {
      set({ snoozed: true });
    },
  };
});
