# mini-pi

A desktop GUI chat application that wraps the `pi` AI coding agent SDK. Built with Rust and GPUI (the GPU-accelerated UI framework from the Zed editor).

## Project Overview

`mini-pi` provides a native chat-window interface for interacting with the `pi` coding agent SDK. Users can create chat threads, select AI models, manage workspaces (project directories), authenticate via Supabase to sync agent configuration across devices, and optionally control the app remotely from a phone over a Cloudflare Tunnel.

The application runs a Bun/WebSocket bridge (`pi-bridge/`) that loads `@earendil-works/pi-coding-agent` and exposes the SDK over a single multiplexed WebSocket connection. Chat sessions are persisted locally in SQLite and as JSONL files, while agent configuration can be synced to a Supabase storage bucket.

On first run, if `~/.pi/agent/` contains JSON files, the app offers to import them into `~/.mini-pi/agent/`.

## Technology Stack

- **Language:** Rust (2024 edition, requires stable Rust >= 1.92)
- **UI Framework:** GPUI 0.2.2 — hybrid immediate/retained mode, GPU-accelerated (Metal on macOS, Vulkan on Linux/Windows)
- **Database:** SQLite via `rusqlite` (bundled), with WAL mode and manual migrations
- **Async:** `smol` + `futures` for background tasks
- **HTTP:** `reqwest` (blocking client for auth, sync, and title generation)
- **Serialization:** `serde` / `serde_json`
- **Markdown:** `gpui_component::text::TextView` / `TextViewState` (Markdown/HTMl rendering via gpui-component)
- **Platform specifics:** `objc` on macOS for native window chrome; `raw-window-handle` for cross-platform window handles; Windows `CREATE_NO_WINDOW` flag

## Repository Layout

```
Cargo.toml              # Package manifest and dependencies
src/main.rs             # Minimal launcher: delegates to mini_pi::app::run()
src/lib.rs              # Public module re-exports (used by examples)
src/app.rs              # Application bootstrap, global key bindings, AppStore setup, initial ThreadList window wrapped in gpui_component::Root
src/core/
  actions.rs            # Global actions: CloseWindow, Quit, SendMessage, Login, Logout, SignUp
  app.rs                # AppStore GPUI Global and custom_window_options()
  assets.rs             # AssetSource implementation that loads SVGs from the assets/ directory
src/config/
  app_config.rs         # ~/.config/mini-pi/config.json (font_size, remote_control, theme)
  command_config.rs     # Slash-command item type, parsing, and startup load from the bridge
  model_config.rs       # Hardcoded model list and provider/name helpers
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
pi-bridge/
  package.json          # Node dependencies for the SDK bridge
  src/index.ts          # WebSocket server that runs @earendil-works/pi-coding-agent
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
src/views/
  thread_list.rs        # Home window showing pinned/unpinned threads
  chat_app.rs           # Per-thread window frame: gpui_component TitleBar with pin/export/workspace controls, wraps ChatWindow in Root
  chat_window.rs        # Per-thread chat content: model dropdown, workspace bar, message rendering via TextView
  user_panel.rs         # Account/auth/settings panel, including theme toggle, remote-control toggle and QR code
  workspace_manager.rs  # Workspace picker content rendered inside a gpui_component::Dialog
  reasoning.rs          # Collapsible thinking/reasoning display using gpui_component::Collapsible
  onboarding.rs         # First-run onboarding / import prompt from ~/.pi/agent/
assets/                 # SVG icons loaded at runtime
docs/                   # Internal reference: GPUI guides, design review, markdown improvement plan, TODO
scripts/                # Build scripts: macOS/Windows installers and generate_icons.py
```

## Build, Run and Test

