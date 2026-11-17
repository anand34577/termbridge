# Contributing

Bug reports, device compatibility notes and pull requests are all useful. If you're planning something large (a new transport, a protocol decoder), open an issue first so we can agree on the shape before you write it.

## Set up

You need Rust 1.95 or newer (`rustc --version`). On Linux also install:

```bash
sudo apt-get install -y build-essential pkg-config libudev-dev \
  libxkbcommon-dev libwayland-dev libx11-dev libxi-dev libgl1-mesa-dev
```

Then:

```bash
cargo build
cargo test --workspace
```

## Where things live

| Crate | What to change there |
| --- | --- |
| `crates/core` | Transports, sessions, codecs, checksums, auto-reply, recording, validation. Anything the GUI, TUI and MCP share. |
| `crates/mcp` | Tool schemas and handlers (`tools.rs`), protocol handling (`server.rs`), stdio/HTTP (`transport.rs`). |
| `crates/gui` | The egui app. `session.rs` is the session workspace, `forms.rs` the New connection dialog. |
| `crates/tui` | The ratatui app. `theme.rs` handles colours and terminal detection. |
| `crates/cli` | Command-line parsing and the headless commands. |

Put logic in `core` when more than one front end needs it. Validation rules belong in `core/src/validate.rs` so the GUI and TUI reject the same input.

## Before you open a pull request

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

- Add a test for bug fixes. Transport behaviour can usually be tested over loopback (see `crates/core/tests/transports.rs`); TUI flows can be driven off-screen (see the test at the bottom of `crates/tui/src/lib.rs`).
- If you change behaviour a user can see, update `README.md`, the matching page in `docs/wiki/`, and the Unreleased section of `CHANGELOG.md`.
- Keep commits focused. Commit messages follow Conventional Commits: `fix(serial): keep DTR low on open when unchecked`.

## Testing with real hardware

A USB-to-serial adapter with TX and RX bridged gives you a loopback for most serial tests. For devices you can't share, a CSV recording of the traffic (Record → CSV) attached to the issue is the next best thing.
