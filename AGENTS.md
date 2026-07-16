# mini-pi

A desktop GUI chat application that wraps the `pi` AI coding agent SDK. Built with Rust and GPUI (the GPU-accelerated UI framework from the Zed editor).

## Project Overview

`mini-pi` provides a native chat-window interface for interacting with the `pi` coding agent SDK. Users can create chat threads, select AI models and thinking levels, manage workspaces (project directories), authenticate via Supabase to sync agent configuration across devices, and optionally control the app remotely from a phone over a Cloudflare Tunnel.

The application runs a Bun/WebSocket bridge (`pi-bridge/`) that loads `@earendil-works/pi-coding-agent` and exposes the SDK over a single multiplexed WebSocket connection. Chat sessions are persisted locally in SQLite and as JSONL files, while agent configuration can be synced to a Supabase storage bucket.

Additional features include a system-tray icon, mini-app launcher (image generation and journal web apps hosted in a `wry` WebView), a skills/extensions/prompts manager backed by the ClawHub skill registry and npm, inline message editing, media attachments, voice input, and session statistics.

On first run, if `~/.pi/agent/` contains JSON files, the app offers to import them into `~/.mini-pi/agent/`.

## Technology Stack

- **Language:** Rust (2024 edition, requires stable Rust >= 1.92)
- **UI Framework:** GPUI from Zed (pinned git revision) — hybrid immediate/retained mode, GPU-accelerated (Metal on macOS, Vulkan on Linux/Windows)
- **Component library:** `gpui-component` and `gpui-wry` from longbridge (pinned git revision)
- **WebView:** `wry` (`lb-wry` package) for the mini-app windows
- **Database:** SQLite via `rusqlite` (bundled), with WAL mode and manual migrations
- **Async:** `smol` + `futures` for GPUI background tasks; dedicated `tokio` runtimes for the WebSocket bridge and the remote-control HTTP server
- **HTTP:** `reqwest` (blocking client for auth, sync, title generation, skill/npm search, voice transcription)
- **Serialization:** `serde` / `serde_json`
- **Markdown:** `gpui_component::text::TextView` / `TextViewState` renders assistant messages
- **System tray:** `tray-icon` (platform-native menu; GTK path on Linux)
- **Audio:** `cpal` + `hound` for voice recording and WAV export
- **Networking:** `tokio-tungstenite` for the bridge WebSocket, `axum` + `tower-http` for remote control
- **QR codes:** `qrcode` + `image`
- **Archive extraction:** `zip`, `tar`, `flate2` for installing skills from the registry
- **Platform specifics:** `objc` on macOS for native window chrome and WebView edit commands; `raw-window-handle` for cross-platform window handles; Windows `CREATE_NO_WINDOW` flag and `windows_subsystem = "windows"` so the release binary does not open a console window; Linux uses `gtk` for the tray icon
- **Bridge runtime:** Bun (TypeScript SDK bridge in `pi-bridge/`)

## Repository Layout

