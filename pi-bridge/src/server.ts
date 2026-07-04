import path from "node:path";
import { WebSocketServer, WebSocket, type RawData } from "ws";
import type { AddressInfo } from "node:net";
import * as net from "node:net";
import { Type } from "typebox";
import { Value } from "typebox/value";
import {
  AuthStorage,
  ModelRegistry,
  SettingsManager,
} from "@earendil-works/pi-coding-agent";
import type {
  BridgeConfig,
  ConnectedClient,
  CreateSessionMessage,
  Logger,
  MessageQueues,
  WireMessage,
} from "./types.js";
import { sendError, sendResponse } from "./messages.js";
import { SessionStore } from "./session.js";
import {
  handleGetCommands,
  handleGetModel,
  handleGetModels,
  handleGetProviders,
  handleGetSettings,
  handleSetAuth,
  handleSetCompactionEnabled,
  handleSetDefaultModel,
  handleSetDefaultProvider,
  handleSetDefaultThinkingLevel,
  sessionCommands,
  type InboundMessage,
} from "./commands.js";

const CreateSessionSchema = Type.Object({
  type: Type.Literal("create_session"),
  sessionId: Type.String({ minLength: 1 }),
  cwd: Type.Optional(Type.String()),
  agentDir: Type.Optional(Type.String()),
  sessionPath: Type.Optional(Type.String()),
  model: Type.Optional(Type.String()),
  thinkingLevel: Type.Optional(Type.String()),
  id: Type.Optional(Type.String()),
});

export interface BridgeServerDeps {
  config: BridgeConfig;
  logger: Logger;
}

export class BridgeServer {
  readonly #config: BridgeConfig;
  readonly #modelRegistry: ModelRegistry;
  readonly #authStorage: AuthStorage;
  readonly #settingsManager: SettingsManager;
  readonly #logger: Logger;
  readonly #store: SessionStore;
  #wss: WebSocketServer | null = null;
  #client: WebSocket | null = null;

