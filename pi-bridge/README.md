# pi-bridge

A local WebSocket bridge that runs the `@earendil-works/pi-coding-agent` SDK for `mini-pi`.

The Rust GUI opens a single WebSocket connection to this bridge and multiplexes multiple agent sessions over it. Each message on the wire includes a `sessionId` so that events and commands are routed to the right session.

## Install

```bash
bun install
```

## Run standalone (for development)

```bash
bun run src/index.ts
```

The bridge will print `BRIDGE_PORT <port>` once it is listening. Connect a WebSocket client to `ws://127.0.0.1:<port>/`.

## Protocol

- Client -> Bridge: `{ "sessionId": "...", "type": "<command>", ... }`
- Bridge -> Client: `{ "sessionId": "...", "type": "<event>", ... }`

Supported commands: `create_session`, `prompt`, `steer`, `follow_up`, `abort`, `set_model`, `set_thinking_level`, `new_session`, `fork`, `clone`, `get_messages`, `get_commands`, `compact`, `export_html`, `extension_ui_response`.

Events are forwarded directly from the SDK (`agent_start`, `agent_end`, `message_update`, `tool_execution_*`, etc.).
