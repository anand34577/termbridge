# Architecture

This page is for people changing the code. Serial MCP is a Cargo workspace with five crates; the binary is built from `crates/cli`.

```
            ┌──────────────┐   ┌──────────────┐   ┌──────────────┐
            │  crates/gui  │   │  crates/tui  │   │  crates/mcp  │
            │  egui/eframe │   │   ratatui    │   │ JSON-RPC MCP │
            └──────┬───────┘   └──────┬───────┘   └──────┬───────┘
                   │                  │                  │
                   └─────────┬────────┴─────────┬────────┘
                             │  SessionManager  │
                   ┌─────────┴──────────────────┴─────────┐
                   │              crates/core             │
                   │ serial · tcp client/server · udp     │
                   │ ring buffer · recording · auto-reply │
                   │ codecs · checksums · validation      │
                   └──────────────────────────────────────┘
                             crates/cli: the binary
```

## Sessions

`SessionManager` (`core/src/session/mod.rs`) owns every open connection. Opening one creates:

- a **worker**: a dedicated thread for serial ports (the `serialport` crate is blocking) or a Tokio task for sockets
- a **command channel** the worker reads writes and control changes from
- a **ring buffer** (`ring.rs`) of recent traffic, 1 MiB by default, where every chunk gets an increasing sequence number
- a **status** block with atomic byte and packet counters, modem lines, peer and client lists
- optional **recording** (`logging.rs`) and **auto-reply** (`responder.rs`)

Workers push every chunk through `WorkerCtx::push`, which updates counters, appends to the ring buffer, writes to the recording, sends a `SessionEvent`, and for received data runs the auto-reply rules and queues replies on the command channel.

The GUI, TUI and MCP never talk to workers directly. They call `SessionManager` and read the ring buffer with `read_since(seq)`, which is why any front end can watch a session another one opened.

## Front ends

- **GUI** (`crates/gui`): one `App` with pages, a `SessionView` per session holding UI state (input, history, filter, rate samples). Validation comes from `core::validate`, the line model from `core::view`.
- **TUI** (`crates/tui`): the same split. `theme.rs` detects colour depth and glyph support and adapts the themes; `form.rs` is the keyboard form. Click targets are rebuilt every frame for mouse and touch.
- **MCP** (`crates/mcp`): `server.rs` handles JSON-RPC and protocol negotiation, `tools.rs` defines and dispatches tools, `transport.rs` serves stdio and HTTP (axum).

## Configuration

`core::config::ConfigStore` loads the TOML file, clamps values, and saves atomically. All front ends share it, so macros and recent connections set in the GUI show up in the TUI.

## Tests

| Where | Covers |
| --- | --- |
| `core` unit tests | codecs, checksums against standard check values, auto-reply matching, recording formats, validation, config |
| `core/tests/transports.rs` | TCP and UDP over loopback, counters, control-line rules, auto-reply plus CSV recording end to end |
| `mcp/tests/protocol.rs` | initialize, tools, resources, prompts, batches, errors, checksum and auto-reply tools |
| `tui` tests | colour mapping, and a full keyboard and mouse session rendered into a test backend |

Run them with `cargo test --workspace`.
