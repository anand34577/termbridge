# Command line

Every transport also works without any interface, for scripts, CI jobs and quick checks. All commands accept `--config FILE` and `-v` (verbose logging on stderr).

## List ports

```bash
termbridge ports
```

```
PORT                     KIND     VID:PID    DESCRIPTION
/dev/ttyUSB0             usb      10C4:EA60  CP2102N USB to UART Bridge Controller
/dev/ttyACM0             usb      2E8A:000A  Pico
```

## Serial

Send one command and print what comes back within a second:

```bash
termbridge serial /dev/ttyUSB0 --baud 115200 --send "AT+GMR" --eol crlf --read-ms 1000
```

Send hex (a Modbus request with its CRC) and show the reply as hex:

```bash
termbridge serial COM4 --baud 9600 --parity even --send "01 03 00 00 00 0A C5 CD" --hex --format hex
```

Interactive terminal, logging everything:

```bash
termbridge serial COM4 --baud 9600 --interactive --log bench.log
```

| Option | Default | Meaning |
| --- | --- | --- |
| `--baud` | 115200 | Baud rate |
| `--data-bits` | 8 | 5 to 8 |
| `--parity` | none | none, even, odd (mark/space are rejected) |
| `--stop-bits` | 1 | 1 or 2 |
| `--flow` | none | none, rts-cts, xon-xoff |
| `--dtr`, `--rts` | off | Assert the line when opening |
| `--send TEXT` | | Data to send; escapes allowed |
| `--hex` | | Treat `--send` as hex |
| `--eol` | none | none, cr, lf, crlf added to `--send` |
| `--read-ms` | 1000 | How long to wait for replies |
| `--format` | ascii | How to print received data: ascii, hex, dec |
| `--log FILE` | | Record traffic |
| `--interactive` | | Read lines from the keyboard and send them |

## TCP and UDP

```bash
termbridge tcp-client 192.168.10.40 4001 --send "status" --eol cr
termbridge tcp-client 10.20.0.15 23 --nvt --interactive
termbridge tcp-server 9000 --echo --duration 60
termbridge udp --local-port 5005 --remote 192.168.10.255:5005 --broadcast --send "WHO"
termbridge scan --port 30718 --payload-hex "00 00 00 F8" --timeout-ms 1500
termbridge tcp-probe 192.168.10.40 4001
termbridge udp-listen 4000 --timeout-ms 5000
```

`tcp-server` runs until Ctrl+C unless `--duration` is set. `scan` sends one probe and lists every responder. `tcp-probe` measures connect latency and prints an optional banner; `udp-listen` collects unsolicited datagrams.

## Checksums

```bash
termbridge checksum "01 03 00 00 00 0A" --hex --algorithm crc16_modbus
termbridge checksum "AT+GMR\r" --algorithm all
```

`--algorithm` accepts `crc16_modbus`, `crc16_ccitt`, `crc16_xmodem`, `crc32`, `lrc`, `xor8`, `sum8`, or `all` (the default). The output shows the checksum bytes in append order plus the complete frame.

## Interactive commands

In `--interactive` mode, lines starting with `/` are commands:

| Command | Effect |
| --- | --- |
| `/help` | List commands |
| `/hex on` / `/hex off` | Send lines as hex |
| `/format ascii\|hex\|dec` | How received data is printed |
| `/eol none\|cr\|lf\|crlf` | Line ending added to each line |
| `/rts on\|off`, `/dtr on\|off`, `/break on\|off` | Serial control lines |
| `/log PATH` | Start recording |
| `/quit` | Exit |

## Exit codes

`0` on success, `1` when a port can't be opened, a host can't be reached or an argument is invalid.

On stdout you get one `session <id>: <label>` line followed by the received data. Connection events (`[info]`), errors (`[error]`) and Telnet negotiation (`[nvt]`) go to stderr, so `2>/dev/null` leaves just the session line and the data.

## Other commands

| Command | Does |
| --- | --- |
| `termbridge gui` | Desktop app |
| `termbridge tui` | Terminal UI, see [Terminal UI](Terminal-UI) |
| `termbridge mcp` | MCP server, see [MCP Server](MCP-Server) |
| `termbridge tcp-probe HOST PORT` | Connect latency and optional banner |
| `termbridge udp-listen PORT` | Collect unsolicited UDP datagrams |
| `termbridge checksum DATA` | CRC-16/32, LRC, XOR and SUM-8 |
| `termbridge config` | Config path, contents and an MCP client snippet |
