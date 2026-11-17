# Recording and auto-reply

## Recording

Recording writes a session's traffic to a file while it runs. It's per session, so you can record a device and the gateway it talks to into separate files.

### Start a recording

Desktop app:

1. On the session's control bar, click **Record**.
2. Pick a format: **Text log**, **CSV** or **Raw RX bytes**.
3. Check the file path. The default is your Documents folder (or the log folder from [Configuration](Configuration)) with the session name and the current time, for example `Survey_boat_GPS-20261117-093112.csv`.
4. Click **Start recording**.

The control bar now shows a pulsing **REC CSV 1.2 KiB** pill with the bytes written so far. Hover it to see the path; click **Stop** to finish.

Terminal UI: press Ctrl+O, Tab to choose the format, edit the path, Enter to start. Ctrl+O again stops.

Headless: `serial-mcp serial COM4 --interactive --log capture.log`.

MCP: `set_logging` with `format` set to `text`, `csv` or `raw`.

### Formats

| Format | Each entry | Good for |
| --- | --- | --- |
| Text log | `[2026-11-17T09:31:12.408Z] RX $GPGGA,093112.000,…\r\n` | Reading, grep, sharing in a bug report |
| CSV | `2026-11-17T09:31:12.408Z,RX,72,24 47 50 47 …,"$GPGGA,…\r\n"` | Excel, pandas, timing analysis |
| Raw RX bytes | Received bytes only, exactly as they arrived | Firmware dumps, binary protocols, feeding another tool |

CSV files start with the header `timestamp,direction,length,hex,text`. Timestamps are UTC. Directions are `RX`, `TX`, `INFO`, `ERROR` and `NVT`.

Existing files are appended to, never overwritten. Data reaches the disk as it arrives, and everything is flushed when you stop recording or close the session.

## Auto-reply

Auto-reply sends a reply the moment received data contains a pattern. Typical uses:

- **Simulate a device** while the real one is on someone else's desk: a TCP server that answers `AT` with `OK`.
- **Answer keep-alives** so a link doesn't time out while you watch it.
- **Acknowledge frames**: reply `06` (ACK) to every `02 … 03` frame.

### Set up rules

Desktop app: **Auto-reply** on the session's control bar.

1. **Add rule**.
2. Enter the **pattern** and the **reply**. Text fields take the same escapes as the composer (`AT\r`, `OK\r\n`). Tick **Hex** to enter both as hex bytes instead.
3. Click **Apply rules**.

The button becomes **Auto-reply (1)** and each rule shows its hit count. Untick a rule to pause it without deleting it.

Rules are checked against everything the session receives, in order of where the match ends in the data. A pattern split across two packets (`A` then `T\r`) still matches, and each occurrence triggers exactly once. A session can have up to 32 rules; patterns can be up to 1,024 bytes.

If a rule is invalid (empty pattern, bad hex), **Apply** shows the error and the previous rules stay in effect.

### Example: GPS receiver simulator

On a TCP server session:

| Pattern | Reply |
| --- | --- |
| `$PMTK605*31` | `$PMTK705,AXN_5.1.7_3333_19020118,0027,PA1616S,1.0*6E\r\n` |
| `$PMTK000*32` | `$PMTK001,0,3*30\r\n` |

Any client that connects and sends the firmware query gets the release string back, the same way the real module answers.

Auto-reply is available over MCP too (`set_auto_responses`, `get_auto_responses`), so an agent can set up a simulator and then test against it.
