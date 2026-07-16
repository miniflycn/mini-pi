import { Theme } from "@earendil-works/pi-coding-agent";
import type { ExtensionUIContext } from "@earendil-works/pi-coding-agent";
import type { ExtensionUiChannel } from "./extension-ui.js";

// Every foreground color slot a Theme must define. Each is mapped to "" so the
// Theme emits a bare reset sequence (a visual no-op) rather than throwing
// "Unknown theme color" when an extension reads it.
const THEME_FG_KEYS = [
  "accent",
  "border",
  "borderAccent",
  "borderMuted",
  "success",
  "error",
  "warning",
  "muted",
  "dim",
  "text",
  "thinkingText",
  "userMessageText",
  "customMessageText",
  "customMessageLabel",
  "toolTitle",
  "toolOutput",
  "mdHeading",
  "mdLink",
  "mdLinkUrl",
  "mdCode",
  "mdCodeBlock",
  "mdCodeBlockBorder",
  "mdQuote",
  "mdQuoteBorder",
  "mdHr",
  "mdListBullet",
  "toolDiffAdded",
  "toolDiffRemoved",
  "toolDiffContext",
  "syntaxComment",
  "syntaxKeyword",
  "syntaxFunction",
  "syntaxVariable",
  "syntaxString",
  "syntaxNumber",
  "syntaxType",
  "syntaxOperator",
  "syntaxPunctuation",
  "thinkingOff",
  "thinkingMinimal",
  "thinkingLow",
  "thinkingMedium",
  "thinkingHigh",
  "thinkingXhigh",
  "bashMode",
] as const;

// Every background color slot a Theme must define. Same "" no-op treatment.
const THEME_BG_KEYS = [
  "selectedBg",
  "userMessageBg",
  "customMessageBg",
  "toolPendingBg",
  "toolSuccessBg",
  "toolErrorBg",
] as const;

function buildNeutralTheme(): Theme {
  const fg: Record<string, string> = {};
  for (const key of THEME_FG_KEYS) fg[key] = "";
  const bg: Record<string, string> = {};
  for (const key of THEME_BG_KEYS) bg[key] = "";
  return new Theme(
    fg as ConstructorParameters<typeof Theme>[0],
    bg as ConstructorParameters<typeof Theme>[1],
    "256color",
  );
}

const NEUTRAL_THEME = buildNeutralTheme();

/**
 * Build the `ExtensionUIContext` the bridge binds to each session.
 *
 * The bridge runs the SDK without a terminal UI, so interactive UI is forwarded
 * to the Rust GUI through `channel`:
 *
 * - `confirm` opens a dialog on the GUI and awaits the user's answer.
 * - `notify` pushes a GUI notification (fire-and-forget).
 *
 * The remaining methods are not wired up yet and fall back to no-op /
 * "cancelled" behaviour so extensions that call them never crash the session.
 */
export function createUiContext(channel: ExtensionUiChannel): ExtensionUIContext {
  return {
    // Forwarded to the GUI.
    confirm: async (title, message, opts) => {
      const result = await channel.request(
        "confirm",
        { title, message, timeout: opts?.timeout },
        opts,
      );
      if (result.cancelled) return false;
      return result.confirmed ?? false;
    },
    notify: (message, type) => {
      channel.emit("notify", { message, notifyType: type });
    },

    // Forwarded to the GUI.
    select: async (title, options, opts) => {
      const result = await channel.request(
        "select",
        { title, options, timeout: opts?.timeout },
        opts,
      );
      if (result.cancelled) return undefined;
      return result.value ?? undefined;
    },
    input: async (title, placeholder, opts) => {
      const result = await channel.request(
        "input",
        { title, placeholder, timeout: opts?.timeout },
        opts,
      );
      if (result.cancelled) return undefined;
      return result.value ?? undefined;
    },
    editor: async (title, prefill) => {
      const result = await channel.request("editor", { title, prefill });
      if (result.cancelled) return undefined;
      return result.value ?? undefined;
    },

    // Not forwarded yet: resolve as cancelled / empty.
    custom: <T>(): Promise<T> => Promise.resolve(undefined as T),

    // Status / widgets: no GUI surface for these yet.
    onTerminalInput: () => () => {},
    setStatus: () => {},
    setWorkingMessage: () => {},
    setWorkingVisible: () => {},
    setWorkingIndicator: () => {},
    setHiddenThinkingLabel: () => {},
    setWidget: (_key: string, _content: unknown, _options?: unknown) => {},
    setFooter: () => {},
    setHeader: () => {},
    setTitle: () => {},

    // Editor: nothing to read from / write to in the bridge.
    pasteToEditor: () => {},
    setEditorText: () => {},
    getEditorText: () => "",
    addAutocompleteProvider: () => {},
    setEditorComponent: () => {},
    getEditorComponent: () => undefined,

    // Theme: expose a neutral no-color theme; switching is unsupported.
    theme: NEUTRAL_THEME,
    getAllThemes: () => [],
    getTheme: () => undefined,
    setTheme: () => ({
      success: false,
      error: "Theme switching is not supported in the bridge",
    }),

    // Tool output expansion is a TUI concept; report collapsed.
    getToolsExpanded: () => false,
    setToolsExpanded: () => {},
  };
}
