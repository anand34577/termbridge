# Installation

Serial MCP is a single program file. There's no installer and nothing else to set up; you put the file somewhere and run it.

## Which download

Releases are on the [Releases page](https://github.com/anand34577/serial-mcp/releases). Pick the file that matches your computer:

| Computer | File | Includes |
| --- | --- | --- |
| Windows PC (Intel/AMD) | `serial-mcp-x86_64-pc-windows-msvc` | GUI, TUI, MCP, CLI |
| Windows on ARM (Surface Pro X, Snapdragon laptops) | `serial-mcp-aarch64-pc-windows-msvc` | GUI, TUI, MCP, CLI |
| Mac with Apple Silicon (M1 to M4) | `serial-mcp-aarch64-apple-darwin` | GUI, TUI, MCP, CLI |
| Intel Mac | `serial-mcp-x86_64-apple-darwin` | GUI, TUI, MCP, CLI |
| Linux PC | `serial-mcp-x86_64-unknown-linux-gnu` | GUI, TUI, MCP, CLI |
| Linux PC, any distro, no dependencies | `serial-mcp-x86_64-unknown-linux-musl` | TUI, MCP, CLI |
| Raspberry Pi 4/5 or other 64-bit ARM board | `serial-mcp-aarch64-unknown-linux-gnu` | TUI, MCP, CLI |
| Raspberry Pi 2/3 (32-bit OS) and ARMv7 boards | `serial-mcp-armv7-unknown-linux-gnueabihf` | TUI, MCP, CLI |

Not sure which Linux build you need? Run `uname -m`. `x86_64` means the PC builds, `aarch64` the ARM64 build, `armv7l` the ARMv7 build.

## Windows

1. Download the Windows `.tar.gz` file and open it (Windows 11 opens `.tar.gz` in Explorer; on Windows 10 use 7-Zip).
2. Copy `serial-mcp-x86_64-pc-windows-msvc.exe` to a folder such as `C:\Tools\` and rename it to `serial-mcp.exe`.
3. Double-click it. The desktop app opens.

Windows SmartScreen may say the app is from an unknown publisher, because the binary isn't code-signed. Click **More info**, then **Run anyway**.

To run it from any terminal, add the folder to your `PATH`: Start → "Edit the system environment variables" → Environment Variables → `Path` → New → `C:\Tools`. Open a new terminal and check:

```powershell
serial-mcp --version
```

You should see `serial-mcp 0.1.0`.

Most USB-to-serial adapters (FTDI, CP210x, CH340, PL2303) install their drivers automatically through Windows Update. If the port doesn't appear in Device Manager under **Ports (COM & LPT)**, install the vendor driver.

## macOS

1. Download the file for your Mac and extract it.
2. Move the binary into place and make it executable:

   ```bash
   sudo mv serial-mcp-aarch64-apple-darwin /usr/local/bin/serial-mcp
   sudo chmod +x /usr/local/bin/serial-mcp
   ```

3. The first launch is blocked by Gatekeeper because the binary isn't notarized. Clear the quarantine flag once:

   ```bash
   xattr -d com.apple.quarantine /usr/local/bin/serial-mcp
   ```

4. Run `serial-mcp`. The desktop app opens.

On macOS, serial ports show up as `/dev/cu.usbserial-XXXX` or `/dev/cu.usbmodemXXXX`. Use the `cu.` names, not `tty.`; the `tty.` devices wait for a carrier signal and often seem to hang.

## Linux and Raspberry Pi

1. Download and install:

   ```bash
   tar -xzf serial-mcp-aarch64-unknown-linux-gnu.tar.gz
   sudo install -m 755 serial-mcp-aarch64-unknown-linux-gnu /usr/local/bin/serial-mcp
   ```

2. Give your user access to serial ports. On Debian, Ubuntu and Raspberry Pi OS the group is `dialout`; on Arch and Fedora it's `uucp` or `dialout`:

   ```bash
   sudo usermod -aG dialout "$USER"
   ```

   Log out and back in (or reboot) for this to take effect. Check with `groups`; `dialout` should be in the list.

3. Run it:

   ```bash
   serial-mcp            # desktop app if you have a desktop session, otherwise the TUI
   serial-mcp tui        # always the terminal UI
   ```

The `gnu` builds need `libudev` at run time, which every desktop and server distro already has. The GUI build also loads X11 or Wayland and OpenGL when you open the GUI; without a display it simply starts the TUI. If your system has none of these (Alpine, minimal containers), use the `musl` build.

## Build from source

You need Rust 1.95 or newer. Check with `rustc --version`; install or update from [rustup.rs](https://rustup.rs).

```bash
git clone https://github.com/anand34577/serial-mcp.git
cd serial-mcp
cargo build --release
```

The binary is `target/release/serial-mcp` (`.exe` on Windows). On Linux, install the build packages first:

```bash
sudo apt-get install -y build-essential pkg-config libudev-dev \
  libxkbcommon-dev libwayland-dev libx11-dev libxi-dev libgl1-mesa-dev
```

For a build without the desktop GUI (smaller, no graphics packages needed):

```bash
cargo build --release --no-default-features --features tui,udev
```

Next: [Getting Started](Getting-Started).
