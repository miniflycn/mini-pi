import type { WebSocket } from "ws";
import {
  DefaultResourceLoader,
  type AuthStorage,
  type ModelRegistry,
  type SettingsManager,
} from "@earendil-works/pi-coding-agent";
import { Type, type Static, type TSchema } from "typebox";
import { Value } from "typebox/value";
import type { Logger, SessionState, CommandWireInfo, ModelWireInfo } from "./types.js";
import { sendResponse, getMessagesFromSession } from "./messages.js";
import type { SessionStore } from "./session.js";

/** Routing id used for responses that are not tied to a real session. */
const GLOBAL_SESSION_ID = "bridge";

const BaseMessageSchema = Type.Object({
  type: Type.String(),
  sessionId: Type.Optional(Type.String()),
  id: Type.Optional(Type.String()),
});

const PromptSchema = Type.Intersect([
  BaseMessageSchema,
  Type.Object({
    type: Type.Literal("prompt"),
    message: Type.String(),
    images: Type.Optional(Type.Array(Type.Unknown())),
    streamingBehavior: Type.Optional(Type.String()),
  }),
]);

const SteerSchema = Type.Intersect([
  BaseMessageSchema,
  Type.Object({
    type: Type.Literal("steer"),
    message: Type.String(),
  }),
]);

const FollowUpSchema = Type.Intersect([
  BaseMessageSchema,
  Type.Object({
    type: Type.Literal("follow_up"),
    message: Type.String(),
  }),
]);

const SetModelSchema = Type.Intersect([
  BaseMessageSchema,
  Type.Object({
    type: Type.Literal("set_model"),
    provider: Type.String(),
    modelId: Type.String(),
  }),
]);

const SetThinkingLevelSchema = Type.Intersect([
  BaseMessageSchema,
  Type.Object({
    type: Type.Literal("set_thinking_level"),
    level: Type.String(),
  }),
]);

const NavigateTreeSchema = Type.Intersect([
  BaseMessageSchema,
  Type.Object({
    type: Type.Literal("navigate_tree"),
    entryId: Type.String(),
  }),
]);

const ForkSchema = Type.Intersect([
  BaseMessageSchema,
  Type.Object({
    type: Type.Literal("fork"),
    entryId: Type.String(),
  }),
]);

const CloneSchema = Type.Intersect([
  BaseMessageSchema,
  Type.Object({
    type: Type.Literal("clone"),
    entryId: Type.Optional(Type.String()),
  }),
]);

const ExportHtmlSchema = Type.Intersect([
  BaseMessageSchema,
  Type.Object({
    type: Type.Literal("export_html"),
    outputPath: Type.Optional(Type.String()),
  }),
]);

const CompactSchema = Type.Intersect([
  BaseMessageSchema,
  Type.Object({
    type: Type.Literal("compact"),
    customInstructions: Type.Optional(Type.String()),
  }),
]);

const GetModelSchema = Type.Intersect([
  BaseMessageSchema,
  Type.Object({
    type: Type.Literal("get_model"),
    provider: Type.Optional(Type.String()),
    modelId: Type.Optional(Type.String()),
  }),
]);

const GetSkillsSchema = Type.Intersect([
  BaseMessageSchema,
  Type.Object({
    type: Type.Literal("get_skills"),
  }),
]);

const GetExtensionsSchema = Type.Intersect([
  BaseMessageSchema,
  Type.Object({
    type: Type.Literal("get_extensions"),
  }),
]);

const GetPromptsSchema = Type.Intersect([
  BaseMessageSchema,
  Type.Object({
    type: Type.Literal("get_prompts"),
  }),
]);

const GetProvidersSchema = Type.Intersect([
  BaseMessageSchema,
  Type.Object({
    type: Type.Literal("get_providers"),
  }),
]);

const SetAuthSchema = Type.Intersect([
  BaseMessageSchema,
  Type.Object({
    type: Type.Literal("set_auth"),
    provider: Type.String(),
    key: Type.String(),
  }),
]);

const GetSettingsSchema = Type.Intersect([
  BaseMessageSchema,
  Type.Object({
    type: Type.Literal("get_settings"),
  }),
]);

const SetCompactionEnabledSchema = Type.Intersect([
  BaseMessageSchema,
  Type.Object({
    type: Type.Literal("set_compaction_enabled"),
    enabled: Type.Boolean(),
  }),
]);