```
Cargo.toml              # Package manifest and dependencies
src/main.rs             # Minimal launcher: delegates to mini_pi::app::run()
src/lib.rs              # Public module re-exports
src/app.rs              # Application bootstrap, global key bindings, menus, AppStore setup, initial main window wrapped in gpui_component::Root
src/core/
  actions.rs            # Global actions (CloseWindow, Quit, SendMessage, Login, Logout, SignUp, About, CreateThread, etc.)
  app.rs                # AppStore GPUI Global, MainOverlay enum, custom_window_options(), apply_font_size()
  assets.rs             # AssetSource implementation that loads SVGs from the assets/ directory
  tray.rs               # System-tray icon and menu (show/hide, create thread, quit)
  session_handle.rs     # SessionHandle: per-thread SDK session state, message handling, bridge event loop
  session_manager.rs    # In-memory registry of active SessionHandle entities keyed by session file
src/config/
  app_config.rs         # ~/.config/mini-pi/config.json (font_size, remote_control, theme)
  command_config.rs     # Slash-command item type, parsing, and startup load from the bridge
  model_config.rs       # Model list helpers, provider:model id parsing, thinking-level mapping
src/data/
  models.rs             # Domain enums: Role, PartState, MessagePart, Message, ChatState
  store.rs              # SQLite connection, migrations, and CRUD for threads/workspaces/user_settings
src/auth/
  state.rs              # AuthState, SupabaseSession, session persistence, agent-dir helpers
  supabase.rs           # Supabase auth and Storage API client (hardcoded URL + anon key)
src/rpc/
  pi_rpc.rs             # PiBridge shared WebSocket client, PiRpc session handle, BridgeEvent enum, JSON parser
src/remote/
  controller.rs         # RemoteController: enable/disable, command dispatch, SSE broadcasting, cloudflared lifecycle
  cloudflared.rs        # Auto-download and resolve cloudflared binary in ~/.mini-pi/bin/
  server.rs             # axum REST server with Server-Sent Events
  tunnel.rs             # cloudflared process management and quick-tunnel URL parsing
  qr.rs                 # QR code generation for the tunnel URL
  auth.rs               # Optional local bearer-token validation
  types.rs              # RemoteCommand / RemoteResponse / AI stream event types
src/sync/
  settings_sync.rs      # Two-way sync of ~/.mini-pi/agent/ files with Supabase Storage bucket pi-sync
src/ui/
  chat_input.rs         # ChatInput: multi-line chat input backed by gpui_component::input::InputState with @ mention and / slash-command autocomplete
  loader.rs             # Animated dot loader and text-loader spinner
src/utils/
  file_scanner.rs       # Workspace file-tree scanner used by @ mention autocomplete
  format.rs             # Relative-time formatting and string truncation helpers
  llm.rs                # Cloudflare AI Gateway title generator
  window_helpers.rs     # Platform-specific window-level helpers (pin-to-top, hide/show, activate)
  paths.rs              # app_root() / find_bun() resource resolution for dev and packaged builds
  color.rs              # Color helpers
  voice.rs              # Microphone capture, WAV export, and remote transcription worker client
src/views/
  thread_list.rs        # Home window showing pinned/unpinned threads, workspace filter, search
  chat_app.rs           # Per-thread window frame: gpui_component TitleBar with pin/export/workspace controls, wraps ChatWindow in Root
  chat_window.rs        # Per-thread chat content: model dropdown, workspace bar, message rendering via TextView
  user_panel.rs         # Account/auth/settings panel, including theme toggle, remote-control toggle and QR code
  workspace_manager.rs  # Workspace picker content rendered inside a gpui_component::Dialog
  reasoning.rs          # Collapsible thinking/reasoning display using gpui_component::Collapsible
  onboarding.rs         # First-run onboarding / import prompt from ~/.pi/agent/
  mini_app.rs           # Mini-app launcher and WebView host for image-gen and journal apps
  skills_panel.rs       # Lists loaded skills/extensions/prompts from the bridge; create-prompt window
  install_skill.rs      # ClawHub skill registry browser and local skill installer
  install_extension.rs  # npm extension search/installer
  pi_settings.rs        # Standalone pi agent settings window (API keys, default model/provider/thinking, compaction)
  about.rs              # About window
  auth_dialog.rs        # Login/signup dialog
  workspace_filter.rs   # Workspace filter dropdown for the thread list
  tool_call.rs          # Tool-call part rendering
  create_thread_button.rs # Floating create-thread button
assets/                 # SVG icons, app_icon.png, theme JSON files loaded at runtime
pi-bridge/              # Bun/WebSocket SDK bridge
  package.json          # Node dependencies; scripts use Bun but `npm ci && npm run build` (tsc) also work in CI
  src/index.ts          # Entry point: parses args and starts BridgeServer
  src/server.ts         # WebSocket server that runs @earendil-works/pi-coding-agent
  src/commands.ts       # Bridge command handlers (models, providers, settings, session commands)
  src/session.ts        # Per-session runtime and SDK invocation
  src/types.ts          # Bridge TypeScript types
  src/messages.ts       # Wire message builders
  src/ui-context.ts     # Extension UI context helpers
  src/extension-ui.ts   # Extension UI protocol
  src/config.ts         # Bridge CLI config parsing
  src/tools/            # Bridge tool implementations and tests
docs/                   # Internal reference material (some files may be outdated; see notes below)
scripts/                # Build scripts: macOS/Windows installers and generate_icons.py
```

