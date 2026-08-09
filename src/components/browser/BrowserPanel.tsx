import { useCallback, useEffect, useRef, useState } from "react";
import { isTauri } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  ArrowLeft,
  ArrowRight,
  Crosshair,
  Globe2,
  RefreshCw,
  Trash2,
  X,
} from "lucide-react";
import { nextBrowserAnnotationNumber } from "../../lib/browserAnnotationNumber";
import { ipc } from "../../lib/ipc";
import { useChatStore } from "../../stores/chatStore";
import { useChatComposerStore } from "../../stores/chatComposerStore";
import { toast } from "../../stores/toastStore";
import { useThreadStore } from "../../stores/threadStore";
import { useWorkspaceStore } from "../../stores/workspaceStore";
import type {
  BrowserAnnotationSubmission,
  BrowserAnnotationSelection,
  BrowserBounds,
  ChatAttachment,
} from "../../types";

const DEFAULT_BROWSER_URL = "https://www.qq.com/";
const EMPTY_CHAT_ATTACHMENTS: ChatAttachment[] = [];
const browserAnnotationAttachmentsByScope = new Map<string, ChatAttachment[]>();
const browserAddressByScope = new Map<string, string>();
const browserAnnotationModeByScope = new Map<string, boolean>();

interface BrowserPanelProps {
  visible?: boolean;
}

interface BrowserNavigatedEvent {
  scope: string;
  url: string;
}

interface BrowserAnnotationSubmittedEvent {
  scope: string;
  submission: BrowserAnnotationSubmission;
}

interface BrowserAnnotationCanceledEvent {
  scope: string;
}

interface BrowserScopeTransfer {
  fromScope: string;
  toScope: string;
  promise: Promise<void>;
}

function getBrowserBounds(node: HTMLDivElement): BrowserBounds | null {
  const rect = node.getBoundingClientRect();
  if (rect.width < 1 || rect.height < 1) {
    return null;
  }
  return {
    x: rect.left,
    y: rect.top,
    width: rect.width,
    height: rect.height,
  };
}

function normalizeBrowserUrl(value: string): string {
  const normalized = value.trim();
  if (!normalized) {
    return DEFAULT_BROWSER_URL;
  }
  return normalized.includes("://") ? normalized : `https://${normalized}`;
}

function browserAnnotationScopeKey(workspaceId: string, threadId: string | null): string {
  return threadId ? `thread:${threadId}` : `draft:${workspaceId}`;
}

function isBrowserAnnotation(attachment: ChatAttachment): boolean {
  return Boolean(attachment.browserAnnotation);
}

function sameAttachments(left: ChatAttachment[], right: ChatAttachment[]): boolean {
  return left.length === right.length && left.every((attachment, index) => attachment.id === right[index]?.id);
}