const SetDefaultThinkingLevelSchema = Type.Intersect([
  BaseMessageSchema,
  Type.Object({
    type: Type.Literal("set_default_thinking_level"),
    level: Type.Union([
      Type.Literal("off"),
      Type.Literal("minimal"),
      Type.Literal("low"),
      Type.Literal("medium"),
      Type.Literal("high"),
      Type.Literal("xhigh"),
    ]),
  }),
]);

const SetDefaultModelSchema = Type.Intersect([
  BaseMessageSchema,
  Type.Object({
    type: Type.Literal("set_default_model"),
    modelId: Type.String(),
  }),
]);

const SetDefaultProviderSchema = Type.Intersect([
  BaseMessageSchema,
  Type.Object({
    type: Type.Literal("set_default_provider"),
    provider: Type.String(),
  }),
]);

const GetSessionStatsSchema = Type.Intersect([
  BaseMessageSchema,
  Type.Object({
    type: Type.Literal("get_session_stats"),
  }),
]);

const ExtensionUiResponseSchema = Type.Intersect([
  BaseMessageSchema,
  Type.Object({
    type: Type.Literal("extension_ui_response"),
    confirmed: Type.Optional(Type.Boolean()),
    cancelled: Type.Optional(Type.Boolean()),
    value: Type.Optional(Type.String()),
  }),
]);

export type InboundMessage = Static<typeof BaseMessageSchema>;

export interface CommandContext {
  ws: WebSocket;
  sessionId: string;
  state: SessionState;
  msg: InboundMessage;
  store: SessionStore;
  modelRegistry: ModelRegistry;
  logger: Logger;
}

export interface GlobalCommandContext {
  ws: WebSocket;
  sessionId: string | undefined;
  msg: InboundMessage;
  modelRegistry: ModelRegistry;
  authStorage: AuthStorage;
  settingsManager: SettingsManager;
  logger: Logger;
}

export type SessionCommandHandler = (ctx: CommandContext) => Promise<void>;
export type GlobalCommandHandler = (ctx: GlobalCommandContext) => Promise<void>;

interface ResourceLoader {
  reload?: () => Promise<void> | void;
  getSkills?: () => Promise<unknown> | unknown;
  getExtensions?: () => Promise<unknown> | unknown;
  getPrompts?: () => Promise<unknown> | unknown;
}

function getResourceLoader(
  runtime: unknown,
): ResourceLoader | undefined {
  const services = (runtime as Record<string, unknown> | undefined)?.services;
  const loader =
    services && typeof services === "object"
      ? (services as Record<string, unknown>).resourceLoader
      : undefined;
  return loader as ResourceLoader | undefined;
}

function assertShape<T extends TSchema>(msg: InboundMessage, schema: T): Static<T> {
  if (!Value.Check(schema, msg)) {
    throw new Error("invalid message shape");
  }
  return msg as Static<T>;
}

export async function handlePrompt(ctx: CommandContext): Promise<void> {
  const { ws, sessionId, state, msg, logger } = ctx;
  const parsed = assertShape(msg, PromptSchema);
  const options: {
    images?: unknown[];
    streamingBehavior?: string;
    preflightResult?: (success: boolean) => void;
  } = {};
  if (parsed.images && parsed.images.length > 0) {
    options.images = parsed.images;
  }
  if (parsed.streamingBehavior) {
    options.streamingBehavior = parsed.streamingBehavior;
  }

  // Do NOT await the full turn — that would block the session message queue
  // and prevent abort/steer/follow_up from being handled while streaming.
  // The SDK streams events via the session subscription; we only need to
  // acknowledge once preflight succeeds (or fail early).
  let preflightSucceeded = false;
  options.preflightResult = (success: boolean) => {
    preflightSucceeded = success;
    if (success) {
      sendResponse(ws, sessionId, "prompt", parsed.id, true);
    }
  };

  state.runtime.session
    .prompt(parsed.message, options as never)
    .catch((e: unknown) => {
      const err = e instanceof Error ? e.message : String(e);
      logger.error("[bridge] prompt failed:", sessionId, err);
      if (!preflightSucceeded) {
        sendResponse(ws, sessionId, "prompt", parsed.id, false, undefined, err);
      }
    });
}

export async function handleSteer(ctx: CommandContext): Promise<void> {
  const { ws, sessionId, state, msg } = ctx;
  const parsed = assertShape(msg, SteerSchema);
  await state.runtime.session.steer(parsed.message);
  sendResponse(ws, sessionId, "steer", parsed.id, true);
}

