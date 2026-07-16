# Code Critique — Open Issues

Remaining issues from the `src/` critique pass conducted on 2026-06-25.
The top 5 (items #1, #2/#9, #3/#16/#17, #4/#5, #5/#11/#12 below) have been
fixed; everything else is still open. Numbering matches the original
critique. Severity ordering is roughly architectural → concurrency →
correctness → style.

Cross-reference: items already addressed live in `docs/design-review.md`
and the git history; this file documents what's *left*.

---

## Architecture / structural

### #3. Stale window handles
`src/core/app.rs:23` — `thread_windows: HashMap<String, AnyWindowHandle>`
is never pruned on window close. AGENTS.md already acknowledges this.
Leaks handles and breaks the "single window per thread" invariant after a
window is closed and then the same thread is reopened.

### #4. Two async runtimes coexist
GPUI spawns via `smol`; `PiBridge`, the remote `axum` server, and
`tunnel` use a hand-rolled `tokio::Runtime` (`src/rpc/pi_rpc.rs:221`,
`src/remote/controller.rs`). Cross-runtime `cx.spawn` + `block_on`
patterns (`pi_rpc.rs:344`) are subtle and will be a long-term source of
deadlocks. Pick one (GPUI is smol-based, so lean on `smol` or push all
blocking IO onto a dedicated thread pool).

### #30. Useless config write at startup
`src/app.rs:33` writes `config.remote_control.enabled = false` to disk
on every launch. Just skip the save if the field is already false.

### #31. Hard exit on DB open failure
`src/app.rs:30` does `Store::open().expect("failed to open database")`
before any UI can render a helpful dialog. At least show an error window.

### #32. Dead `BackPressed` arm
`src/app.rs:275` (now in `MiniPiApp::new`) and the matching handler in
`ThreadList` both no-op `UserPanelEvent::BackPressed`. Either delete the
variant or implement.

### #35. Logging via scattered `eprintln!` + per-module `log!` macros
Spread across 6+ files. Pull in `tracing` with a single subscriber;
structured spans would also make the multi-runtime story easier to debug.

---

## Concurrency / panic-safety

### #6. `PiBridge::drop` is racy and incomplete
`src/rpc/pi_rpc.rs:155` only `child.kill()`s; it doesn't drop the tokio
runtime cleanly, doesn't close the writer channel, and doesn't notify
sessions of `Disconnected` until the WS reader exits. The monitor thread
(`pi_rpc.rs:214`) holds `child.lock().unwrap()` for the entire duration of
`wait()` — every other `child.lock()` site blocks until the process dies.
Snapshot `Arc::clone(child).try_lock()` or move the handle into the
monitor thread.

### #7. `.lock().unwrap()` accepts poisoning silently
`pi_rpc.rs:215/255/275/299/329/339/370`, `tunnel.rs:135`, `voice.rs:45/64`
all still use `std::sync::Mutex` with `unwrap()`. A single panic on the
lock-holding thread will poison the mutex and cascade panics through
unrelated sessions. The `Store` migration to `parking_lot::Mutex` in
`data/store.rs` should be replicated here, or handle `PoisonError`
explicitly. (Note: `parking_lot` is now a dependency after the store fix,
so the swap in `pi_rpc.rs` is cheap.)

### #8. Real-time audio thread uses `std::sync::Mutex`
`src/utils/voice.rs:45` locks a `Mutex<Vec<f32>>` inside the cpal
callback. A short-lived lock held while the UI thread builds the WAV can
cause dropouts and (worse) the callback can be invoked reentrantly. Use a
lock-free ring buffer (`heapless::spsc::Queue`, `ringbuf`, or
double-buffered `crossbeam`).

### #28. TOCTOU on `RemoteController` status transitions
`set_enabled(false)` from `set_error` (`controller.rs:818`) writes config
while a tunnel task may still be in flight. `begin_start` spawned a
`cx.spawn` that holds `WeakEntity`; if the user toggles off before the
tunnel resolves, the spawn still mutates `status`. The `keep` /
`should_apply` checks are scattered across the spawn closure and easy to
miss. Centralize status transitions behind a single
`transition(target, cx)` method.

---

## Database / store

### #13. `list_threads` loads every row
`src/data/store.rs:185` has no `LIMIT`. With a year of history this is a
full table scan + N `serde_json` parses on every render. Add `LIMIT ?`
and use the paginated API from `ThreadList`.

### #14. `search_threads` LIKE without index
`src/data/store.rs:267` does `lower(title) LIKE ?1`. Create a
`COLLATE NOCASE` index or migrate to FTS5 — current is O(n) on every
keystroke if used live.

### #15. Row-mapping still partially duplicated
The five copies of `row.get(N)?` mapping in `store.rs` were collapsed
into `row_to_thread` / `row_to_workspace` helpers. Worth auditing
`core/session_handle.rs` and `remote/controller.rs` for similar
`ThreadMeta`/`WorkspaceMeta` construction sites and reusing the helpers.

### #36. `MIGRATIONS` has no checksum/version check
`src/data/store.rs:31` (now under `parking_lot::Mutex`) records migrations
by name only. A user who manually edited the db could mis-record
`_migrations` and skip a needed `ALTER`. At minimum store a SHA of `sql`
next to `name`.

---

## Auth / supabase

### #18. `signup` returns `Api { status: 200, … }` for a "no tokens" state
`src/auth/supabase.rs:115` (line numbers shifted after the shared-client
fix). The 200 in the success-but-error path is misleading to callers that
branch on `status >= 400`. Use a dedicated
`SupabaseAuthError::EmailConfirmationRequired`.

### #19. Hardcoded Supabase URL + anon key in the binary
`src/auth/supabase.rs:5-7`. For an SPA-style anon key that's somewhat
expected, but every release embeds it; at minimum gate the SDK behind
feature flags and don't ship it in test builds.

### #20. `load_session` deletes legacy `auth.json` as a side effect
`src/auth/state.rs:88-104` removes a user file when `load_session` is
called. Surprising and untestable without a real home dir. Make migration
an explicit one-shot on first run (alongside `import_from_pi_agent`).

---

## pi-bridge / RPC

### #22. `read_bridge_port` blocks with no timeout
`src/rpc/pi_rpc.rs:789` reads bridge stdout synchronously with no
deadline. If `pi-bridge` writes nothing, the app hangs forever silently.
Add a deadline — the entire `PiBridge::spawn` should be cancelable.

### #23. `get_models` waits for `command == "get_models"` but ignores `request_id`
`src/rpc/pi_rpc.rs:325` generates a `request_id` and never uses it in the
filter. If the bridge responds to a *different* message first (e.g. an
unsolicited `error`), the loop spins forever. Filter by `request_id` and
bail on `BridgeEvent::Error`.

### #24. `parse_pi_line_value` silently swallows malformed events
`src/rpc/pi_rpc.rs:868` returns `None` on unknown `type`/shape with only
an `eprintln!`. No metric/counter for protocol drift in production. Add a
debug counter or surface the unparsed line via `BridgeEvent::Unknown`.

### #25. `send_extension_ui_response` blindly overwrites `type`
`src/rpc/pi_rpc.rs:706` `obj.insert`s `type` over whatever the caller
supplied. If `response` is already an Object with a conflicting `type`,
it's silently overwritten. Document this contract or error.

---

## Remote control

### #26. `RemoteController` mixes I/O, business logic, and protocol serialization
`src/remote/controller.rs` is 1400+ lines; `AiSubmitStream::sync_parts`
indexes into `MessagePart` with `usize` and tracks `sent_len` as bytes
(`controller.rs:484`). If the SDK replaces a part's text with a shorter
one mid-stream (edit/regen), `unsent_suffix` silently returns no delta
and `done` never fires. Track `(len, hash)` or reset the part on length
regression.

### #27. `download_file` re-canonicalizes every workspace per call
`src/remote/controller.rs:1258` calls `std::fs::canonicalize` on every
workspace path every request. Cache `canonical_ws` per workspace entity.

### #29. `stable_tool_call_id` falls back to `tool-{index}`
`src/remote/controller.rs:471` falls back to `tool-{index}` when the SDK
gives an empty id, but `AiPartState::Tool.id` for a `ToolResult` is
matched by the same index — if the SDK reorders parts, you'll stream the
wrong output to the wrong tool card. Use a monotonic counter or the part's
stable id.

---

## Misc / style

### #33. `open_file` flashes a console window on Windows
`src/views/chat_window.rs:48` uses `cmd /c start "" <path>` without
`CREATE_NO_WINDOW` — a console flashes. Wrap with
`creation_flags(0x08000000)` like `pi_rpc.rs:182`.

### #34. AGENTS.md doc drift on `thread_windows` key type
AGENTS.md says `thread_windows: HashMap<i64, …>` but the source uses
`HashMap<String, …>` (`src/core/app.rs:23`). Fix one.

---

## Tests

### #37. Coverage gaps
Only the 4 unit tests in `pi_rpc.rs` + 1 in `auth/state.rs` touch the
critique area. Nothing covers `data/store.rs`, `sync/settings_sync.rs`,
`remote/controller.rs`. The `AiSubmitStream::sync_parts` byte-delta logic
in particular is pure and begging for property tests:
- length regression (text gets shorter mid-stream)
- mid-stream tool-call reorder
- multi-byte UTF-8 deltas crossing `sent_len`
- `done` fired twice
- session closed mid-stream

After the store rewrite (#1 fix) it's now possible to spin up an
in-memory `Store` per test (open a `Connection::open_in_memory()`, run
migrations) — worth adding a `Store::open_with_conn` test helper.