```bash
# Install the SDK bridge dependencies (Bun only)
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

- `scripts/build-windows.ps1` produces `target/mini-pi-<version>-x64.msi`.
- `scripts/build-macos.sh` produces `target/mini-pi-<version>-x64.dmg`.

Both scripts download the platform-specific Bun runtime and bundle it alongside the `pi-bridge` source and its production dependencies, so end users do not need Bun or Node.js installed. They are also driven by `.github/workflows/release.yml` on version tags.

### Packaging layout

- Release binaries resolve `assets/` and `pi-bridge/` relative to the executable (`src/utils/paths.rs`), falling back to `CARGO_MANIFEST_DIR` during `cargo run`.
- macOS: `cargo-bundle` reads `[package.metadata.bundle]` in `Cargo.toml` and builds `Mini Pi.app` with resources under `Contents/Resources`.
- Windows: a hand-written WiX source (`wix/main.wxs`) plus a `heat`-generated file list produces a per-machine `.msi`.

### Prerequisites

1. **Rust stable** (>= 1.92)
2. **Platform toolchain:**
   - **macOS:** Xcode + Xcode Command Line Tools (`xcode-select --install`)
   - **Linux:** Vulkan drivers, `libxcb`, `libxkbcommon`, `libfontconfig`, `libssl`
   - **Windows:** Vulkan SDK or DirectX
3. **Bun** is the only supported runtime for the SDK bridge; `pi-bridge/node_modules` must be present for development (`cd pi-bridge && bun install`). The app spawns the bridge automatically and connects to it over a local WebSocket.
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
- **Sync metadata:** Stored in the `user_settings` table of `~/.mini-pi/mini-pi.db` under key `sync_meta` (migrated from legacy `~/.mini-pi/sync_meta.json` on first read)

### Database Migrations

Migrations are defined as a static slice of `(name, sql)` tuples in `src/data/store.rs` and tracked in a `_migrations` table. Current migrations:

- `001_init` — creates `threads` table
- `002_workspaces` — creates `workspaces` table
- `003_user_settings` — creates `user_settings` key-value table
- `004_thinking_level` — adds `thinking_level` column to `threads` table

### Process Model

On launch the app:

1. Opens SQLite and runs migrations.
2. Loads `AppConfig` from disk.
3. Attempts to restore the Supabase session (refresh if expired).
4. If logged in, spawns a background `smol` task to sync agent config changes.
5. Opens the `ThreadList` window.

On launch the app also spawns a single `PiBridge` process that runs the SDK bridge (`pi-bridge/src/index.ts`). The Rust GUI opens one shared WebSocket connection to the bridge and multiplexes all sessions over it.

Each chat thread opens its own window. Inside the window, `ChatWindow::spawn_pi` creates a `PiRpc` session handle that:

- Registers a session with the shared `PiBridge` via `{ type: "create_session", sessionId, sessionPath, cwd, model, thinkingLevel }`.
- Receives a per-session `futures::channel::mpsc` stream of `BridgeEvent`s from the bridge.
- Sends commands (`prompt`, `set_model`, `fork`, etc.) over the shared WebSocket; the bridge forwards SDK events back with the same `sessionId`.
- The `ChatWindow` GPUI task consumes the per-session event stream and updates messages/reasoning/tool-call state.

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

- `AppStore.thread_windows: HashMap<i64, AnyWindowHandle>` maps thread IDs to open chat windows to avoid duplicate windows for the same thread. Be aware that stale handles are not currently removed when a window is closed externally.
- All windows use `custom_window_options()` from `src/core/app.rs`: a transparent titlebar on macOS with traffic-light offset, and client-side decorations on other platforms.
- `TitleBar` is custom-rendered and includes platform-specific pin-to-top support (macOS `NSWindow` level, Windows `SetWindowPos`, Linux best-effort `wmctrl`).

### Key Bindings

Global bindings are registered in `main.rs`. Context-sensitive bindings use GPUI's `key_context` system (e.g. `"Input"` for `gpui_component::input::Input`). Common bindings:

- `Cmd/Ctrl + W` — Close window
- `Cmd + Q` — Quit
- `Enter` — Send message (in chat window)
- `Cmd + A` — Select all
- `Cmd + C/V/X` — Copy / Paste / Cut
- Arrow keys with Shift — Selection
- Home/End or `Ctrl+A/E` — Line start/end
- `Escape` — Close mention/command popups or workspace manager

Dropdowns handle `up`, `down`, `enter`, and `escape` internally. The chat input's `@` / `/` popup navigation is handled by an `on_key_down` listener on the input container.

## Code Organization Conventions

- **Entities:** All persistent UI state lives in GPUI `Entity<T>` structs. Views implement `Render`.
- **Events:** Components communicate via `EventEmitter<E>` + `cx.subscribe(...)` or `cx.observe(...)`.
- **Actions:** Global actions are declared with the `actions!` macro in `src/core/actions.rs`.
- **Globals:** `AppStore` is a GPUI `Global` holding `Arc<Store>`, config, auth state, session, sync state, and a window handle map.
- **Styling:** Uses GPUI's Tailwind-inspired fluent API (`div().flex().bg(rgb(...)).child(...)`).
- **Text input:** Custom inputs use `gpui_component::input::Input` / `InputState`:
  - `ui::chat_input::ChatInput` — multi-line chat input with `@` mention autocomplete and `/` slash-command palette support, backed by `gpui_component::input::InputState`
- **Markdown:** `gpui_component::text::TextView` / `TextViewState` renders assistant messages and the standalone markdown example.

## External SDK Dependency

This application is a thin GUI wrapper around the `@earendil-works/pi-coding-agent` SDK, run inside a local pi-bridge process. Release builds ship the Bun runtime plus a single bundled `pi-bridge.js` produced by `bun build --target bun --outfile pi-bridge.js`; the Rust app spawns `bun run pi-bridge.js` directly. For development, Bun and installed `pi-bridge/node_modules` are required. Chat functionality will fail at runtime when `PiBridge::spawn` is called if no Bun runtime or bridge bundle/source is available. The wire protocol is documented by the `BridgeEvent` enum and the multiplexed JSON messages in `src/rpc/pi_rpc.rs`.

## Security Considerations

- The Supabase anonymous key and URL are hardcoded in `src/auth/supabase.rs`.
- Auth tokens are stored in plaintext JSON inside the `user_settings` table of the local SQLite database (`~/.mini-pi/mini-pi.db`).
- Agent configuration and chat sessions are stored locally in the user's home directory.
- Cloudflare API credentials are read from environment variables only.

## Testing

- `cargo test` runs the remote-control tests in `src/remote/` and the markdown example doc-tests.
- The current codebase has **40 unit tests** plus doc-tests, including tunnel URL extraction, bearer-token validation, SSE framing, and HTTP-server integration (status, auth, `since_id`, SSE CORS headers, SSE heartbeat, and SSE query-token auth); all pass.
- `.github/workflows/release.yml` builds Windows (`.msi`) and macOS (`.dmg`) installers on version tags.

## Documentation

- `docs/at-mention-autocomplete.md` — Guide for implementing `@` mention autocomplete, derived from Zed's `agent_ui` crate.
- `docs/design-review.md` — Design review (in Chinese) that lists known architecture issues such as PiRpc process monitoring, sync file locking, stale window handles, and Store `Connection` thread safety.
- `docs/markdown-improvement-plan.md` — Planned markdown renderer improvements and known rendering bugs.
- `docs/TODO.md` — Short checklist of upcoming features.


## Notes for Agents

- `docs/` contains internal reference material, not project user documentation.
- The model list is loaded dynamically at startup from the SDK bridge via `ModelRegistry.getAvailable()` and stored in `AppStore.models`. `src/config/model_config.rs` exposes the helpers (`all_models`, `get_model_name`, `model_display_name`, `parse_model_id`) that take a `&[ModelInfo]` slice. Model IDs use a `<provider>:<model>` format parsed by `parse_model_id`.
- The slash-command / skill list is loaded once at startup from the SDK bridge and cached in `AppStore.commands` (`src/config/command_config.rs`). New composer `ChatInput`s are seeded with this list so the `/` popup works immediately; each session still refreshes the list via `get_commands` when it starts.
- When adding database changes, append a new migration tuple to `MIGRATIONS` in `src/data/store.rs`.
- Assets are loaded at runtime via `core::assets::Assets`. The asset root is resolved from the executable path (`src/utils/paths::app_root`), so packaged releases keep `assets/` next to the binary (Windows) or inside `Mini Pi.app/Contents/Resources` (macOS). During development the helper falls back to `CARGO_MANIFEST_DIR`.
- The `pi-bridge/` directory is resolved the same way. Release builds ship the platform-specific Bun binary (`bun` on macOS/Linux, `bun.exe` on Windows) plus a single bundled `pi-bridge.js` in the app root; during development the bridge is run with `bun run src/index.ts`.
- The app is primarily developed and tested on macOS. Windows-specific and Linux-specific code exists (e.g. `CREATE_NO_WINDOW`, client-side titlebar controls, `wmctrl`) but may need verification.
- The `pi-bridge/` directory must have its dependencies installed with `bun install` before running the app outside an installer.
- The wire protocol between Rust and the bridge uses a single WebSocket connection; every message includes a `sessionId` so multiple chat sessions can share one connection.
- Model IDs in `src/config/model_config.rs` must resolve through the SDK's `ModelRegistry`/`getModel`. Provider names like `cloudflare-ai-gateway` may not be recognized by the SDK and may need to be mapped to SDK-supported providers (`anthropic`, `openai`, etc.).
- Several known issues are documented in `docs/design-review.md`; review it before making large changes to process management, sync, or window lifecycle.