export async function handleFollowUp(ctx: CommandContext): Promise<void> {
  const { ws, sessionId, state, msg } = ctx;
  const parsed = assertShape(msg, FollowUpSchema);
  await state.runtime.session.followUp(parsed.message);
  sendResponse(ws, sessionId, "follow_up", parsed.id, true);
}

export async function handleAbort(ctx: CommandContext): Promise<void> {
  const { ws, sessionId, state, msg } = ctx;
  await state.runtime.session.abort();
  sendResponse(ws, sessionId, "abort", msg.id, true);
}

export async function handleSetModel(ctx: CommandContext): Promise<void> {
  const { ws, sessionId, state, msg, modelRegistry } = ctx;
  const parsed = assertShape(msg, SetModelSchema);
  const model = modelRegistry.find(parsed.provider, parsed.modelId);
  if (!model) {
    throw new Error(`model not found: ${parsed.provider}:${parsed.modelId}`);
  }
  await state.runtime.session.setModel(model);
  sendResponse(ws, sessionId, "set_model", parsed.id, true);
}

export async function handleSetThinkingLevel(ctx: CommandContext): Promise<void> {
  const { ws, sessionId, state, msg } = ctx;
  const parsed = assertShape(msg, SetThinkingLevelSchema);
  state.runtime.session.setThinkingLevel(parsed.level as never);
  sendResponse(ws, sessionId, "set_thinking_level", parsed.id, true);
}

export async function handleNewSession(ctx: CommandContext): Promise<void> {
  const { ws, sessionId, state, msg, store } = ctx;
  await state.runtime.newSession();
  store.resubscribe(ws, sessionId, state);
  sendResponse(ws, sessionId, "new_session", msg.id, true);
}

export async function handleNavigateTree(ctx: CommandContext): Promise<void> {
  const { ws, sessionId, state, msg, logger } = ctx;
  const parsed = assertShape(msg, NavigateTreeSchema);
  logger.info("navigate_tree:", parsed.entryId);
  const result = await state.runtime.session.navigateTree(parsed.entryId);
  logger.info("navigate_tree result:", result);
  sendResponse(ws, sessionId, "navigate_tree", parsed.id, true, result);
}

export async function handleFork(ctx: CommandContext): Promise<void> {
  const { ws, sessionId, state, msg, store } = ctx;
  const parsed = assertShape(msg, ForkSchema);
  await state.runtime.fork(parsed.entryId);
  store.resubscribe(ws, sessionId, state);
  sendResponse(ws, sessionId, "fork", parsed.id, true);
}

export async function handleClone(ctx: CommandContext): Promise<void> {
  const { ws, sessionId, state, msg, store } = ctx;
  const parsed = assertShape(msg, CloneSchema);
  await state.runtime.fork(parsed.entryId || "", { position: "at" });
  store.resubscribe(ws, sessionId, state);
  sendResponse(ws, sessionId, "clone", parsed.id, true);
}

export async function handleGetMessages(ctx: CommandContext): Promise<void> {
  const { ws, sessionId, state, msg, logger } = ctx;
  const sessionFile = state.runtime.session.sessionFile;
  logger.info("get_messages:", sessionId, "sessionFile:", sessionFile);
  try {
    const fallback = (state.runtime.session.agent?.state?.messages as unknown[]) ?? [];
    const messages = await getMessagesFromSession(state.sessionManager, sessionFile, fallback);
    sendResponse(ws, sessionId, "get_messages", msg.id, true, { messages });
  } catch (e: unknown) {
    const err = e instanceof Error ? e.message : String(e);
    logger.error("get_messages failed:", err);
    sendResponse(ws, sessionId, "get_messages", msg.id, false, undefined, err);
  }
}

/**
 * Return the effective slash-command list.
 *
 * If `sessionId` refers to a live bridge session, this uses that session's
 * `resourceLoader` and `extensionRunner` so extension-registered commands are
 * included. Otherwise it creates a standalone `DefaultResourceLoader` for the
 * global no-session path (used at app startup).
 */
