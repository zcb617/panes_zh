export interface PairingConfig {
  version: 1;
  endpoint: string;
  tunnel_id: string;
  relay_credential: string;
  pairing_token?: string;
  device_credential?: string;
  expires_at?: string;
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
}

export interface Message {
  id: string;
  threadId: string;
  role: "user" | "assistant";
  content?: string;
  blocks?: Array<Record<string, unknown>>;
  status: "completed" | "streaming" | "interrupted" | "error";
  createdAt: string;
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
  payload: Record<string, unknown>;
}
