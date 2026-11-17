# Running headless

A common setup: the device is plugged into a Raspberry Pi or a small Linux box on the bench or in a cabinet, and you (or an agent) work with it from elsewhere. This page covers running Termbridge there as a service.

## Option 1: SSH and the TUI

The simplest way. No service, nothing listening on the network.

```bash
ssh pi@bench-pi.local
termbridge tui
```

To keep the session running when you disconnect, start it inside `tmux` or `screen`:

```bash
tmux new -s serial
termbridge tui
# detach with Ctrl+B, then D; reattach later with: tmux attach -t serial
```

## Option 2: MCP over HTTP as a systemd service

This keeps an MCP endpoint running so agents can reach the device at any time.

### 1. Install the binary

Follow [Installation](Installation) for Linux, including adding the service user to `dialout`.

### 2. Create the service

Save this as `/etc/systemd/system/termbridge.service`. Replace `pi` with the user that owns the serial ports:

```ini
[Unit]
Description=Termbridge server
After=network-online.target
Wants=network-online.target

[Service]
User=pi
Group=dialout
ExecStart=/usr/local/bin/termbridge mcp --transport http --bind 127.0.0.1:8765
Restart=on-failure
RestartSec=2
Environment=RUST_LOG=info

[Install]
WantedBy=multi-user.target
```

### 3. Start it

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now termbridge
systemctl status termbridge
```

You should see `active (running)`. Check the endpoint:

```bash
curl -s http://127.0.0.1:8765/health
```

You should get `{"name":"termbridge","protocolVersion":"2025-06-18","sessions":0,"status":"ok","version":"0.1.0"}`. Logs are in `journalctl -u termbridge -f`.

### 4. Reach it from your computer

The service binds to `127.0.0.1` on purpose, because the endpoint has no authentication. Forward it over SSH instead of opening it to the network:

```bash
ssh -N -L 8765:127.0.0.1:8765 pi@bench-pi.local
```

Now `http://127.0.0.1:8765/mcp` on your computer reaches the Pi. Point your MCP client there (see [MCP Server](MCP-Server)).

If you must bind to a LAN address, put it on an isolated network and firewall the port to the machines that need it.

## Option 3: one-shot commands from scripts and cron

```bash
# Poll a sensor every minute and append the reply to a file
* * * * * /usr/local/bin/termbridge serial /dev/ttyUSB0 --baud 9600 --send "READ" --eol cr --read-ms 800 2>/dev/null >> /var/log/sensor.log
```

See [Command Line](Command-Line) for every option.

## Stable device names on Linux

`/dev/ttyUSB0` can become `/dev/ttyUSB1` after a reboot or replug. Use the persistent links instead:

```bash
ls -l /dev/serial/by-id/
```

Paths like `/dev/serial/by-id/usb-Silicon_Labs_CP2102N_USB_to_UART_Bridge_Controller_5c3e…-if00-port0` always point at the same adapter.
