# FAQ

### What escape syntax does it use for control bytes?

Text mode accepts `\r`, `\n`, `\t`, `\0`, `\xNN` for any byte, `$NN` for hex and `\#NNN` for decimal, so `$0D` and `\#013` both produce a carriage return. `$$`, `##` and `<<` give a literal `$`, `#` or `<`. Switch the composer to **Hex** to type raw bytes directly.

### Does it support RS-485?

Yes, through a USB-to-RS-485 adapter that handles direction switching itself, which almost all of them do. Termbridge sees it as an ordinary serial port.

### Does it support Modbus?

It doesn't decode Modbus, but it makes sending Modbus RTU frames easy: type the request in hex and pick **CRC-16 Modbus** as the checksum, and the CRC is appended for you. The Tools page computes CRCs for checking replies, and an AI agent can do the same through MCP.

### Can two programs use the same serial port?

Not at the OS level; a serial port has one owner. Within Termbridge, the GUI or TUI and any number of MCP clients connected to its HTTP endpoint can all use the same session at once.

### Is RFC 2217 (serial over Telnet) supported?

Not yet. The TCP client handles plain Telnet option negotiation (NVT), so it works with device servers in raw or Telnet mode, but it can't change a remote port's baud rate.

### How much data can a session hold?

The terminal view keeps the last 20,000 lines, and each session keeps the last 1 MiB of traffic in memory for reading back (configurable up to 64 MiB in Settings). To keep everything, start a recording; files have no size limit.

### Is the MCP endpoint safe to expose?

No. It has no authentication, and anyone who can reach it can use your serial ports and open network connections. Keep it on `127.0.0.1` and use an SSH tunnel to reach it remotely. See [Running Headless](Running-Headless).

### Where are my macros and recent connections stored?

In the config file. See [Configuration](Configuration).

### Why is the binary 14 MB?

It contains the GPU-rendered desktop GUI, the terminal UI, an HTTP server and the MCP protocol in one file. The build without the GUI (`--no-default-features --features tui,udev`) is considerably smaller.
