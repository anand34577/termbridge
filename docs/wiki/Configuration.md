# Configuration

Settings live in one TOML file (a plain text settings file) shared by the desktop app, the TUI and the headless commands. Most people never edit it by hand: the desktop app's **Settings** page writes it for you.

## Where the file is

| OS | Path |
| --- | --- |
| Windows | `%APPDATA%\termbridge\config.toml` |
| macOS | `~/Library/Application Support/termbridge/config.toml` |
| Linux | `~/.config/termbridge/config.toml` |

Print the exact path and the current contents:

```bash
termbridge config
```

Use a different file for one run with `--config path/to/file.toml`, or for every run by setting `TERMBRIDGE_CONFIG`. Handy for keeping a separate setup per project or per bench.

The file is created the first time something is saved. If it's missing or can't be parsed, defaults are used and nothing crashes.

## Settings

| Key | Default | What it does |
| --- | --- | --- |
| `theme` | `"dark"` | Desktop app theme: `"dark"` or `"light"` |
| `tui_theme` | `"Midnight"` | TUI theme: `Midnight`, `Arctic`, `Ember`, `Light`, `High contrast` |
| `show_timestamps` | `true` | Show timestamps in new sessions |
| `buffer_bytes` | `1048576` | Traffic kept in memory per session (1 MiB). Range 16 KiB to 64 MiB. Applies to sessions opened after a restart. |
| `log_dir` | not set | Default folder for recordings and saved views. Not set means your Documents folder. |
| `[mcp] bind` | `"127.0.0.1:8765"` | Address of the desktop app's HTTP MCP endpoint |
| `[mcp] autostart` | `false` | Start that endpoint when the desktop app opens |
| `[[macros]]` | CR, AT, Ping | Up to 12 macros, see below |
| `[[recent]]` | empty | The last 8 connections, written automatically |

Values outside their range are corrected when the file is loaded. Saves are atomic: the app writes a temporary file and renames it, so a crash mid-save can't leave a half-written config.

### Macros

```toml
[[macros]]
name = "Read regs"
data = "01 03 00 00 00 0A C5 CD"
hex = true

[[macros]]
name = "Firmware"
data = 'AT+GMR\r\n'
hex = false
```

The first macro is F1, the second F2, and so on up to F12. Text macros take the same escapes as the send box. Use single quotes in TOML so backslashes stay as typed. The desktop app's **Settings → Macros** table edits these with validation and reordering.

## Environment variables

| Variable | Effect |
| --- | --- |
| `TERMBRIDGE_CONFIG` | Path of the config file |
| `TERMBRIDGE_COLORS` | TUI colour depth: `truecolor`, `256`, `16`, `none` |
| `TERMBRIDGE_GLYPHS` | TUI characters: `full`, `basic`, `ascii` |
| `NO_COLOR` | Any value turns TUI colours off ([no-color.org](https://no-color.org)) |
| `RUST_LOG` | Log detail on stderr, for example `RUST_LOG=debug`. `-v` on any command does the same. |

## Port and connection limits

| Setting | Allowed |
| --- | --- |
| Baud rate | 50 to 4,000,000 |
| Data bits | 5, 6, 7, 8 |
| Stop bits | 1, 2 |
| TCP connect timeout | 100 ms to 120 s |
| TCP server clients | 1 to 512 |
| Repeat interval | 10 ms to 1 hour |
| Send file size | up to 16 MiB |
| Auto-reply rules | 32 per session, patterns up to 1,024 bytes |
