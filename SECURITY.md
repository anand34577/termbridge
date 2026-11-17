# Security

## Reporting a vulnerability

Please report security problems privately through [GitHub security advisories](https://github.com/anand34577/termbridge/security/advisories/new), not in a public issue. Include the version (`termbridge --version`), your platform, and steps to reproduce. You'll get a reply within a week.

## What to keep in mind when running it

- **The HTTP MCP endpoint has no authentication.** Anyone who can reach it can open serial ports, connect to hosts, listen on ports and send data. It binds to `127.0.0.1` by default; only bind to another address on a network you trust. The GUI shows a warning when you do.
- **MCP gives agents real access to hardware.** An agent connected over stdio or HTTP can write to any serial port the user account can open. Only connect agents you trust, and prefer stdio for single-agent setups.
- **Recordings can contain sensitive data.** Text, CSV and raw recordings store everything that crossed the link, including passwords typed into device consoles.
- **TCP and UDP servers listen on all interfaces by default** (`0.0.0.0`). Choose "Local only" in the New connection dialog when you don't need remote clients.

## Supported versions

Security fixes go into the latest release.
