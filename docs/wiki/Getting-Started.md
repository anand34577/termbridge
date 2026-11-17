# Getting started

This walks through opening a serial port, sending a command and reading the answer in the desktop app. If you don't have a device handy, step 6 shows how to try everything with no hardware at all.

## 1. Plug in the device

Connect your USB-to-serial adapter or board. On Windows it shows up as a `COM` port, on Linux as `/dev/ttyUSB0` or `/dev/ttyACM0`, on macOS as `/dev/cu.usbserial-…`.

You can list ports from a terminal:

```bash
serial-mcp ports
```

You should see something like:

```
PORT                     KIND     VID:PID    DESCRIPTION
COM4                     usb      0403:6001  FT232R USB UART
```

## 2. Open the app

Run `serial-mcp` (or double-click it on Windows). You'll see the **Start a session** screen with four cards: Serial port, TCP client, TCP server and UDP.

## 3. Configure the port

1. Click **Serial port** (or press Ctrl+N).
2. Pick your port from **Detected**. The product name and USB ID appear under the field so you can tell adapters apart.
3. Set **Baud rate** to what the device uses. 9600 and 115200 are the most common; check the device's manual.
4. Leave **Data bits 8, Parity None, Stop bits 1** unless the manual says otherwise. This is the usual "8N1".
5. Look at the line at the bottom of the dialog. It summarises your choice, for example `COM4  9600 8N1  no flow control`.
6. Click **Open port**.

If something is wrong (a baud rate out of range, a port that's already open), the field turns red with an explanation and **Open port** stays disabled. A port that isn't in the detected list gets a warning but can still be opened.

## 4. Send a command

1. Type into the box at the bottom, for example `AT`.
2. Most devices expect a line ending. Pick **CR** or **CRLF** under **Line end**.
3. Press Enter.

The terminal shows your command tagged **TX** in amber and the reply tagged **RX** in teal. The byte preview on the right of the composer shows exactly what was sent, for example `4 B  41 54 0D 0A`.

Special characters work as escapes: `\r` (carriage return), `\n` (line feed), `\x1B` (any byte in hex), `$0D` and `\#013` (Hercules-style hex and decimal). Switch the composer to **Hex** to type raw bytes like `01 03 00 00 00 0A`.

## 5. Look at the data

- **Text / Hex / Dec** switch how received bytes are shown. Hex gives a classic hex dump with an ASCII column.
- **Filter** hides lines that don't contain your search.
- **Record** saves traffic to a file. See [Recording and Auto-Reply](Recording-and-Auto-Reply).

## 6. Try it without hardware

The app can talk to itself over TCP:

1. Ctrl+N → **TCP server** → **Local only** → port `9000` → tick **Echo** → **Start listening**.
2. Ctrl+N → **TCP client** → host `127.0.0.1`, port `9000` → **Connect**.
3. In the client session, type `hello` and press Enter.

You should see `hello` as TX, and the same text come back as RX because the server echoes it.

## What's next

- [Desktop App](Desktop-App) explains every control.
- [MCP Server](MCP-Server) connects an AI agent to the sessions you just opened.
- [Terminal UI](Terminal-UI) is the same workflow over SSH.
