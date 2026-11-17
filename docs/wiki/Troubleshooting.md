# Troubleshooting

Error messages are quoted as they appear, so you can search this page for the text you see.

## Serial ports

### `open COM5: The system cannot find the file specified.`

The port doesn't exist (any more). The adapter was unplugged, Windows gave it a new number, or there's a typo. Run `termbridge ports` or use **Detected** / **Rescan** in the New connection dialog.

### `open COM5: Access is denied.`

Another program has the port open: Arduino IDE, PuTTY, a vendor tool, or another Termbridge session. Windows allows one program per port. Close the other program and try again.

### `open /dev/ttyUSB0: Permission denied`

Your Linux user isn't in the serial group. Add it and log out and back in:

```bash
sudo usermod -aG dialout "$USER"
```

### `open COM3: The semaphore timeout period has expired.`

Usually a Bluetooth serial port whose device isn't in range, or a driver that hung. Unplug and replug the adapter, or pick another port.

### The port opens but nothing comes back

Work through these in order:

1. **Baud rate.** Wrong baud shows up as garbage or nothing. Try 9600 and 115200.
2. **Line ending.** Many devices ignore a command until they see CR or CRLF. Set **Line end**.
3. **Wiring.** TX goes to RX. Two TX pins wired together give silence.
4. **Flow control.** If it's set to RTS/CTS and the CTS light is off, nothing is sent. Set it to None, or check the handshake wiring.
5. **DTR/RTS.** Some boards hold the MCU in reset while DTR is asserted (or need it to start). Toggle them on the control bar.
6. **Echo test.** Bridge TX and RX on the adapter and type. If you don't see RX, the adapter or driver is the problem.

### Garbled characters

Baud rate, parity or data bits don't match the device. "8N1" (8 data bits, no parity, 1 stop bit) is the most common; Modbus RTU often uses 8E1.

### `Mark/space parity is not supported by the serial backend.`

An error, shown before anything opens. The `serialport` backend exposes only none, even and odd parity. Pick one of those; most devices that ask for mark/space also work with even/odd for a quick test.

## TCP and UDP

### `connect to 192.168.1.50:4001 failed: … actively refused it`

Nothing is listening on that port. Check the device's IP and port, and that no other client holds its only connection (many serial device servers accept just one).

### `bind 0.0.0.0:9000: Only one usage of each socket address … is normally permitted`

On Linux: `Address already in use`. Another program, or another session, is already listening on that port. Pick a different port or close the other session. The New connection dialog warns you when one of your own sessions uses it.

### `This is a broadcast address. Enable Broadcast to send to it.`

Sending to `255.255.255.255` or a subnet broadcast like `192.168.1.255` needs **Allow broadcast** ticked (or `--broadcast` on the command line).

### Discovery finds nothing

- Broadcasts don't cross routers; the device has to be on the same subnet.
- The firewall may drop the replies. On Windows, allow `termbridge.exe` on private networks when asked.
- Many devices only answer their vendor's probe bytes; put them in **Payload (hex)**.

## Sending data

### `\# escape expects a decimal value 0..255`

`\#` must be followed by 1 to 3 digits up to 255, like `\#013`. To send a literal `#`, just type it.

### `\x escape expects two hex digits`

Write bytes as `\x0D`, not `\xD` or `\x`.

### `$` disappears or turns into another character

`$` followed by two hex digits is a byte escape (`$0D` is a carriage return). `$GPGGA` is safe because `G` isn't hex, but `$AB` becomes one byte. Write `$$` for a literal `$`.

## MCP

### The agent says the server isn't available

- Check the path in the client config points at the real binary. `termbridge config` prints a snippet with the correct path.
- Restart the client after editing its config.
- Run the command from the config yourself. `termbridge mcp --transport stdio` should start and wait silently.

### `invalid socket address syntax`

The `--bind` value must be an IP and port, like `127.0.0.1:8765`. Host names such as `localhost:8765` aren't accepted.

### The GUI's sessions don't show up for the agent

A stdio client starts its own private process. To share sessions, start the HTTP endpoint on the GUI's **MCP server** page (or `termbridge tui --mcp 127.0.0.1:8765`) and point the client at `http://127.0.0.1:8765/mcp`.

## Terminal UI

### Boxes, question marks or broken lines

The terminal font lacks some symbols. Use `termbridge tui --ascii`, or set `TERMBRIDGE_GLYPHS=ascii`.

### Colours look wrong or washed out

Force the colour depth your terminal supports: `--colors 256` or `--colors 16`. Over SSH, make sure `TERM` is passed through (`echo $TERM` should print something like `xterm-256color`).

### I can't select text with the mouse

Mouse reporting is on so tapping works. Hold Shift while dragging (Option in macOS Terminal and iTerm2).

### `Terminal too small. Resize to at least 60x16.`

Make the window larger or reduce the font size.

## Desktop app

### The app doesn't start on Linux

The GUI needs X11 or Wayland with OpenGL. Over SSH or on a server, use `termbridge tui`. If you have a desktop and it still fails, run `termbridge gui -v` and include the output in an issue.

### Windows says the app is from an unknown publisher

The binary isn't code-signed. Click **More info** → **Run anyway**.
