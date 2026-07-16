import type {
  AgentSessionRuntime,
  CreateAgentSessionRuntimeFactory,
  SessionManager,
  ToolDefinition,
} from "@earendil-works/pi-coding-agent";
import {
  createAgentSessionFromServices,
  createAgentSessionRuntime,
  createAgentSessionServices,
  getAgentDir,
  SessionManager as SessionManagerCtor,
} from "@earendil-works/pi-coding-agent";
import type { WebSocket } from "ws";
import type { Logger, SessionState } from "./types.js";
import { forwardEvent } from "./messages.js";
import { createSendFileTool } from "./tools/send-file-tool.js";
import { createUiContext } from "./ui-context.js";
import { ExtensionUiChannel } from "./extension-ui.js";

function createRuntimeFactory(
  cwd: string,
  customTools: ToolDefinition[] = [],
): CreateAgentSessionRuntimeFactory {
  return async ({
    cwd: factoryCwd,
    agentDir,
    sessionManager,
    sessionStartEvent,
  }) => {
    const effectiveCwd = factoryCwd || cwd;
    const services = await createAgentSessionServices({
      cwd: effectiveCwd,
      agentDir,
    });
    return {
      ...(await createAgentSessionFromServices({
        services,
        sessionManager,
        sessionStartEvent,
        customTools,
      })),
      services,
      diagnostics: (services as { diagnostics?: unknown }).diagnostics,
    } as Awaited<ReturnType<CreateAgentSessionRuntimeFactory>>;
  };
}

function parseModelId(model: string): [string, string] | null {
  const parts = String(model).split(":");
  if (parts.length === 2 && parts[0] && parts[1]) {
    return [parts[0], parts[1]];
  }
  return null;
}

export interface SessionManagerDeps {
  agentDir: string;
  logger: Logger;
}

export class SessionStore {
  readonly #sessions = new Map<string, SessionState>();
  readonly #sendFileTool = createSendFileTool();
  readonly #agentDir: string;
  readonly #logger: Logger;

  constructor(deps: SessionManagerDeps) {
    this.#agentDir = deps.agentDir;
    this.#logger = deps.logger;
  }

  has(sessionId: string): boolean {
    return this.#sessions.has(sessionId);
  }

  get(sessionId: string): SessionState | undefined {
    return this.#sessions.get(sessionId);
  }

  *entries(): IterableIterator<[string, SessionState]> {
    yield* this.#sessions.entries();
  }

  subscribe(
    ws: WebSocket,
    sessionId: string,
    runtime: AgentSessionRuntime,
  ): () => void {
    const unsubscribe = runtime.session.subscribe(
      (event: Record<string, unknown>) => {
        forwardEvent(ws, sessionId, event);
      },
    );
    return unsubscribe;
  }

  async create(
    ws: WebSocket,
    opts: {
      sessionId: string;
      cwd?: string;
      agentDir?: string;
      sessionPath?: string;
      model?: string;
      thinkingLevel?: string;
    },
  ): Promise<SessionState> {
    const sessionId = opts.sessionId;
    const cwd = opts.cwd || process.cwd();
    const agentDir = opts.agentDir || this.#agentDir || getAgentDir();

    this.#logger.info(
      "create_session:",
      sessionId,
      opts.sessionPath,
      cwd,
      opts.model,
    );

    const sessionManager = this.#createSessionManager(cwd, opts.sessionPath);
    this.#logger.info("creating runtime...");
    const runtime = await createAgentSessionRuntime(
      createRuntimeFactory(cwd, [this.#sendFileTool]),
      {
        cwd,
        agentDir,
        sessionManager,
      },
    );
    const ui = new ExtensionUiChannel(ws, sessionId, this.#logger);
    await runtime.session.bindExtensions({
      uiContext: createUiContext(ui),
    });
    this.#logger.info(
      "runtime created, sessionFile:",
      runtime.session.sessionFile,
    );

    await this.#applyModel(runtime, opts.model);
    this.#applyThinkingLevel(runtime, opts.thinkingLevel);

    const unsubscribe = this.subscribe(ws, sessionId, runtime);
    const state: SessionState = {
      runtime,
      sessionManager,
      unsubscribe,
      cwd,
      agentDir,
      ui,
    };
    this.#sessions.set(sessionId, state);
    return state;
  }

  #createSessionManager(
    cwd: string,
    sessionPath: string | undefined,
  ): SessionManager {
    if (sessionPath) {
      try {
        this.#logger.info("opening session:", sessionPath);
        const sm = SessionManagerCtor.open(sessionPath);
        this.#logger.info("session opened:", sessionPath);
        return sm;
      } catch (e: unknown) {
        const err = e instanceof Error ? e.message : String(e);
        this.#logger.info(
          "failed to open session, creating new:",
          sessionPath,
          err,
        );
        return SessionManagerCtor.create(cwd);
      }
    }

    if (cwd) {
      this.#logger.info("creating session for cwd:", cwd);
      return SessionManagerCtor.create(cwd);
    }

    this.#logger.info("creating in-memory session");
    return SessionManagerCtor.inMemory();
  }

  async #applyModel(
    runtime: AgentSessionRuntime,
    model?: string,
  ): Promise<void> {
    if (!model) return;
    const parsed = parseModelId(model);
    if (!parsed) {
      this.#logger.warn("invalid model id:", model);
      return;
    }
    const [provider, modelId] = parsed;
    try {
      const resolved = runtime.session.modelRegistry.find(provider, modelId);
      if (resolved) {
        await runtime.session.setModel(resolved);
      } else {
        this.#logger.warn("model not found:", model);
      }
    } catch (e: unknown) {
      this.#logger.error("failed to set model:", e);
    }
  }

  #applyThinkingLevel(runtime: AgentSessionRuntime, level?: string): void {
    if (!level) return;
    try {
      runtime.session.setThinkingLevel(level as never);
    } catch (e: unknown) {
      this.#logger.error("failed to set thinking level:", e);
    }
  }

  resubscribe(ws: WebSocket, sessionId: string, state: SessionState): void {
    state.unsubscribe();
    state.unsubscribe = this.subscribe(ws, sessionId, state.runtime);
  }

  disposeAll(): void {
    for (const [, state] of this.#sessions.entries()) {
      try {
        state.ui.dispose();
        state.unsubscribe();
        state.runtime.dispose?.();
      } catch {
        // ignore
      }
    }
    this.#sessions.clear();
  }
}