export async function handleGetCommands(
  ctx: GlobalCommandContext,
  store: SessionStore,
  agentDir: string,
): Promise<void> {
  const { ws, sessionId, msg } = ctx;

  let loader: ResourceLoader | undefined;
  let extensionRunner: unknown;
  if (sessionId) {
    const state = store.get(sessionId);
    if (state) {
      loader = state.runtime.session.resourceLoader;
      extensionRunner = (state.runtime.session as { extensionRunner?: unknown })
        .extensionRunner;
    }
  }

  if (!loader) {
    loader = new DefaultResourceLoader({
      cwd: process.cwd(),
      agentDir,
    });
    extensionRunner = undefined;
  }

  // reload() is optional on the ResourceLoader contract; static loaders can
  // skip it safely.
  await loader.reload?.();
  // getPrompts / getSkills may return either a plain object or a Promise;
  // wrap them so we always await the result before iterating.
  const promptsResult = (await Promise.resolve(
    loader.getPrompts?.() ?? { prompts: [] },
  )) as { prompts?: unknown[] };
  const skillsResult = (await Promise.resolve(
    loader.getSkills?.() ?? { skills: [] },
  )) as { skills?: unknown[] };
  const prompts = promptsResult.prompts ?? [];
  const skills = skillsResult.skills ?? [];
  const commands: CommandWireInfo[] = [];

  // Extension commands (pi.registerCommand) live in the session's extension runner.
  const registeredCommands = Array.isArray(
    (extensionRunner as { getRegisteredCommands?: () => unknown[] } | undefined)
      ?.getRegisteredCommands?.(),
  )
    ? (extensionRunner as { getRegisteredCommands: () => unknown[] }).getRegisteredCommands()
    : [];
  for (const cmd of registeredCommands) {
    const c = cmd as { invocationName?: string; description?: string };
    if (c.invocationName) {
      commands.push({
        name: c.invocationName,
        description: c.description,
        source: "extension",
      });
    }
  }

  for (const prompt of prompts) {
    const p = prompt as { name: string; description?: string; source?: string };
    commands.push({
      name: p.name,
      description: p.description,
      source: p.source || "prompt",
    });
  }
  for (const skill of skills) {
    const s = skill as { name: string; description?: string };
    commands.push({
      name: s.name,
      description: s.description,
      source: "skill",
    });
  }
  sendResponse(ws, sessionId || GLOBAL_SESSION_ID, "get_commands", msg.id, true, { commands });
}

function normalizeResourceArray(raw: unknown): unknown[] {
  if (Array.isArray(raw)) {
    return raw;
  }
  if (raw && typeof raw === "object") {
    const obj = raw as Record<string, unknown>;
    if (Array.isArray(obj.skills)) return obj.skills;
    if (Array.isArray(obj.extensions)) return obj.extensions;
    if (Array.isArray(obj.prompts)) return obj.prompts;
  }
  return [];
}

function extractResourceName(item: unknown): string | undefined {
  if (item && typeof item === "object") {
    const obj = item as Record<string, unknown>;
    if (typeof obj.name === "string") return obj.name;
    if (typeof obj.title === "string") return obj.title;
    if (typeof obj.id === "string") return obj.id;
    if (typeof obj.path === "string") return obj.path;
    if (typeof obj.entryPoint === "string") return obj.entryPoint;
  }
  return undefined;
}

function extractResourceDescription(item: unknown): string | undefined {
  if (item && typeof item === "object") {
    const obj = item as Record<string, unknown>;
    if (typeof obj.description === "string") return obj.description;
  }
  return undefined;
}

function normalizeResourceItems(raw: unknown): unknown[] {
  return normalizeResourceArray(raw).map((item) => ({
    name: extractResourceName(item) ?? "Unnamed",
    description: extractResourceDescription(item) ?? "",
    raw: item,
  }));
}

export async function handleGetSkills(ctx: CommandContext): Promise<void> {
  const { ws, sessionId, state, msg } = ctx;
  const loader = getResourceLoader(state.runtime);
  if (!loader?.getSkills) {
    sendResponse(
      ws,
      sessionId,
      "get_skills",
      msg.id,
      false,
      undefined,
      "resource loader or getSkills not available",
    );
    return;
  }
  await loader.reload?.();
  const raw = await loader.getSkills();
  const skills = normalizeResourceItems(raw);
  sendResponse(ws, sessionId, "get_skills", msg.id, true, { skills });
}

