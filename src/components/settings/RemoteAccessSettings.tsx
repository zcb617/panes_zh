import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { CheckCircle2, Clipboard, RefreshCw, ShieldCheck, Smartphone, Wifi, WifiOff } from "lucide-react";
import { useTranslation } from "react-i18next";
import { ipc } from "../../lib/ipc";
import { toast } from "../../stores/toastStore";
import type { RemoteAccessStatus } from "../../types";

export function RemoteAccessSettings() {
  const { t } = useTranslation(["app", "common"]);
  const [status, setStatus] = useState<RemoteAccessStatus | null>(null);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [rotating, setRotating] = useState(false);
  const [refreshing, setRefreshing] = useState(false);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;

    void ipc.getRemoteAccessStatus()
      .then((nextStatus) => {
        if (!disposed) setStatus(nextStatus);
      })
      .catch((error) => {
        if (!disposed) toast.error(t("app:settingsPage.remoteAccess.loadFailed", { error: String(error) }));
      })
      .finally(() => {
        if (!disposed) setLoading(false);
      });

    void listen<RemoteAccessStatus>("remote-access-updated", (event) => {
      if (!disposed) setStatus(event.payload);
    }).then((dispose) => {
      if (disposed) dispose();
      else unlisten = dispose;
    });

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [t]);

  return (
    <>
      <section className="usp-section usp-section-first">
        <div className="usp-section-header">
          <h2>{t("app:settingsPage.remoteAccess.serviceTitle")}</h2>
          <p>{t("app:settingsPage.remoteAccess.serviceDescription")}</p>
        </div>
        <div className="usp-group">
          <div className="usp-row">
            <span className="usp-row-icon"><Smartphone size={17} /></span>
            <span className="usp-row-copy">
              <span className="usp-row-title">{t("app:settingsPage.remoteAccess.enableTitle")}</span>
              <span className="usp-row-description">{t("app:settingsPage.remoteAccess.enableDescription")}</span>
            </span>
            <div className="usp-row-control">
              <label className="ws-toggle" title={t("app:settingsPage.remoteAccess.enableTitle")}>
                <input
                  type="checkbox"
                  checked={status?.enabled ?? false}
                  disabled={loading || saving}
                  aria-label={t("app:settingsPage.remoteAccess.enableTitle")}
                  onChange={(event) => {
                    const enabled = event.target.checked;
                    setSaving(true);
                    void ipc.setRemoteAccessEnabled(enabled)
                      .then((nextStatus) => {
                        setStatus(nextStatus);
                        toast.success(t(enabled
                          ? "app:settingsPage.remoteAccess.enabledToast"
                          : "app:settingsPage.remoteAccess.disabledToast"));
                      })
                      .catch((error) => toast.error(t("app:settingsPage.remoteAccess.saveFailed", { error: String(error) })))
                      .finally(() => setSaving(false));
                  }}
                />
                <span className="ws-toggle-track" />
                <span className="ws-toggle-thumb" />
              </label>
            </div>
          </div>

          <div className="usp-row">
            <span className="usp-row-icon">
              {status?.connected ? <Wifi size={17} /> : <WifiOff size={17} />}
            </span>
            <span className="usp-row-copy">
              <span className="usp-row-title">{t("app:settingsPage.remoteAccess.connectionTitle")}</span>
              <span className="usp-row-description">
                {loading
                  ? t("app:settingsPage.remoteAccess.loading")
                  : !status?.enabled
                    ? t("app:settingsPage.remoteAccess.disabled")
                    : status.connected
                      ? status.peerOnline
                        ? t("app:settingsPage.remoteAccess.phoneOnline")
                        : t("app:settingsPage.remoteAccess.waitingForPhone")
                      : t("app:settingsPage.remoteAccess.connecting")}
              </span>
              {status?.lastError ? <span className="remote-access-error">{status.lastError}</span> : null}
            </span>
            <div className="usp-row-control">
              <span className={`usp-status${status?.connected ? " usp-status-ready" : ""}`}>
                {status?.connected
                  ? t("app:settingsPage.remoteAccess.connected")
                  : t("app:settingsPage.remoteAccess.offline")}
              </span>
            </div>
          </div>

          <div className="usp-row">
            <span className="usp-row-icon"><ShieldCheck size={17} /></span>
            <span className="usp-row-copy">
              <span className="usp-row-title">{t("app:settingsPage.remoteAccess.relayTitle")}</span>
              <span className="usp-row-description remote-access-mono">
                {status?.endpoint || "wss://panes.jxrjkf.cn/ws/tunnel"}
              </span>
            </span>
          </div>
        </div>
      </section>

      {status?.enabled ? (
        <section className="usp-section">
          <div className="usp-section-header">
            <h2>{t("app:settingsPage.remoteAccess.pairTitle")}</h2>
            <p>{t("app:settingsPage.remoteAccess.pairDescription")}</p>
          </div>
          <div className="remote-access-pairing-card">
            {status.pairingQrSvg && status.pairingPayload ? (
              <div className="remote-access-qr-wrap">
                <img
                  className="remote-access-qr"
                  src={`data:image/svg+xml;charset=utf-8,${encodeURIComponent(status.pairingQrSvg)}`}
                  alt={t("app:settingsPage.remoteAccess.qrAlt")}
                />
              </div>
            ) : (
              <div className="remote-access-qr-wrap remote-access-paired-state">
                <ShieldCheck size={58} />
                <strong>{t("app:settingsPage.remoteAccess.pairedTitle")}</strong>
                <span>{t("app:settingsPage.remoteAccess.pairedDescription")}</span>
              </div>
            )}
            <div className="remote-access-pairing-copy">
              <div>
                <span className="remote-access-label">{t("app:settingsPage.remoteAccess.deviceId")}</span>
                <code>{status.tunnelId}</code>
              </div>
              <p>
                {status.pairingExpiresAt
                  ? t("app:settingsPage.remoteAccess.expiresAt", {
                      time: new Intl.DateTimeFormat(undefined, { timeStyle: "short" }).format(new Date(status.pairingExpiresAt)),
                    })
                  : t("app:settingsPage.remoteAccess.securityNote")}
              </p>
              <div className="remote-access-actions">
                {status.pairingPayload ? (
                  <button
                    type="button"
                    className="usp-button"
                    onClick={() => {
                      void navigator.clipboard.writeText(status.pairingPayload ?? "")
                        .then(() => toast.success(t("app:settingsPage.remoteAccess.copied")))
                        .catch((error) => toast.error(t("app:settingsPage.remoteAccess.copyFailed", { error: String(error) })));
                    }}
                  >
                    <Clipboard size={13} />
                    {t("app:settingsPage.remoteAccess.copyPairing")}
                  </button>
                ) : null}
                <button
                  type="button"
                  className="usp-button"
                  disabled={refreshing}
                  onClick={() => {
                    setRefreshing(true);
                    void ipc.refreshRemotePairingToken()
                      .then((nextStatus) => {
                        setStatus(nextStatus);
                        toast.success(t("app:settingsPage.remoteAccess.refreshed"));
                      })
                      .catch((error) => toast.error(t("app:settingsPage.remoteAccess.refreshFailed", { error: String(error) })))
                      .finally(() => setRefreshing(false));
                  }}
                >
                  {refreshing ? <RefreshCw size={13} className="usp-spin" /> : <RefreshCw size={13} />}
                  {t("app:settingsPage.remoteAccess.refresh")}
                </button>
                <button
                  type="button"
                  className="usp-button"
                  disabled={rotating}
                  onClick={() => {
                    setRotating(true);
                    void ipc.regenerateRemoteAccessIdentity()
                      .then((nextStatus) => {
                        setStatus(nextStatus);
                        toast.success(t("app:settingsPage.remoteAccess.rotated"));
                      })
                      .catch((error) => toast.error(t("app:settingsPage.remoteAccess.rotateFailed", { error: String(error) })))
                      .finally(() => setRotating(false));
                  }}
                >
                  {rotating ? <RefreshCw size={13} className="usp-spin" /> : <RefreshCw size={13} />}
                  {t("app:settingsPage.remoteAccess.revoke")}
                </button>
              </div>
              <span className="remote-access-secure">
                <CheckCircle2 size={14} />
                {t("app:settingsPage.remoteAccess.tlsProtected")}
              </span>
            </div>
          </div>
        </section>
      ) : null}
    </>
  );
}
