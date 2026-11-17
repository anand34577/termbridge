# Terminal UI

`serial-mcp tui` runs the same sessions in a terminal window. Use it over SSH, on a Raspberry Pi without a desktop, inside tmux, or from a phone with an SSH app. When you run plain `serial-mcp` on Linux without a display, you get the TUI automatically.

![The terminal UI](https://raw.githubusercontent.com/anand34577/serial-mcp/main/assets/screenshot-tui.png)

## Layout

- **Top line**: session count, MCP status, and on the right the theme and colour mode (or the latest notification).
- **Left**: sessions with a status dot and byte counts, and below them recent connections.
- **Header**: name, status, RX/TX lights, rates, packet counts, modem lines for serial ports, and a throughput graph on wide terminals. **REC** blinks while recording.
- **Terminal**: the traffic, newest at the bottom.
- **Composer**: the send line. Its title shows the input mode, line ending and checksum.
- **Bottom line**: clickable shortcut hints.

## Keys

| Keys | Action |
| --- | --- |
| Ctrl+N | New connection |
| Tab / Shift+Tab | Next / previous session |
| Enter | Send |
| Up / Down | Send history |
| Ctrl+X | Text or hex input |
| Ctrl+E | Line ending: none, CR, LF, CRLF |
| Ctrl+K | Checksum: none, CRC-16 Modbus/CCITT/XMODEM, CRC-32, LRC, XOR, SUM-8 |
| Ctrl+F | View: text, hex dump, decimal |
| Ctrl+T | Timestamps on or off |
| Ctrl+L | Clear |
| Ctrl+O | Start or stop recording |
| Ctrl+S | Send a local file (up to 16 MiB) |
| Ctrl+R / Ctrl+D | Toggle RTS / DTR (serial) |
| Ctrl+U | Clear the input line |
| Ctrl+P | Next colour theme |
| F1 to F12 | Macros |
| PgUp / PgDn / Esc | Scroll back / forward / back to live |
| Ctrl+W | Close session |
| Ctrl+G | Help |
| Ctrl+Q | Quit |

### New connection form

Up and Down move between fields. Left and Right change the transport on the first row, cycle choices such as parity, flip toggles, step through detected serial ports on the **Port** field and through common baud rates on **Baud rate**. Type to edit text fields. Enter opens; problems are listed in red under the form and nothing opens until they're fixed.

## Mouse and touch

Mouse reporting is on, which is what lets tapping work in terminal apps like htop and Midnight Commander. You can:

- tap a session or a recent connection
- tap transports, fields, choices and the **Open** / **Cancel** buttons in dialogs
- tap a shortcut in the bottom line
- scroll or swipe the terminal and the session list

Phone SSH apps (Termux, JuiceSSH, Blink) send taps as clicks once mouse reporting is on. To select text for copying, hold Shift while dragging (Option on macOS Terminal and iTerm2); most terminals let Shift bypass mouse reporting.

## Colours and characters

The TUI checks what your terminal can show and adapts:

| Terminal | Colours | Characters |
| --- | --- | --- |
| Windows Terminal, iTerm2, WezTerm, Kitty, Alacritty, GNOME Terminal, VS Code | truecolor | Full: rounded borders, braille spinner, graph |
| macOS Terminal, xterm, tmux with `TERM=*-256color` | 256 colours | Full |
| Windows console host (conhost) | truecolor | Basic symbols |
| Linux text console (`TERM=linux`), serial consoles | 16 colours | ASCII |
| `TERM=dumb` | none, uses bold and reverse video | ASCII |
| Any terminal with `NO_COLOR` set | none, uses bold and reverse video | Unchanged |

Override detection when it gets it wrong:

```bash
serial-mcp tui --colors 256          # truecolor, 256, 16 or none
serial-mcp tui --ascii               # plain ASCII borders and symbols
serial-mcp tui --theme "High contrast"
```

The same settings work as environment variables: `SERIAL_MCP_COLORS`, `SERIAL_MCP_GLYPHS=ascii|basic|full`, `NO_COLOR=1`.

Themes: **Midnight** (default), **Arctic**, **Ember**, **Light** (for light terminal backgrounds) and **High contrast**. Ctrl+P cycles them and the choice is saved.

## Sharing sessions with an agent

```bash
serial-mcp tui --mcp 127.0.0.1:8765
```

Agents connecting to `http://127.0.0.1:8765/mcp` see and use the same sessions, and anything they open appears in the TUI. See [MCP Server](MCP-Server).

The terminal must be at least 60×16 characters.