## Build, Run and Test

```bash
# Install the SDK bridge dependencies (Bun only for development)
cd pi-bridge && bun install && cd ..

# Standard cargo workflow
cargo build
cargo run

# Release build
cargo build --release

# Tests
cargo test
```

### Installers

Self-contained installers are built from the scripts in `scripts/`:

- `scripts/build-windows.ps1` produces `target/mini-pi-<version>-<arch>.msi`.
- `scripts/build-macos.sh` produces `target/mini-pi-<version>-<arch>.dmg`.

Both scripts download the platform-specific Bun runtime and bundle it alongside a single `pi-bridge.js` produced by `bun build --target bun --outfile pi-bridge.js`, so end users do not need Bun or Node.js installed. They are also driven by `.github/workflows/release.yml` on version tags.

### Packaging layout

- Release binaries resolve `assets/` and `pi-bridge/` relative to the executable (`src/utils/paths.rs`), falling back to `CARGO_MANIFEST_DIR` during `cargo run`.
- macOS: `cargo-bundle` reads `[package.metadata.bundle]` in `Cargo.toml` and builds `Mini Pi.app` with resources under `Contents/Resources`.
- Windows: a hand-written WiX source (`wix/main.wxs`) plus a `heat`-generated file list produces a per-machine `.msi`. The installer creates both a Start Menu shortcut and a Desktop shortcut.

### CI note for the bridge

`.github/workflows/release.yml` uses `npm ci && npm run build` inside `pi-bridge/` because the `build` script only runs `tsc`. The runtime still requires a Bun binary (system or bundled) to execute the compiled bridge.

### Prerequisites

1. **Rust stable** (>= 1.92)
2. **Platform toolchain:**
   - **macOS:** Xcode + Xcode Command Line Tools (`xcode-select --install`)
   - **Linux:** Vulkan drivers, `libxcb`, `libxkbcommon`, `libfontconfig`, `libssl`
   - **Windows:** Vulkan SDK or DirectX
3. **Bun** is the only supported runtime for the SDK bridge during development; `pi-bridge/node_modules` must be present (`cd pi-bridge && bun install`). The app spawns the bridge automatically and connects to it over a local WebSocket.
4. *(Optional)* **cloudflared** is used for the phone remote-control feature. If it is not installed on the system, the app offers to download the official binary into `~/.mini-pi/bin/` when the user enables remote control. You can also install it manually with `brew install cloudflared` or from https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/downloads/.
5. *(Optional)* **Cloudflare AI Gateway** environment variables for auto-generated thread titles:
   - `CLOUDFLARE_API_KEY`
   - `CLOUDFLARE_ACCOUNT_ID`
   - `CLOUDFLARE_GATEWAY_ID`

## Runtime Architecture

### Data Storage

- **Database:** `~/.mini-pi/mini-pi.db` (SQLite, WAL mode, foreign keys ON)
- **Sessions:** `~/.mini-pi/sessions/*.jsonl` — conversation history files used by the `pi` subprocess
- **Agent config:** `~/.mini-pi/agent/` — passed to the SDK bridge via `--agent-dir`; imported from `~/.pi/agent/` on first run
- **App config:** `~/.config/mini-pi/config.json`
- **Auth session:** Stored in the `user_settings` table of `~/.mini-pi/mini-pi.db` under key `supabase_session`
- **Sync metadata:** Stored in the `user_settings` table of `~/.mini-pi/mini-pi.db` under key `sync_meta`
- **Skills/prompts:** `~/.mini-pi/agent/skills/` and `~/.mini-pi/agent/prompts/`
- **Bun cache:** `~/.mini-pi/bun-cache/`

### Database Migrations

Migrations are defined as a static slice of `(name, sql)` tuples in `src/data/store.rs` and tracked in a `_migrations` table. Current migrations:

