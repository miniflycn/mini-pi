import * as fs from "node:fs";
import type { WebSocket } from "ws";
import type { SessionManager } from "@earendil-works/pi-coding-agent";
import type {
  ErrorPayload,
  ResponsePayload,
  WireMessage,
} from "./types.js";

export interface ChatContentBlock {
  type: string;
  text?: string;
  thinking?: string;
  name?: string;
  arguments?: unknown;
  data?: string;
  mimeType?: string;
}

export interface ChatMessage {
  id?: string;
  role: string;
  content: ChatContentBlock[];
  toolName?: string;
}

interface SdkMessage {
  role?: string;
  id?: string;
  content?: string | SdkContentBlock[];
  toolName?: string;
  toolCallId?: string;
}

interface SdkContentBlock {
  type?: string;
  text?: string;
  thinking?: string;
  name?: string;
  arguments?: unknown;
  data?: string;
  mimeType?: string;
}

interface MessageWrapperEntry {
  type: "message";
  id?: string;
  message: SdkMessage;
}

interface BashExecutionEntry {
  type: "bashExecution";
  id?: string;
  command?: string;
  output?: string;
  exitCode?: number;
}

type SessionEntry = MessageWrapperEntry | BashExecutionEntry | SdkMessage;

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isSdkMessage(value: unknown): value is SdkMessage {
  return isObject(value) && typeof value.role === "string";
}

function isMessageWrapperEntry(value: unknown): value is MessageWrapperEntry {
  return (
    isObject(value) &&
    value.type === "message" &&
    isSdkMessage(value.message)
  );
}

function isBashExecutionEntry(value: unknown): value is BashExecutionEntry {
  return isObject(value) && value.type === "bashExecution";
}

function parseSessionEntry(value: unknown): SessionEntry | null {
  if (isMessageWrapperEntry(value)) return value;
  if (isBashExecutionEntry(value)) return value;
  if (isSdkMessage(value)) return value;
  return null;
}

function normalizeUserContent(content: string | SdkContentBlock[] | undefined): ChatContentBlock[] {
  if (typeof content === "string") {
    return [{ type: "text", text: content }];
  }
  if (Array.isArray(content)) {
    return content.map((block) => {
      if (block.type === "image") {
        return {
          type: "image",
          data: block.data,
          mimeType: block.mimeType,
        };
      }
      return {
        type: String(block.type ?? "text"),
        text: block.text,
      };
    });
  }
  return [];
}

function normalizeAssistantContent(content: string | SdkContentBlock[] | undefined): ChatContentBlock[] {
  const parts: ChatContentBlock[] = [];
  if (Array.isArray(content)) {
    for (const block of content) {
      const type = block.type;
      if (type === "text") {
        parts.push({ type: "text", text: block.text ?? "" });
      } else if (type === "thinking") {
        parts.push({ type: "thinking", thinking: block.thinking ?? "" });
      } else if (type === "toolCall") {
        parts.push({
          type: "toolCall",
          name: block.name ?? "",
          arguments: block.arguments,
        });
      }
    }
  } else if (typeof content === "string") {
    parts.push({ type: "text", text: content });
  }
  return parts;
}

export function agentMessageToChat(entryId: string, msg: SdkMessage): ChatMessage | null {
  const role = msg.role;
  if (role === "user") {
    return {
      id: entryId,
      role: "user",
      content: normalizeUserContent(msg.content),
    };
  }

  if (role === "assistant") {
    return {
      id: entryId,
      role: "assistant",
      content: normalizeAssistantContent(msg.content),
    };
  }

  if (role === "toolResult") {
    return {
      id: entryId,
      role: "toolResult",
      toolName: msg.toolName || msg.toolCallId || "",
      content: normalizeAssistantContent(msg.content),
    };
  }

  // Bash execution messages are custom types in the SDK; skip unknowns here.
  return null;
}

export async function readSessionJsonl(sessionPath: string): Promise<ChatMessage[]> {
  const messages: ChatMessage[] = [];
  if (!fs.existsSync(sessionPath)) {
    return messages;
  }

  const text = await fs.promises.readFile(sessionPath, "utf8");
  for (const line of text.split("\n")) {
    const trimmed = line.trim();
    if (!trimmed) continue;
    try {
      const parsed = JSON.parse(trimmed);
      const entry = parseSessionEntry(parsed);
      if (!entry) continue;

      if (isBashExecutionEntry(entry)) {
        // Legacy RPC mode top-level bash execution entry.
        messages.push({
          id: entry.id,
          role: "bashExecution",
          command: entry.command ?? "",
          output: entry.output,
          exitCode: entry.exitCode,
        } as unknown as ChatMessage);
        continue;
      }

      // Newer pi session files wrap every entry in `type: "message"`
      // with the actual message stored in `entry.message`.
      const message = isMessageWrapperEntry(entry) ? entry.message : entry;
      const id = isMessageWrapperEntry(entry) ? entry.id : message.id;

      const role = message.role;
      if (!role) continue;

      if (role === "user") {
        messages.push({
          id,
          role: "user",
          content: normalizeUserContent(message.content),
        });
      } else if (role === "assistant") {
        messages.push({
          id,
          role: "assistant",
          content: normalizeAssistantContent(message.content),
        });
      } else if (role === "toolResult") {
        messages.push({
          id,
          role: "toolResult",
          toolName: message.toolName || message.toolCallId || "",
          content: normalizeAssistantContent(message.content),
        });
      }
    } catch {
      // ignore malformed lines
    }
  }
  return messages;
}

export async function getMessagesFromSession(
  sessionManager: SessionManager,
  sessionFile: string | undefined,
  fallbackMessages: unknown[]
): Promise<ChatMessage[]> {
  // Prefer SessionManager.getBranch() so we only return the current leaf path,
  // matching how the pi TUI renders branched sessions.
  if (typeof sessionManager.getBranch === "function") {
    const branch = sessionManager.getBranch() as unknown[];
    const messages: ChatMessage[] = [];
    for (const raw of branch) {
      const entry = parseSessionEntry(raw);
      if (isMessageWrapperEntry(entry)) {
        const wired = agentMessageToChat(entry.id ?? "", entry.message);
        if (wired) {
          messages.push(wired);
        }
      }
    }
    return messages;
  }

  if (sessionFile && fs.existsSync(sessionFile)) {
    return readSessionJsonl(sessionFile);
  }

  return fallbackMessages as ChatMessage[];
}

// ---------------------------------------------------------------------------
// WebSocket wire helpers
// ---------------------------------------------------------------------------

export function send(ws: WebSocket, message: WireMessage | ResponsePayload | ErrorPayload): void {
  if (ws.readyState === ws.OPEN) {
    ws.send(JSON.stringify(message));
  }
}

export function sendError(ws: WebSocket, error: string): void {
  send(ws, { type: "error", error });
}

export function sendResponse(
  ws: WebSocket,
  sessionId: string,
  command: string,
  requestId: string | undefined,
  success: boolean,
  data?: unknown,
  error?: string
): void {
  send(ws, {
    type: "response",
    sessionId,
    command,
    id: requestId,
    success,
    data,
    error,
  });
}

export function forwardEvent(ws: WebSocket, sessionId: string, event: Record<string, unknown>): void {
  send(ws, { sessionId, ...event } as WireMessage);
}
