export interface PairingConfig {
  version: 1;
  endpoint: string;
  tunnel_id: string;
  relay_credential: string;
  pairing_token?: string;
  device_credential?: string;
  // 桌面端为当前手机分配的稳定设备 ID；重连时继续复用该 ID。
  device_id?: string;
  expires_at?: string;
}

export interface PairedPanes {
  panesId: string;
  name: string;
  endpoint: string;
  tunnelId: string;
  relayCredential: string;
  deviceCredential?: string;
  // deviceId 只在当前 panesId 范围内有效，用于设备级实时事件校验。
  deviceId?: string;
  pairingToken?: string;
  expiresAt?: string;
  pairedAt: string;
  lastConnectedAt?: string;
}

export interface MobilePanesSettings {
  devices: PairedPanes[];
  activePanesId: string | null;
}

export interface ConnectionState {
  relayConnected: boolean;
  peerOnline: boolean;
  lastError: string | null;
}

export interface Workspace {
  id: string;
  name: string;
  rootPath: string;
  lastOpenedAt: string;
}

export interface Thread {
  id: string;
  workspaceId: string;
  engineId: string;
  modelId: string;
  engineMetadata?: Record<string, unknown> | null;
  title: string;
  status: "idle" | "streaming" | "awaiting_approval" | "error" | "completed";
  messageCount: number;
  lastActivityAt: string;
}

export interface ChatAttachment {
  id: string;
  fileName: string;
  filePath: string;
  sizeBytes: number;
  mimeType?: string;
  uploading?: boolean;
  failed?: boolean;
  error?: string;
}

export interface Message {
  id: string;
  threadId: string;
  role: "user" | "assistant";
  content?: string;
  blocks?: Array<Record<string, unknown>>;
  status: "completed" | "streaming" | "interrupted" | "error";
  createdAt: string;
  // 本地回显尚未与桌面端历史消息合并时为 true；失败后会移除该消息。
  localOnly?: boolean;
}

export interface MessageWindowCursor {
  createdAt: string;
  id: string;
  rowId?: number;
}

export interface MessageWindow {
  messages: Message[];
  nextCursor: MessageWindowCursor | null;
}

export interface DesktopStatus {
  version: string;
  online: boolean;
}

export interface ReasoningEffortOption {
  reasoningEffort: string;
  description: string;
}

export interface EngineModel {
  id: string;
  displayName: string;
  description: string;
  defaultReasoningEffort: string | null;
  supportedReasoningEfforts: ReasoningEffortOption[];
  isDefault: boolean;
  hidden?: boolean;
}

export interface EngineInfo {
  id: string;
  name: string;
  models: EngineModel[];
}

export interface RemoteEvent {
  version: number;
  kind: "event";
  event: string;
  // 设备级事件的目标手机 ID；消息已由连接来源携带 panesId。
  targetDeviceId?: string;
  payload: Record<string, unknown>;
}

export interface ThreadMessageCompletedPayload {
  // 完成消息所属的会话 ID。
  threadId: string;
  // 桌面端最终消息 ID，用于手机端去重。
  messageId: string;
  // 与 message.list 返回一致的最终助手消息。
  message: Message;
}