- `001_init` — creates `threads` table
- `002_workspaces` — creates `workspaces` table
- `003_user_settings` — creates `user_settings` key-value table
- `004_thinking_level` — adds `thinking_level` column to `threads` table
- `005_thread_metadata` — adds `metadata` JSON column to `threads` table (used for workspace filtering and the "new activity" flag)

Thread and workspace IDs are nanoid strings.

### Process Model

On launch the app:

1. Opens SQLite and runs migrations.
2. Loads `AppConfig` from disk (remote control is forced off at startup and saved).
3. Attempts to restore the Supabase session (refresh if expired, then fetch user).
4. Spawns the shared `PiBridge` process and loads the model and command lists from it.
5. Sets the global `AppStore` and triggers a background agent-config sync if logged in.
6. Opens the main `MiniPiApp` window and initializes the system tray.

`MiniPiApp` hosts two tabs (`Threads`, `Skills`) and two overlay panels (`UserPanel`, `MiniApp`). Each chat thread opens its own window. Inside the window, `ChatWindow` uses a `SessionHandle` (`src/core/session_handle.rs`) that:

- Registers a session with the shared `PiBridge` via `{ type: "create_session", sessionId, sessionPath, cwd, model, thinkingLevel }`.
- Receives a per-session `futures::channel::mpsc` stream of `BridgeEvent`s from the bridge.
- Sends commands (`prompt`, `set_model`, `set_thinking_level`, `fork`, `navigate_tree`, `abort`, `export_html`, `extension_ui_response`, etc.) over the shared WebSocket; the bridge forwards SDK events back with the same `sessionId`.
- The `ChatWindow` GPUI task consumes the per-session event stream and updates messages/reasoning/tool-call state.

`SessionManager` (`src/core/session_manager.rs`) keeps a map of active `SessionHandle` entities keyed by session file so the remote controller and thread list can find them without creating duplicates.

### Remote Control

When enabled in the user settings panel (`remote_control.enabled` in `~/.config/mini-pi/config.json`):

- `RemoteController` starts a local `axum` server bound to `127.0.0.1:<bind_port>`, served by a dedicated Tokio runtime. Commands and SSE events are routed through Tokio channels.
- It auto-spawns `cloudflared` to expose that port through a Cloudflare Tunnel (quick tunnel by default, or a named tunnel via `cloudflared.tunnel_token`; named tunnels also require `cloudflared.hostname`).
- If no bundled `cloudflared` binary exists in `~/.mini-pi/bin/`, `UserPanel` shows a modal that downloads the platform-specific official release into `~/.mini-pi/bin/`, updates `remote_control.cloudflared.command`, and starts the tunnel.
- The user panel displays the public tunnel URL and a QR code for easy phone scanning.
- The phone sends REST commands (`GET /threads`, `POST /threads/:id/message`, `POST /files/download`, etc.) and receives live assistant replies from the streaming message POST response.
- Message responses stream AI SDK UI message chunks over data-only Server-Sent Events.
- Cloudflare Access is the recommended authentication layer at the tunnel edge; an optional local `bearer_token` can be configured for quick-tunnel mode.

### Window Management

- `AppStore.main_window` tracks the single main window handle; `AppStore.main_window_hidden` supports show/hide from the tray menu.
- `AppStore.thread_windows: HashMap<String, AnyWindowHandle>` maps thread IDs to open chat windows to avoid duplicate windows for the same thread. Be aware that stale handles are not currently removed when a window is closed externally.
- `AppStore.pi_settings_window` tracks the standalone pi-settings window.
- All windows use `custom_window_options()` from `src/core/app.rs`: a transparent titlebar on macOS with traffic-light offset, and client-side decorations on other platforms.
- `TitleBar` is custom-rendered and includes platform-specific pin-to-top support (macOS `NSWindow` level, Windows `SetWindowPos`, Linux best-effort `wmctrl`).

### System Tray

