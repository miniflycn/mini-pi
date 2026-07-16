import type { WebSocket } from "ws";
import type {
  AgentSessionRuntime,
  SessionManager,
} from "@earendil-works/pi-coding-agent";
import type { ExtensionUiChannel } from "./extension-ui.js";

export interface BridgeConfig {
  port: number;
  agentDir: string;
}

export interface SessionState {
  runtime: AgentSessionRuntime;
  sessionManager: SessionManager;
  unsubscribe: () => void;
  cwd: string;
  agentDir: string;
  ui: ExtensionUiChannel;
}

export interface Logger {
  debug(...args: unknown[]): void;
  info(...args: unknown[]): void;
  warn(...args: unknown[]): void;
  error(...args: unknown[]): void;
}

export interface WireMessage {
  type: string;
  sessionId?: string;
  id?: string;
  [key: string]: unknown;
}

export interface CreateSessionMessage extends WireMessage {
  type: "create_session";
  sessionId: string;
  cwd?: string;
  agentDir?: string;
  sessionPath?: string;
  model?: string;
  thinkingLevel?: string;
}

export interface ResponsePayload {
  type: "response";
  sessionId: string;
  command: string;
  id?: string;
  success: boolean;
  data?: unknown;
  error?: string;
}

export interface ErrorPayload {
  type: "error";
  error: string;
}

export interface ConnectedClient {
  socket: WebSocket;
  queues: MessageQueues;
}

export interface MessageQueues {
  global: Promise<void>;
  sessions: Map<string, Promise<void>>;
}

export interface ModelWireInfo {
  provider: string;
  id: string;
  name: string;
  thinkingLevelMap?: Record<string, string>;
}

export interface ProviderWireInfo {
  id: string;
  name: string;
  configured: boolean;
}

export interface CommandWireInfo {
  name: string;
  description?: string;
  source: string;
}