  constructor(deps: BridgeServerDeps) {
    this.#config = deps.config;
    this.#logger = deps.logger;

    const agentDir = this.#config.agentDir;
    const authStorage = AuthStorage.create(path.join(agentDir, "auth.json"));
    const modelRegistry = ModelRegistry.create(
      authStorage,
      path.join(agentDir, "models.json"),
    );
    this.#authStorage = authStorage;
    this.#modelRegistry = modelRegistry;
    this.#settingsManager = SettingsManager.create(process.cwd(), agentDir);

    this.#store = new SessionStore({
      agentDir,
      logger: deps.logger,
    });
  }

  async start(): Promise<void> {
    const port = this.#config.port || (await findFreePort());
    return new Promise((resolve) => {
      this.#wss = new WebSocketServer({ port, host: "127.0.0.1" }, () => {
        this.#logger.info("listening", port);
        console.log(`BRIDGE_PORT ${port}`);
        resolve();
      });

      this.#wss.on("connection", (ws) => this.#onConnection(ws));
      this.#wss.on("error", (err) => this.#logger.error("server error:", err));
    });
  }

  #onConnection(ws: WebSocket): void {
    if (this.#client) {
      this.#logger.warn("second client tried to connect; closing previous");
      this.#client.close();
    }
    this.#client = ws;
    this.#logger.info("client connected");

    const clientCtx: ConnectedClient = {
      socket: ws,
      queues: createMessageQueues(),
    };

    ws.on("message", (data) => this.#onMessage(clientCtx, data));
    ws.on("close", () => this.#onClose(ws));
    ws.on("error", (err) => this.#logger.error("websocket error:", err));
  }

  #onMessage(client: ConnectedClient, data: RawData): void {
    let msg: InboundMessage;
    try {
      msg = JSON.parse(String(data)) as InboundMessage;
    } catch (e: unknown) {
      const err = e instanceof Error ? e.message : String(e);
      this.#logger.error("failed to parse message:", err);
      sendError(client.socket, err);
      return;
    }

    this.#logger.info("recv:", msg.type, msg.sessionId || "-");
    enqueueMessage(client, msg, (m) => this.#dispatch(client.socket, m));
  }

  async #dispatch(ws: WebSocket, msg: InboundMessage): Promise<void> {
    const { type, sessionId } = msg;

    try {
      if (type === "create_session") {
        await this.#handleCreateSession(ws, msg);
        return;
      }

      if (type === "get_models") {
        await handleGetModels({
          ws,
          sessionId,
          msg,
          modelRegistry: this.#modelRegistry,
          authStorage: this.#authStorage,
          settingsManager: this.#settingsManager,
          logger: this.#logger,
        });
        return;
      }

      if (type === "get_providers") {
        await handleGetProviders({
          ws,
          sessionId,
          msg,
          modelRegistry: this.#modelRegistry,
          authStorage: this.#authStorage,
          settingsManager: this.#settingsManager,
          logger: this.#logger,
        });
        return;
      }

      if (type === "set_auth") {
        await handleSetAuth({
          ws,
          sessionId,
          msg,
          modelRegistry: this.#modelRegistry,
          authStorage: this.#authStorage,
          settingsManager: this.#settingsManager,
          logger: this.#logger,
        });
        return;
      }

      if (type === "get_settings") {
        await handleGetSettings({
          ws,
          sessionId,
          msg,
          modelRegistry: this.#modelRegistry,
          authStorage: this.#authStorage,
          settingsManager: this.#settingsManager,
          logger: this.#logger,
        });
        return;
      }

      if (type === "set_compaction_enabled") {
        await handleSetCompactionEnabled({
          ws,
          sessionId,
          msg,
          modelRegistry: this.#modelRegistry,
          authStorage: this.#authStorage,
          settingsManager: this.#settingsManager,
          logger: this.#logger,
        });
        return;
      }

      if (type === "set_default_thinking_level") {
        await handleSetDefaultThinkingLevel({
          ws,
          sessionId,
          msg,
          modelRegistry: this.#modelRegistry,
          authStorage: this.#authStorage,
          settingsManager: this.#settingsManager,
          logger: this.#logger,
        });
        return;
      }

      if (type === "set_default_model") {
        await handleSetDefaultModel({
          ws,
          sessionId,
          msg,
          modelRegistry: this.#modelRegistry,
          authStorage: this.#authStorage,
          settingsManager: this.#settingsManager,
          logger: this.#logger,
        });
        return;
      }

      if (type === "set_default_provider") {
        await handleSetDefaultProvider({
          ws,
          sessionId,
          msg,
          modelRegistry: this.#modelRegistry,
          authStorage: this.#authStorage,
          settingsManager: this.#settingsManager,
          logger: this.#logger,
        });
        return;
      }

      if (type === "get_model") {
        const state = sessionId ? this.#store.get(sessionId) : undefined;
        await handleGetModel(
          {
            ws,
            sessionId,
            msg,
            modelRegistry: this.#modelRegistry,
            authStorage: this.#authStorage,
            settingsManager: this.#settingsManager,
            logger: this.#logger,
          },
          state,
        );
        return;
      }

      if (type === "get_commands") {
        await handleGetCommands(
          {
            ws,
            sessionId,
            msg,
            modelRegistry: this.#modelRegistry,
            authStorage: this.#authStorage,
            settingsManager: this.#settingsManager,
            logger: this.#logger,
          },
          this.#store,
          this.#config.agentDir,
        );
        return;
      }

      if (!sessionId) {
        sendError(ws, "missing sessionId");
        return;
      }

      const state = this.#store.get(sessionId);
      if (!state) {
        this.#logger.warn("session not found for command:", type, sessionId);
        sendResponse(
          ws,
          sessionId,
          type,
          msg.id,
          false,
          undefined,
          "session not found",
        );
        return;
      }

      const handler = sessionCommands.get(type);
      if (!handler) {
        this.#logger.warn("unknown command:", type);
        sendResponse(
          ws,
          sessionId,
          type,
          msg.id,
          false,
          undefined,
          "unknown command",
        );
        return;
      }

      await handler({
        ws,
        sessionId,
        state,
        msg,
        store: this.#store,
        modelRegistry: this.#modelRegistry,
        logger: this.#logger,
      });
    } catch (e: unknown) {
      const err = e instanceof Error ? e.message : String(e);
      this.#logger.error("command failed:", type, err);
      if (sessionId) {
        sendResponse(ws, sessionId, type, msg.id, false, undefined, err);
      } else {
        sendError(ws, err);
      }
    }
  }

  async #handleCreateSession(
    ws: WebSocket,
    msg: InboundMessage,
  ): Promise<void> {
    if (!Value.Check(CreateSessionSchema, msg)) {
      sendError(ws, "invalid create_session message");
      return;
    }

    const createMsg = msg as CreateSessionMessage;
    const sessionId = createMsg.sessionId;
    const existing = this.#store.get(sessionId);

    if (existing) {
      // Session already exists; keep it running and just acknowledge the request.
      // This makes create_session idempotent on the Rust side.
      sendResponse(ws, sessionId, "create_session", createMsg.id, true, {
        sessionId,
        sessionFile: existing.runtime.session.sessionFile,
      });
      return;
    }

    const state = await this.#store.create(ws, {
      sessionId,
      cwd: createMsg.cwd,
      agentDir: createMsg.agentDir || this.#config.agentDir,
      sessionPath: createMsg.sessionPath,
      model: createMsg.model,
      thinkingLevel: createMsg.thinkingLevel,
    });

    sendResponse(ws, sessionId, "create_session", createMsg.id, true, {
      sessionId,
      sessionFile: state.runtime.session.sessionFile,
    });
  }

  #onClose(ws: WebSocket): void {
    this.#logger.info("client disconnected");
    if (this.#client === ws) {
      this.#client = null;
    }
    this.#store.disposeAll();
    // Exit the bridge when the GUI disconnects so we do not leak processes.
    setTimeout(() => {
      this.#logger.info("exiting after disconnect");
      process.exit(0);
    }, 500);
  }
}