`TrayManager` (`src/core/tray.rs`) creates a platform-native tray icon with a menu for Show/Hide, Create Thread, and Quit. Left-clicking the icon also toggles the main window. On Linux the tray icon runs on its own GTK thread.

### Key Bindings

Global bindings are registered in `src/app.rs`. Context-sensitive bindings use GPUI's `key_context` system (e.g. `"Input"` for `gpui_component::input::Input`). Common bindings:

- `Cmd/Ctrl + W` — Close window
- `Cmd + Q` — Quit
- `Enter` — Send message (in chat window)
- `Cmd + A` / `Ctrl + A` — Select all
- `Cmd + C/V/X` / `Ctrl + C/V/X` — Copy / Paste / Cut
- `Cmd + Z` / `Ctrl + Z` — Undo
- `Cmd + Shift + Z` / `Ctrl + Shift + Z` — Redo
- Arrow keys with Shift — Selection
- Home/End or `Ctrl+A/E` — Line start/end
- `Escape` — Close mention/command popups or workspace manager

Dropdowns handle `up`, `down`, `enter`, and `escape` internally. The chat input's `@` / `/` popup navigation is handled by an `on_key_down` listener on the input container.

## Code Organization Conventions

- **Entities:** All persistent UI state lives in GPUI `Entity<T>` structs. Views implement `Render`.
- **Events:** Components communicate via `EventEmitter<E>` + `cx.subscribe(...)` or `cx.observe(...)`.
- **Actions:** Global actions are declared with the `actions!` macro in `src/core/actions.rs`.
- **Globals:** `AppStore` is a GPUI `Global` holding `Arc<Store>`, config, auth state, session, sync state, a window-handle map, the shared `PiBridge`, loaded models/commands, and the remote controller.
- **Styling:** Uses GPUI's Tailwind-inspired fluent API (`div().flex().bg(rgb(...)).child(...)`).
- **Text input:** Custom inputs use `gpui_component::input::Input` / `InputState`:
  - `ui::chat_input::ChatInput` — multi-line chat input with `@` mention autocomplete and `/` slash-command palette support, backed by `gpui_component::input::InputState`
- **Markdown:** `gpui_component::text::TextView` / `TextViewState` renders assistant messages.
- **WebView:** `gpui-wry::WebView` wraps a `wry` webview inside GPUI windows for the mini apps.
- **Async patterns:**
  - GPUI background work uses `cx.spawn(async { smol::unblock(...) }).detach()`.
  - Blocking I/O (database, HTTP, sync, title generation) runs on `smol::unblock`.
  - The bridge and remote server each create their own `tokio::runtime::Runtime` for WebSocket/HTTP networking.

## External SDK Dependency

This application is a thin GUI wrapper around the `@earendil-works/pi-coding-agent` SDK, run inside a local pi-bridge process. Release builds ship the Bun runtime plus a single bundled `pi-bridge.js` produced by `bun build --target bun --outfile pi-bridge.js`; the Rust app spawns `bun run pi-bridge.js` directly. For development, Bun and installed `pi-bridge/node_modules` are required. Chat functionality will fail at runtime when `PiBridge::spawn` is called if no Bun runtime or bridge bundle/source is available. The wire protocol is documented by the `BridgeEvent` enum and the multiplexed JSON messages in `src/rpc/pi_rpc.rs`.

## Security Considerations

- The Supabase anonymous key and URL are hardcoded in `src/auth/supabase.rs`.
- Auth tokens are stored in plaintext JSON inside the `user_settings` table of the local SQLite database (`~/.mini-pi/mini-pi.db`).
- Agent configuration and chat sessions are stored locally in the user's home directory.
- Cloudflare API credentials are read from environment variables only.
- The remote-control quick tunnel supports an optional local `bearer_token`; Cloudflare Access is recommended for production use.

## Testing