export async function handleGetExtensions(ctx: CommandContext): Promise<void> {
  const { ws, sessionId, state, msg } = ctx;
  const loader = getResourceLoader(state.runtime);
  if (!loader?.getExtensions) {
    sendResponse(
      ws,
      sessionId,
      "get_extensions",
      msg.id,
      false,
      undefined,
      "resource loader or getExtensions not available",
    );
    return;
  }
  await loader.reload?.();
  const raw = await loader.getExtensions();
  const extensions = normalizeResourceItems(raw);
  sendResponse(ws, sessionId, "get_extensions", msg.id, true, { extensions });
}

export async function handleGetPrompts(ctx: CommandContext): Promise<void> {
  const { ws, sessionId, state, msg } = ctx;
  const loader = getResourceLoader(state.runtime);
  if (!loader?.getPrompts) {
    sendResponse(
      ws,
      sessionId,
      "get_prompts",
      msg.id,
      false,
      undefined,
      "resource loader or getPrompts not available",
    );
    return;
  }
  await loader.reload?.();
  const raw = await loader.getPrompts();
  const prompts = normalizeResourceItems(raw);
  sendResponse(ws, sessionId, "get_prompts", msg.id, true, { prompts });
}

export async function handleExportHtml(ctx: CommandContext): Promise<void> {
  const { ws, sessionId, state, msg } = ctx;
  const parsed = assertShape(msg, ExportHtmlSchema);
  const outputPath = parsed.outputPath ? String(parsed.outputPath) : undefined;
  const path = await state.runtime.session.exportToHtml(outputPath);
  sendResponse(ws, sessionId, "export_html", parsed.id, true, { path });
}

export async function handleCompact(ctx: CommandContext): Promise<void> {
  const { ws, sessionId, state, msg } = ctx;
  const parsed = assertShape(msg, CompactSchema);
  await state.runtime.session.compact(
    parsed.customInstructions ? String(parsed.customInstructions) : undefined
  );
  sendResponse(ws, sessionId, "compact", parsed.id, true);
}

export async function handleExtensionUiResponse(ctx: CommandContext): Promise<void> {
  const { state, msg, logger } = ctx;
  const parsed = assertShape(msg, ExtensionUiResponseSchema);
  if (!parsed.id) {
    logger.warn("extension_ui_response missing id");
    return;
  }
  // Resolve the pending confirm/select/input/editor request the bridge is
  // awaiting. This is intentionally not acknowledged back to the GUI: the
  // GUI sends responses fire-and-forget and does not wait for a reply.
  state.ui.resolve(parsed.id, {
    confirmed: parsed.confirmed,
    cancelled: parsed.cancelled,
    value: parsed.value,
  });
}

export async function handleGetModels(ctx: GlobalCommandContext): Promise<void> {
  const { ws, msg, modelRegistry } = ctx;
  const models = await modelRegistry.getAvailable();
  const wireModels: ModelWireInfo[] = models.map((m: unknown) => {
    const model = m as {
      provider: string;
      id: string;
      name: string;
      thinkingLevelMap?: Record<string, string>;
    };
    return {
      provider: model.provider,
      id: model.id,
      name: model.name,
      thinkingLevelMap: model.thinkingLevelMap,
    };
  });
  sendResponse(ws, ctx.sessionId || "bridge", "get_models", msg.id, true, {
    models: wireModels,
  });
}

export async function handleGetSessionStats(ctx: CommandContext): Promise<void> {
  const { ws, sessionId, state, msg } = ctx;
  const stats = state.runtime.session.getSessionStats();
  const contextUsage = state.runtime.session.getContextUsage();
  sendResponse(ws, sessionId, "get_session_stats", msg.id, true, {
    ...stats,
    contextUsage,
  });
}

export async function handleGetModel(ctx: GlobalCommandContext, state?: SessionState): Promise<void> {
  const { ws, msg, modelRegistry } = ctx;
  const parsed = assertShape(msg, GetModelSchema);
  let model: unknown;

  if (parsed.provider && parsed.modelId) {
    model = modelRegistry.find(parsed.provider, parsed.modelId);
    if (!model) {
      throw new Error(`model not found: ${parsed.provider}:${parsed.modelId}`);
    }
  } else if (state?.runtime.session.model) {
    model = state.runtime.session.model;
  } else {
    throw new Error("missing provider/modelId or active session model");
  }

  const m = model as { provider: string; id: string; name: string } | undefined;
  sendResponse(ws, ctx.sessionId || "bridge", "get_model", msg.id, true, {
    model: m
      ? {
          provider: m.provider,
          id: m.id,
          name: m.name,
        }
      : null,
  });
}