type MessageHandler = (msg: WireMessage) => Promise<void>;

function createMessageQueues(): MessageQueues {
  return {
    global: Promise.resolve(),
    sessions: new Map(),
  };
}

function enqueueMessage(
  client: ConnectedClient,
  msg: WireMessage,
  handler: MessageHandler,
): void {
  const run = async (): Promise<void> => {
    try {
      await handler(msg);
    } catch (err) {
      // The handler is expected to catch its own errors and send a response.
      // If it throws, log is the best we can do; do not crash the queue.
      console.error("[pi-bridge] unhandled message error:", err);
    }
  };

  const sid = msg.sessionId;

  if (msg.type === "create_session") {
    // create_session establishes a new session, so it must run on the
    // global queue to avoid racing with its own session queue creation.
    // It also seeds that session's queue so subsequent per-session
    // commands wait until the session is actually registered.
    const createPromise = client.queues.global.then(run);
    client.queues.global = createPromise;
    if (sid) {
      const prevSession = client.queues.sessions.get(sid) ?? Promise.resolve();
      client.queues.sessions.set(
        sid,
        prevSession.then(() => createPromise),
      );
    }
    return;
  }

  if (sid) {
    const prev = client.queues.sessions.get(sid) ?? Promise.resolve();
    const next = prev.then(run);
    client.queues.sessions.set(sid, next);
    next.finally(() => {
      if (client.queues.sessions.get(sid) === next) {
        client.queues.sessions.delete(sid);
      }
    });
    return;
  }

  client.queues.global = client.queues.global.then(run);
}

async function findFreePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const server = net.createServer();
    server.listen(0, "127.0.0.1", () => {
      const addr = server.address() as AddressInfo | null;
      const port = addr?.port ?? 0;
      server.close((err) => {
        if (err) {
          reject(err);
        } else {
          resolve(port);
        }
      });
    });
    server.on("error", reject);
  });
}