export function BrowserPanel({ visible = true }: BrowserPanelProps) {
  const hostRef = useRef<HTMLDivElement>(null);
  const annotationScopeRef = useRef<string | null>(null);
  const annotationWorkspaceRef = useRef<string | null>(null);
  const browserScopeTransferRef = useRef<BrowserScopeTransfer | null>(null);
  const activeWorkspaceId = useWorkspaceStore((state) => state.activeWorkspaceId);
  const activeThreadId = useThreadStore((state) => state.activeThreadId);
  const setWorkspaceAttachments = useChatComposerStore(
    (state) => state.setWorkspaceAttachments,
  );
  const composerAttachments = useChatComposerStore((state) =>
    activeWorkspaceId
      ? state.attachmentsByWorkspace[activeWorkspaceId] ?? EMPTY_CHAT_ATTACHMENTS
      : EMPTY_CHAT_ATTACHMENTS,
  );
  const [address, setAddress] = useState(DEFAULT_BROWSER_URL);
  const [annotationEnabled, setAnnotationEnabled] = useState(false);
  const [saving, setSaving] = useState(false);
  const [browserError, setBrowserError] = useState<string | null>(null);
  const annotationScope = activeWorkspaceId
    ? browserAnnotationScopeKey(activeWorkspaceId, activeThreadId)
    : null;

  useEffect(() => {
    if (!activeWorkspaceId || !annotationScope) {
      annotationScopeRef.current = null;
      annotationWorkspaceRef.current = null;
      return;
    }

    const previousScope = annotationScopeRef.current;
    const previousWorkspaceId = annotationWorkspaceRef.current;
    const visibleBrowserAttachments = composerAttachments.filter(isBrowserAnnotation);

    if (previousScope && previousScope !== annotationScope) {
      if (previousWorkspaceId === activeWorkspaceId) {
        browserAnnotationAttachmentsByScope.set(previousScope, visibleBrowserAttachments);
      }

      const transfersDraftBrowser =
        previousWorkspaceId === activeWorkspaceId &&
        previousScope.startsWith("draft:") &&
        annotationScope.startsWith("thread:");
      if (transfersDraftBrowser) {
        browserAnnotationAttachmentsByScope.set(
          annotationScope,
          browserAnnotationAttachmentsByScope.get(previousScope) ?? [],
        );
        browserAnnotationAttachmentsByScope.delete(previousScope);

        const previousAddress = browserAddressByScope.get(previousScope);
        if (previousAddress) {
          browserAddressByScope.set(annotationScope, previousAddress);
          browserAddressByScope.delete(previousScope);
        }

        const previousMode = browserAnnotationModeByScope.get(previousScope);
        if (previousMode !== undefined) {
          browserAnnotationModeByScope.set(annotationScope, previousMode);
          browserAnnotationModeByScope.delete(previousScope);
        }

        if (isTauri()) {
          browserScopeTransferRef.current = {
            fromScope: previousScope,
            toScope: annotationScope,
            promise: ipc.browserTransferScope(previousScope, annotationScope),
          };
        }
      }
    }

    if (!previousScope) {
      browserAnnotationAttachmentsByScope.set(annotationScope, visibleBrowserAttachments);
    }

    const scopedBrowserAttachments = browserAnnotationAttachmentsByScope.get(annotationScope) ?? [];
    browserAnnotationAttachmentsByScope.set(annotationScope, scopedBrowserAttachments);
    const nextComposerAttachments = [
      ...composerAttachments.filter((attachment) => !isBrowserAnnotation(attachment)),
      ...scopedBrowserAttachments,
    ];
    if (!sameAttachments(composerAttachments, nextComposerAttachments)) {
      setWorkspaceAttachments(activeWorkspaceId, nextComposerAttachments);
    }

    annotationScopeRef.current = annotationScope;
    annotationWorkspaceRef.current = activeWorkspaceId;
    const targetUrl = browserAddressByScope.get(annotationScope) ?? DEFAULT_BROWSER_URL;
    setAddress(targetUrl);
    setAnnotationEnabled(browserAnnotationModeByScope.get(annotationScope) ?? false);
  }, [activeWorkspaceId, annotationScope, composerAttachments, setWorkspaceAttachments]);

  useEffect(() => {
    if (!annotationScope) {
      return;
    }
    const visibleBrowserAttachments = composerAttachments.filter(isBrowserAnnotation);
    const scopedBrowserAttachments = browserAnnotationAttachmentsByScope.get(annotationScope);
    if (
      scopedBrowserAttachments &&
      visibleBrowserAttachments.every((attachment) =>
        scopedBrowserAttachments.some((known) => known.id === attachment.id),
      )
    ) {
      browserAnnotationAttachmentsByScope.set(annotationScope, visibleBrowserAttachments);
    }
  }, [annotationScope, composerAttachments]);

  const syncBrowserBounds = useCallback(async () => {
    const host = hostRef.current;
    const scope = annotationScopeRef.current;
    if (!host || !scope || !isTauri() || !visible) {
      return;
    }
    const bounds = getBrowserBounds(host);
    if (!bounds) {
      return;
    }
    try {
      const transfer = browserScopeTransferRef.current;
      if (transfer?.toScope === scope) {
        try {
          await transfer.promise;
        } catch (error) {
          setBrowserError(error instanceof Error ? error.message : "新对话浏览器状态交接失败。");
          return;
        } finally {
          if (browserScopeTransferRef.current === transfer) {
            browserScopeTransferRef.current = null;
          }
        }
      }
      // browser_show only creates a WebView for a scope once. Later calls merely hide
      // the other conversations and show this exact instance again.
      await ipc.browserShow(scope, bounds, DEFAULT_BROWSER_URL);
      setBrowserError(null);
    } catch (error) {
      setBrowserError(error instanceof Error ? error.message : "浏览器初始化失败。");
    }
  }, [visible]);

  useEffect(() => {
    if (!isTauri()) {
      setBrowserError("内嵌浏览器只能在桌面端使用。");
      return;
    }
    if (!visible) {
      const scope = annotationScopeRef.current;
      if (scope) {
        void ipc.browserHide(scope);
      }
      return;
    }

    const host = hostRef.current;
    if (!host) {
      return;
    }
    const observer = new ResizeObserver(() => {
      void syncBrowserBounds();
    });
    observer.observe(host);
    return () => observer.disconnect();
  }, [syncBrowserBounds, visible]);

  useEffect(() => {
    if (visible) {
      void syncBrowserBounds();
    }
  }, [annotationScope, syncBrowserBounds, visible]);

  useEffect(() => () => {
    const scope = annotationScopeRef.current;
    if (scope && isTauri()) {
      void ipc.browserHide(scope);
    }
  }, []);

  useEffect(() => {
    if (!isTauri()) {
      return;
    }
    let unlisten: UnlistenFn | undefined;
    void listen<BrowserNavigatedEvent>("browser:navigated", (event) => {
      browserAddressByScope.set(event.payload.scope, event.payload.url);
      if (event.payload.scope === annotationScopeRef.current) {
        setAddress(event.payload.url);
      }
    }).then((listener) => {
      unlisten = listener;
    });
    return () => unlisten?.();
  }, []);

  const navigate = useCallback(async () => {
    const scope = annotationScopeRef.current;
    if (!scope) {
      return;
    }
    const url = normalizeBrowserUrl(address);
    setAddress(url);
    try {
      const resolvedUrl = await ipc.browserNavigate(scope, url);
      browserAddressByScope.set(scope, resolvedUrl);
      setAddress(resolvedUrl);
      setBrowserError(null);
    } catch (error) {
      setBrowserError(error instanceof Error ? error.message : "无法打开这个地址。");
    }
  }, [address]);

  const startAnnotation = useCallback(async () => {
    const scope = annotationScopeRef.current;
    if (!scope) {
      return;
    }
    try {
      await ipc.browserClearPendingAnnotation(scope);
      await ipc.browserSetAnnotationEnabled(scope, true);
      browserAnnotationModeByScope.set(scope, true);
      setAnnotationEnabled(true);
      setBrowserError(null);
    } catch (error) {
      setBrowserError(error instanceof Error ? error.message : "无法进入标注模式。");
    }
  }, []);

  const cancelAnnotation = useCallback(async () => {
    const scope = annotationScopeRef.current;
    if (!scope) {
      return;
    }
    try {
      await ipc.browserSetAnnotationEnabled(scope, false);
      await ipc.browserClearPendingAnnotation(scope);
    } catch {
      // The page may already have navigated. The local button state can still be reset safely.
    }
    browserAnnotationModeByScope.set(scope, false);
    setAnnotationEnabled(false);
  }, []);

  const saveAnnotation = useCallback(async (
    selection: BrowserAnnotationSelection,
    comment: string,
  ) => {
    const normalizedComment = comment.trim();
    if (!normalizedComment || saving) {
      return;
    }
    if (!activeWorkspaceId || !annotationScope) {
      toast.warning("请先打开一个工作区，再添加标注附件。");
      return;
    }

    setSaving(true);
    try {
      const composer = useChatComposerStore.getState();
      const current = composer.attachmentsByWorkspace[activeWorkspaceId] ?? [];
      const pendingAttachments =
        browserAnnotationAttachmentsByScope.get(annotationScope) ??
        current.filter(isBrowserAnnotation);
      const chat = useChatStore.getState();
      const number = nextBrowserAnnotationNumber({
        threadId: activeThreadId,
        messages: chat.messages,
        pendingAttachments,
      });
      const saved = await ipc.browserCaptureAnnotation(annotationScope, number, selection);
      const attachment: ChatAttachment = {
        id: crypto.randomUUID(),
        fileName: saved.fileName,
        filePath: saved.filePath,
        sizeBytes: saved.sizeBytes,
        mimeType: saved.mimeType,
        browserAnnotation: {
          comment: normalizedComment,
          number,
          sourceUrl: selection.url,
          targetLabel: selection.targetLabel,
        },
      };
      const nextPendingAttachments = [...pendingAttachments, attachment];
      browserAnnotationAttachmentsByScope.set(annotationScope, nextPendingAttachments);
      setWorkspaceAttachments(activeWorkspaceId, [
        ...current.filter((item) => !isBrowserAnnotation(item)),
        ...nextPendingAttachments,
      ]);
      browserAnnotationModeByScope.set(annotationScope, false);
      setAnnotationEnabled(false);
      toast.success("浏览器标注已作为附件加入当前对话。");
    } catch (error) {
      setBrowserError(error instanceof Error ? error.message : "标注截图保存失败。");
      void ipc.browserClearPendingAnnotation(annotationScope);
    } finally {
      setSaving(false);
    }
  }, [activeThreadId, activeWorkspaceId, annotationScope, saving, setWorkspaceAttachments]);

  useEffect(() => {
    if (!isTauri()) {
      return;
    }
    let unlistenSubmission: UnlistenFn | undefined;
    let unlistenCancel: UnlistenFn | undefined;
    void listen<BrowserAnnotationSubmittedEvent>("browser:annotation-submitted", (event) => {
      if (event.payload.scope !== annotationScopeRef.current) {
        return;
      }
      browserAnnotationModeByScope.set(event.payload.scope, false);
      setAnnotationEnabled(false);
      void saveAnnotation(event.payload.submission.selection, event.payload.submission.comment);
    }).then((listener) => {
      unlistenSubmission = listener;
    });
    void listen<BrowserAnnotationCanceledEvent>("browser:annotation-canceled", (event) => {
      if (event.payload.scope !== annotationScopeRef.current) {
        return;
      }
      browserAnnotationModeByScope.set(event.payload.scope, false);
      setAnnotationEnabled(false);
    }).then((listener) => {
      unlistenCancel = listener;
    });
    return () => {
      unlistenSubmission?.();
      unlistenCancel?.();
    };
  }, [saveAnnotation]);

  const clearAllAnnotations = useCallback(async () => {
    const scope = annotationScopeRef.current;
    if (!scope) {
      return;
    }
    try {
      await ipc.browserClearAllAnnotations(scope);
    } catch (error) {
      setBrowserError(error instanceof Error ? error.message : "无法清除页面标记。");
    }
  }, []);

  return (
    <section className="browser-panel" aria-label="浏览器">
      <header className="browser-panel-toolbar">
        <div className="browser-nav-actions">
          <button
            type="button"
            className="browser-toolbar-button"
            onClick={() => {
              const scope = annotationScopeRef.current;
              if (scope) void ipc.browserGoBack(scope);
            }}
            title="后退"
            aria-label="后退"
          >
            <ArrowLeft size={14} />
          </button>
          <button
            type="button"
            className="browser-toolbar-button"
            onClick={() => {
              const scope = annotationScopeRef.current;
              if (scope) void ipc.browserGoForward(scope);
            }}
            title="前进"
            aria-label="前进"
          >
            <ArrowRight size={14} />
          </button>
          <button
            type="button"
            className="browser-toolbar-button"
            onClick={() => {
              const scope = annotationScopeRef.current;
              if (scope) void ipc.browserReload(scope);
            }}
            title="刷新"
            aria-label="刷新"
          >
            <RefreshCw size={13} />
          </button>
        </div>
        <form
          className="browser-address-form"
          onSubmit={(event) => {
            event.preventDefault();
            void navigate();
          }}
        >
          <Globe2 size={13} aria-hidden="true" />
          <input
            value={address}
            onChange={(event) => setAddress(event.target.value)}
            aria-label="网页地址"
            spellCheck={false}
          />
        </form>
        <button
          type="button"
          className={`browser-annotate-button${annotationEnabled ? " browser-annotate-button-active" : ""}`}
          onClick={() => void (annotationEnabled ? cancelAnnotation() : startAnnotation())}
          title={annotationEnabled ? "退出标注模式" : "标注网页元素"}
          aria-pressed={annotationEnabled}
          disabled={saving}
        >
          {annotationEnabled ? <X size={13} /> : <Crosshair size={13} />}
          {annotationEnabled ? "退出标注" : "标注"}
        </button>
        <button
          type="button"
          className="browser-clear-annotations"
          onClick={() => void clearAllAnnotations()}
          title="清除页面上的全部标记，不影响已加入对话的图片附件"
          disabled={saving}
        >
          <Trash2 size={13} />
          清除标记
        </button>
      </header>

      <div ref={hostRef} className="browser-webview-host" />

      {browserError ? <div className="browser-panel-error">{browserError}</div> : null}
    </section>
  );
}
