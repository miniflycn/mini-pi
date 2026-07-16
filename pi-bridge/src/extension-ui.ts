import type { WebSocket } from "ws";
import { send } from "./messages.js";
import type { Logger } from "./types.js";

/**
 * Payload the GUI sends back in an `extension_ui_response`.
 *
 * Mirrors the SDK's RPC-mode dialog protocol: a dialog that was answered
 * carries `confirmed`/`value`, a dismissed one carries `cancelled: true`.
 */
export interface UiResponsePayload {
  confirmed?: boolean;
  cancelled?: boolean;
  value?: string;
}

/** Options accepted by an interactive UI request (timeout / programmatic abort). */
export interface UiRequestOptions {
  signal?: AbortSignal;
  timeout?: number;
}

/**
 * Per-session transport for extension UI interactions.
 *
 * The bridge has no UI of its own; it forwards `extension_ui_request` messages
 * to the Rust GUI over the shared WebSocket and, for interactive requests,
 * awaits the matching `extension_ui_response`. This is the same wire protocol
 * the SDK uses in RPC mode, so the Rust side already understands it.
 */
export class ExtensionUiChannel {
  readonly #socket: WebSocket;
  readonly #sessionId: string;
  readonly #logger: Logger;
  readonly #pending = new Map<string, (payload: UiResponsePayload) => void>();

  constructor(socket: WebSocket, sessionId: string, logger: Logger) {
    this.#socket = socket;
    this.#sessionId = sessionId;
    this.#logger = logger;
  }

  /** Fire-and-forget UI request (e.g. notify); no response is expected. */
  emit(method: string, params: Record<string, unknown>): void {
    send(this.#socket, {
      type: "extension_ui_request",
      sessionId: this.#sessionId,
      id: crypto.randomUUID(),
      method,
      ...params,
    });
  }

  /**
   * Interactive UI request (e.g. confirm). Resolves when the GUI replies with
   * an `extension_ui_response`, or with `{ cancelled: true }` if the request
   * times out, is aborted, or the socket is not connected.
   */
  request(
    method: string,
    params: Record<string, unknown>,
    opts?: UiRequestOptions,
  ): Promise<UiResponsePayload> {
    const id = crypto.randomUUID();
    return new Promise<UiResponsePayload>((resolve) => {
      let settled = false;
      let timer: ReturnType<typeof setTimeout> | undefined;

      const finish = (payload: UiResponsePayload): void => {
        if (settled) return;
        settled = true;
        this.#pending.delete(id);
        if (timer) clearTimeout(timer);
        opts?.signal?.removeEventListener("abort", onAbort);
        resolve(payload);
      };

      const onAbort = (): void => finish({ cancelled: true });

      if (opts?.signal?.aborted) {
        finish({ cancelled: true });
        return;
      }
      opts?.signal?.addEventListener("abort", onAbort);
      if (opts?.timeout && opts.timeout > 0) {
        timer = setTimeout(() => finish({ cancelled: true }), opts.timeout);
      }

      this.#pending.set(id, finish);

      if (this.#socket.readyState !== this.#socket.OPEN) {
        this.#logger.warn("ui request with no open socket:", method);
        finish({ cancelled: true });
        return;
      }

      send(this.#socket, {
        type: "extension_ui_request",
        sessionId: this.#sessionId,
        id,
        method,
        ...params,
      });
    });
  }

  /** Resolve a pending interactive request with the GUI's response. */
  resolve(id: string, payload: UiResponsePayload): void {
    const pending = this.#pending.get(id);
    if (pending) {
      pending(payload);
    } else {
      this.#logger.warn("no pending ui request for id:", id);
    }
  }

  /** Cancel every in-flight request (called when the session is disposed). */
  dispose(): void {
    for (const pending of this.#pending.values()) {
      pending({ cancelled: true });
    }
    this.#pending.clear();
  }
}
