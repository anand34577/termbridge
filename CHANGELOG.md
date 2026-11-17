# Changelog

All notable changes to this project are listed here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses [Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.1.0] - 2026-11-17

First release.

### Added

- Serial terminal: port list with USB VID:PID and product name, 50 to 4,000,000 baud, 5 to 8 data bits, five parity modes, 1 or 2 stop bits, RTS/CTS and XON/XOFF flow control, DTR/RTS outputs, CTS/DSR/RI/CD inputs and break.
- TCP client with connect timeout, auto-reconnect and Telnet (NVT) negotiation; TCP server for up to 512 clients with echo; UDP with broadcast and a default target.
- UDP broadcast discovery with a custom probe payload.
- Desktop GUI (egui) with a validated New connection dialog, recent connections, per-session metrics with throughput sparklines, RX/TX activity lights, filterable text/hex/decimal terminal, send history, repeat send, file send and drag and drop.
- Terminal UI (ratatui) with the same session engine, mouse and touch support, file send, five colour themes, automatic truecolor/256/16/no-colour detection and an ASCII mode for limited consoles.
- MCP server over stdio and HTTP with 24 tools, live resources and two prompts. The GUI can host the HTTP endpoint so agents share its sessions; so can the TUI with `--mcp`.
- Recording to text, CSV or raw binary, flushed as data arrives.
- Auto-reply rules that answer a received pattern with a reply, matched across chunk boundaries.
- Checksums: CRC-16 Modbus, CCITT and XMODEM, CRC-32, LRC, XOR and SUM-8, appended on send or computed in the Tools page.
- Twelve macros on F1 to F12, shared by the GUI, TUI and MCP.
- Headless commands: `ports`, `serial`, `tcp-client`, `tcp-server`, `udp`, `scan`, `tcp-probe`, `udp-listen`, `checksum`, `config`.
- Release builds for Windows, macOS and Linux on x86_64, plus ARM64 and ARMv7 Linux and Windows ARM64.

[Unreleased]: https://github.com/anand34577/termbridge/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/anand34577/termbridge/releases/tag/v0.1.0
