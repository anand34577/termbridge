# MCP reference

Termbridge implements MCP protocol revisions `2024-11-05`, `2025-03-26` and `2025-06-18` over two transports:

- **stdio**: newline-delimited JSON-RPC on stdin/stdout, logs on stderr.
- **HTTP**: `POST /mcp` with JSON-RPC bodies, `GET /health`, CORS enabled, no authentication.

```bash
termbridge mcp --transport stdio
termbridge mcp --transport http --bind 127.0.0.1:8765
```

## Typical agent workflow

1. `list_serial_ports`: find the device.
2. `open_serial` (or `open_tcp_client`, `open_tcp_server`, `open_udp`): get a `session_id`. `open_tcp_client` waits for the connection (up to `connect_timeout_ms`) and fails if it cannot connect; with `reconnect: true` it returns immediately and keeps retrying in the background.
3. `write_session`: send a command.
4. `wait_for_data` / `read_session`: collect the response.
5. `get_session`: inspect counters, modem lines, errors.
6. `close_session`: release the port or socket.

`read_session` returns `next_seq`; pass it back as `since_seq` to avoid duplicates. The ring buffer keeps the most recent 1 MiB per session (configurable in the GUI settings).

## Request examples

### initialize

```json
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{
  "protocolVersion":"2025-06-18",
  "capabilities":{},
  "clientInfo":{"name":"my-agent","version":"1.0"}
}}
```

### Open a serial port

```json
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{
  "name":"open_serial",
  "arguments":{
    "port":"COM3",
    "baud":115200,
    "data_bits":8,
    "parity":"none",
    "stop_bits":1,
    "flow_control":"none",
    "dtr":true
  }
}}
```

Result:

```json
{"jsonrpc":"2.0","id":2,"result":{
  "content":[{"type":"text","text":"{ ... }"}],
  "structuredContent":{"session":{"id":"serial-1","kind":"serial","connected":true,"...":"..."}},
  "isError":false
}}
```

### Send and wait

```json
{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{
  "name":"write_session",
  "arguments":{"session_id":"serial-1","data":"AT\\r\\n"}
}}
```

```json
{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{
  "name":"wait_for_data",
  "arguments":{"session_id":"serial-1","pattern":"OK","timeout_ms":2000}
}}
```

### Read hex

```json
{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{
  "name":"read_session",
  "arguments":{"session_id":"serial-1","since_seq":0,"format":"hex","max_bytes":4096}
}}
```

### Loopback test

```json
{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{
  "name":"open_tcp_server",
  "arguments":{"bind":"127.0.0.1","port":9100,"echo":true}
}}
```

```json
{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{
  "name":"open_tcp_client",
  "arguments":{"host":"127.0.0.1","port":9100}
}}
```

### Discovery

```json
{"jsonrpc":"2.0","id":8,"method":"tools/call","params":{
  "name":"udp_scan",
  "arguments":{"target":"255.255.255.255","port":4000,"payload_hex":"00","timeout_ms":1500}
}}
```

### Device simulation with auto-responses

Make a TCP server answer like a modem, then record everything to CSV:

```json
{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{
  "name":"set_auto_responses",
  "arguments":{"session_id":"server-1","rules":[
    {"pattern":"AT\r","reply":"OK\r\n"},
    {"pattern":"01 03","reply":"01 03 02 00 2A","hex":true}
  ]}
}}
```

```json
{"jsonrpc":"2.0","id":10,"method":"tools/call","params":{
  "name":"set_logging",
  "arguments":{"session_id":"server-1","path":"capture.csv","format":"csv"}
}}
```

Rules match across chunk boundaries and keep hit counters (`get_auto_responses`). An empty `rules` list turns the responder off. Invalid rules are rejected and the previous rules stay active.

### Checksums

```json
{"jsonrpc":"2.0","id":11,"method":"tools/call","params":{
  "name":"compute_checksum",
  "arguments":{"data":"01 03 00 00 00 0A","format":"hex","algorithm":"crc16_modbus"}
}}
```

Returns `checksum_hex` (`C5 CD`) and `frame_hex`. Omit `algorithm` to get every supported checksum. `write_session` accepts the same names in its `checksum` argument and appends the checksum before the line ending, which makes Modbus RTU requests a single call:

```json
{"jsonrpc":"2.0","id":12,"method":"tools/call","params":{
  "name":"write_session",
  "arguments":{"session_id":"serial-1","data":"01 03 00 00 00 0A","format":"hex","checksum":"crc16_modbus"}
}}
```

## Recording formats

`set_logging` accepts `format`:

| Format | Content |
| --- | --- |
| `text` (default) | `[timestamp] RX escaped-data` per chunk |
| `csv` | `timestamp,direction,length,hex,text` with a header row |
| `raw` | Received bytes only, exactly as they arrived |

Files are flushed as each chunk arrives, and on stop/close. `get_session` reports the active recording (`recording.path`, `format`, `bytes`).

## Input formats

`write_session` accepts:

- `ascii` (default): `\r`, `\n`, `\t`, `\0`, `\xNN`, `\#ddd`, `$hh`, `##`, `$$`, `<<`.
- `hex`: `48 65 6C 6C 6F`, `0x48,0x65`, or `48656c6c6f`.
- `base64`: standard base64.

Optional `eol`: `none` (default), `cr`, `lf`, `crlf`.

Optional `checksum`: `crc16_modbus`, `crc16_ccitt`, `crc16_xmodem`, `crc32`, `lrc`, `xor8`, `sum8`.

## Resources

| URI | Content |
| --- | --- |
| `termbridge://info` | Version, platform, config path, uptime |
| `termbridge://ports` | Current serial port list |
| `termbridge://sessions` | All sessions |
| `termbridge://sessions/{id}/status` | One session's status |
| `termbridge://sessions/{id}/log` | Last 64 KiB of traffic |

## Prompts

- `diagnose_serial_link` (`port`, `baud`): opens the port, probes it, reports raw bytes and modem state.
- `echo_test` (`transport`): sets up a TCP or UDP loopback and verifies the echo.

## Errors

- JSON-RPC errors (`-32601` unknown method, `-32602` invalid params/tool) are protocol failures.
- Tool execution failures are returned as `result` with `"isError": true` and a text explanation, as the MCP specification recommends.

## Security

The HTTP transport has no authentication and is meant for `127.0.0.1` or trusted networks. Serial ports and listening sockets are powerful: only expose the server to agents you trust.
