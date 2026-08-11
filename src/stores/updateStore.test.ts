import { beforeEach, describe, expect, it, vi } from "vitest";

const updaterMocks = vi.hoisted(() => ({
  check: vi.fn(),
  relaunch: vi.fn(),
}));

const storageMock = vi.hoisted(() => ({
  getItem: vi.fn(),
  setItem: vi.fn(),
}));

vi.mock("@tauri-apps/plugin-updater", () => ({
  check: updaterMocks.check,
}));

vi.mock("@tauri-apps/plugin-process", () => ({
  relaunch: updaterMocks.relaunch,
}));

import { useUpdateStore } from "./updateStore";

describe("updateStore", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.stubGlobal("localStorage", storageMock);
    useUpdateStore.setState({
      status: "idle",
      version: null,
      error: null,
      lastCheckedAt: null,
      downloadPhase: "idle",
      downloadedBytes: 0,
      totalBytes: null,
      update: null,
      downloadSource: null,
      autoUpdateIntervalMinutes: 30,
      snoozed: false,
    });
    updaterMocks.relaunch.mockResolvedValue(undefined);
  });

  it("tracks determinate download progress before installation", async () => {
    const downloadAndInstall = vi.fn(async (onEvent: (event: unknown) => void) => {
      onEvent({ event: "Started", data: { contentLength: 1000 } });
      onEvent({ event: "Progress", data: { chunkLength: 400 } });
      onEvent({ event: "Progress", data: { chunkLength: 600 } });
      onEvent({ event: "Finished" });
    });
    updaterMocks.check.mockResolvedValue({ downloadAndInstall });

    await useUpdateStore.getState().downloadAndInstall();

    expect(downloadAndInstall).toHaveBeenCalledOnce();
    expect(useUpdateStore.getState()).toMatchObject({
      status: "ready",
      downloadPhase: "installing",
      downloadedBytes: 1000,
      totalBytes: 1000,
    });
    expect(updaterMocks.relaunch).toHaveBeenCalledOnce();
  });

  it("tracks bytes when the server does not provide a total size", async () => {
    updaterMocks.check.mockResolvedValue({
      downloadAndInstall: async (onEvent: (event: unknown) => void) => {
        onEvent({ event: "Started", data: {} });
        onEvent({ event: "Progress", data: { chunkLength: 256 } });
        onEvent({ event: "Finished" });
      },
    });

    await useUpdateStore.getState().downloadAndInstall();

    expect(useUpdateStore.getState()).toMatchObject({
      status: "ready",
      downloadPhase: "installing",
      downloadedBytes: 256,
      totalBytes: null,
    });
  });

  it("downloads automatic updates without installing them", async () => {
    const download = vi.fn(async (onEvent: (event: unknown) => void) => {
      onEvent({ event: "Started", data: { contentLength: 1000 } });
      onEvent({ event: "Progress", data: { chunkLength: 1000 } });
      onEvent({ event: "Finished" });
    });
    const install = vi.fn().mockResolvedValue(undefined);
    updaterMocks.check.mockResolvedValue({ version: "0.65.2", download, install });

    await useUpdateStore.getState().checkForUpdate("automatic");

    expect(download).toHaveBeenCalledOnce();
    expect(install).not.toHaveBeenCalled();
    expect(updaterMocks.relaunch).not.toHaveBeenCalled();
    expect(useUpdateStore.getState()).toMatchObject({
      status: "downloaded",
      version: "0.65.2",
      downloadSource: "automatic",
      downloadedBytes: 1000,
      totalBytes: 1000,
    });

    await useUpdateStore.getState().installDownloadedUpdate();

    expect(install).toHaveBeenCalledOnce();
    expect(updaterMocks.relaunch).toHaveBeenCalledOnce();
  });

  it("persists a zero interval to disable automatic checks", () => {
    useUpdateStore.getState().setAutoUpdateIntervalMinutes(0);

    expect(useUpdateStore.getState().autoUpdateIntervalMinutes).toBe(0);
    expect(storageMock.setItem).toHaveBeenCalledWith("panes:auto-update-interval-minutes", "0");
  });
});
