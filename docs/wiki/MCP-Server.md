# MCP server

The Model Context Protocol (MCP) is how AI assistants call external tools. Serial MCP's server gives an assistant tools to list ports, open sessions, send data, wait for replies, record, and compute checksums.

There are two ways to run it:

| Transport | How it runs | Sessions shared with |
| --- | --- | --- |
| **stdio** | The assistant starts its own `serial-mcp mcp` process | Nobody else; it's private to that assistant |
| **HTTP** | Inside the desktop app, the TUI, or `serial-mcp mcp --transport http` | Every client of that endpoint, and the GUI/TUI hosting it |

Use stdio for "let Claude talk to my board". Use HTTP when you want to watch what the agent does in the GUI, or share one port between several clients.

## Claude Code

```bash
claude mcp add serial-mcp -- /usr/local/bin/serial-mcp mcp --transport stdio
```

On Windows, use the full path to `serial-mcp.exe`. The desktop app's **MCP server** page shows this command with the right path, with a Copy button.

## Claude Desktop and Cursor

Add this to the client's MCP configuration (`claude_desktop_config.json` for Claude Desktop, `.cursor/mcp.json` for Cursor):

```json
{
  "mcpServers": {
    "serial-mcp": {
      "command": "C:\\Tools\\serial-mcp.exe",
      "args": ["mcp", "--transport", "stdio"]
    }
  }
}
```

Restart the client. `serial-mcp config` prints this snippet with the path of the binary you ran.

## HTTP inside the desktop app

1. Open **MCP server**.
2. Check **Bind address** (`127.0.0.1:8765` by default) and click **Start server**.
3. The status shows **Running** and the endpoint `http://127.0.0.1:8765/mcp`.
4. Point HTTP-capable clients at that URL:

   ```json
   { "mcpServers": { "serial-mcp": { "type": "http", "url": "http://127.0.0.1:8765/mcp" } } }
   ```

Tick **On launch** to start the endpoint whenever the app opens. The top bar shows **MCP online** while it runs.

If you bind to anything other than a loopback address, the page warns you: the endpoint has no authentication, and anyone who can reach it can use your ports.

## Tools

| Tool | What it does |
| --- | --- |
| `list_serial_ports` | Ports with USB VID:PID, manufacturer, product |
| `open_serial` | Open a port (baud, data bits, parity, stop bits, flow control, DTR/RTS) |
| `open_tcp_client`, `open_tcp_server`, `open_udp` | Open network sessions |
| `list_sessions`, `get_session` | Status, counters, modem lines, recording, rule count |
| `write_session` | Send text with escapes, hex or Base64; optional line ending and checksum |
| `read_session` | Read traffic since a sequence number; can wait for new data |
| `wait_for_data` | Wait until any data, or a specific pattern, arrives |
| `set_control_lines` | RTS, DTR, break; echo on servers and UDP |
| `set_logging` | Start or stop recording (text, CSV, raw) |
| `set_auto_responses`, `get_auto_responses` | Device simulation rules and hit counts |
| `compute_checksum` | CRC-16 Modbus/CCITT/XMODEM, CRC-32, LRC, XOR, SUM-8 |
| `clear_session_log`, `close_session` | Housekeeping |
| `send_file` | Stream a file in chunks |
| `list_macros`, `run_macro` | Use the macros from Settings |
| `udp_scan`, `udp_listen`, `tcp_probe` | Discovery and reachability |
| `get_server_info` | Version, platform, uptime, config path |

Resources (`serial-mcp://info`, `…/ports`, `…/sessions`, `…/sessions/{id}/status`, `…/sessions/{id}/log`) and two prompts (`diagnose_serial_link`, `echo_test`) are also available.

Request and response examples are in [docs/MCP.md](https://github.com/anand34577/serial-mcp/blob/main/docs/MCP.md).

## Things to ask an agent

- "List the serial ports and open the FTDI one at 115200."
- "Send `AT+GMR` with CRLF and tell me the firmware version."
- "Read holding registers 0 to 9 from Modbus slave 1 on COM4 at 9600 8E1." (The agent uses `write_session` with `checksum: crc16_modbus`.)
- "Record the next 30 seconds of the GPS session to CSV and summarise the fix quality."
- "Start a TCP server on port 5020 that answers `PING` with `PONG\r\n`, then connect to it and check it works."

## Reading data reliably

Every chunk in a session has a sequence number. `read_session` returns `next_seq`; pass it back as `since_seq` next time and you get only new data, with nothing skipped or repeated. Each session keeps the most recent 1 MiB (configurable) for readers that come in late.