- `cargo test` runs unit tests across `src/config`, `src/remote`, `src/rpc`, `src/sync`, `src/auth`, `src/data`, and `src/utils`, plus doc-tests in `src/ui/loader.rs`.
- As of the last verification, the suite contains **73 unit tests** and **3 doc-tests**.
- Two tests in `src/remote/cloudflared.rs` (`resolve_cloudflared_command_falls_back_to_bundled_binary` and `resolve_cloudflared_command_errors_when_nothing_exists`) fail when a `cloudflared` binary is present on `PATH`, because the resolver prefers the system binary. They pass in environments without `cloudflared` installed.
- `.github/workflows/release.yml` builds Windows (`.msi`) and macOS (`.dmg`) installers on version tags.

## Documentation

- `docs/at-mention-autocomplete.md` — Guide for implementing `@` mention autocomplete, derived from Zed's `agent_ui` crate.
- `docs/design-review.md` — Design review (in Chinese) that lists known architecture issues. **Note:** parts of this document still describe an older JSON Lines subprocess architecture and may not match the current WebSocket bridge implementation.
- `docs/markdown-improvement-plan.md` — Planned markdown renderer improvements and known rendering bugs.
- `docs/remote-api.md` — Remote-control REST/SSE API documentation.
- `docs/TODO.md` — Short checklist of upcoming features and known bugs.
- `docs/ISSUES.md` — Issue notes.
- `docs/dialog-enter-close-bug.md` — Bug investigation notes.

`docs/` contains internal reference material, not project user documentation.

## Notes for Agents

- `docs/` contains internal reference material, not project user documentation.
- The model list is loaded dynamically at startup from the SDK bridge via `ModelRegistry.getAvailable()` and stored in `AppStore.models`. `src/config/model_config.rs` exposes the helpers (`all_models`, `get_model_name`, `get_model_id`, `model_display_name`, `parse_model_id`, `resolve_full_model_id`) that take a `&[ModelInfo]` slice. Model IDs use a `<provider>:<model>` format parsed by `parse_model_id`.
- The slash-command / skill list is loaded once at startup from the SDK bridge and cached in `AppStore.commands` (`src/config/command_config.rs`). New composer `ChatInput`s are seeded with this list so the `/` popup works immediately; each session still refreshes the list via `get_commands` when it starts.
- When adding database changes, append a new migration tuple to `MIGRATIONS` in `src/data/store.rs`.
- Assets are loaded at runtime via `core::assets::Assets`. The asset root is resolved from the executable path (`src/utils/paths::app_root`), so packaged releases keep `assets/` next to the binary (Windows) or inside `Mini Pi.app/Contents/Resources` (macOS). During development the helper falls back to `CARGO_MANIFEST_DIR`.
- The `pi-bridge/` directory is resolved the same way. Release builds ship the platform-specific Bun binary (`bun` on macOS/Linux, `bun.exe` on Windows) plus a single bundled `pi-bridge.js` in the app root; during development the bridge is run with `bun run src/index.ts`.
- The app is primarily developed and tested on macOS. Windows-specific and Linux-specific code exists (e.g. `CREATE_NO_WINDOW`, `windows_subsystem = "windows"`, client-side titlebar controls, `wmctrl`, GTK tray icon) but may need verification.
- The `pi-bridge/` directory must have its dependencies installed with `bun install` before running the app outside an installer.
- The wire protocol between Rust and the bridge uses a single WebSocket connection; every message includes a `sessionId` so multiple chat sessions can share one connection.
- Model IDs in `src/config/model_config.rs` must resolve through the SDK's `ModelRegistry`/`getModel`. Provider names like `cloudflare-ai-gateway` may not be recognized by the SDK and may need to be mapped to SDK-supported providers (`anthropic`, `openai`, etc.).
- Several known issues are documented in `docs/design-review.md`; review it before making large changes to process management, sync, or window lifecycle.
- New windows and dialogs are typically wrapped in `gpui_component::Root` to get dialog/notification/sheet layers.
- Adding a new top-level window usually requires:
  1. A new `Render` view in `src/views/`.
  2. An `open_*_window(cx)` helper.
  3. Optionally a global action in `src/core/actions.rs` and a menu/key binding in `src/app.rs`.
- The main window tabs and overlays are driven by `MainOverlay` and `MiniPiTab` in `src/app.rs`; adding a new main-window panel means updating both the enum and the render switch.