export async function handleGetProviders(ctx: GlobalCommandContext): Promise<void> {
  const { ws, msg, modelRegistry } = ctx;
  const allModels = await modelRegistry.getAll();
  const providers = [...new Set(allModels.map((m: unknown) => (m as { provider: string }).provider))];
  const providerList = providers.map((id: string) => ({
    id,
    name: modelRegistry.getProviderDisplayName(id),
    configured: modelRegistry.getProviderAuthStatus(id).configured,
  }));
  sendResponse(ws, ctx.sessionId || "bridge", "get_providers", msg.id, true, {
    providers: providerList,
  });
}

export async function handleSetAuth(ctx: GlobalCommandContext): Promise<void> {
  const { ws, msg, authStorage } = ctx;
  const parsed = assertShape(msg, SetAuthSchema);
  authStorage.set(parsed.provider, { type: "api_key", key: parsed.key });
  sendResponse(ws, ctx.sessionId || "bridge", "set_auth", msg.id, true);
}

export async function handleGetSettings(ctx: GlobalCommandContext): Promise<void> {
  const { ws, msg, settingsManager } = ctx;
  assertShape(msg, GetSettingsSchema);
  sendResponse(ws, ctx.sessionId || "bridge", "get_settings", msg.id, true, {
    compactionEnabled: settingsManager.getCompactionEnabled(),
    defaultThinkingLevel: settingsManager.getDefaultThinkingLevel(),
    defaultModel: settingsManager.getDefaultModel(),
    defaultProvider: settingsManager.getDefaultProvider(),
  });
}

export async function handleSetCompactionEnabled(
  ctx: GlobalCommandContext,
): Promise<void> {
  const { ws, msg, settingsManager } = ctx;
  const parsed = assertShape(msg, SetCompactionEnabledSchema);
  settingsManager.setCompactionEnabled(parsed.enabled);
  await settingsManager.flush();
  sendResponse(
    ws,
    ctx.sessionId || "bridge",
    "set_compaction_enabled",
    msg.id,
    true,
  );
}

export async function handleSetDefaultThinkingLevel(
  ctx: GlobalCommandContext,
): Promise<void> {
  const { ws, msg, settingsManager } = ctx;
  const parsed = assertShape(msg, SetDefaultThinkingLevelSchema);
  settingsManager.setDefaultThinkingLevel(parsed.level);
  await settingsManager.flush();
  sendResponse(
    ws,
    ctx.sessionId || "bridge",
    "set_default_thinking_level",
    msg.id,
    true,
  );
}

export async function handleSetDefaultModel(
  ctx: GlobalCommandContext,
): Promise<void> {
  const { ws, msg, settingsManager } = ctx;
  const parsed = assertShape(msg, SetDefaultModelSchema);
  settingsManager.setDefaultModel(parsed.modelId);
  await settingsManager.flush();
  sendResponse(
    ws,
    ctx.sessionId || "bridge",
    "set_default_model",
    msg.id,
    true,
  );
}

export async function handleSetDefaultProvider(
  ctx: GlobalCommandContext,
): Promise<void> {
  const { ws, msg, settingsManager } = ctx;
  const parsed = assertShape(msg, SetDefaultProviderSchema);
  settingsManager.setDefaultProvider(parsed.provider);
  await settingsManager.flush();
  sendResponse(
    ws,
    ctx.sessionId || "bridge",
    "set_default_provider",
    msg.id,
    true,
  );
}

export const sessionCommands = new Map<string, SessionCommandHandler>([
  ["prompt", handlePrompt],
  ["steer", handleSteer],
  ["follow_up", handleFollowUp],
  ["abort", handleAbort],
  ["set_model", handleSetModel],
  ["set_thinking_level", handleSetThinkingLevel],
  ["new_session", handleNewSession],
  ["navigate_tree", handleNavigateTree],
  ["fork", handleFork],
  ["clone", handleClone],
  ["get_messages", handleGetMessages],
  ["get_skills", handleGetSkills],
  ["get_extensions", handleGetExtensions],
  ["get_prompts", handleGetPrompts],
  ["export_html", handleExportHtml],
  ["compact", handleCompact],
  ["extension_ui_response", handleExtensionUiResponse],
  ["get_session_stats", handleGetSessionStats],
]);
