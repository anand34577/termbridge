# Installation

Releases are on the [Releases page](https://github.com/anand34577/termbridge/releases).
Each release ships three kinds of file. Pick the kind that suits you, then the
build that matches your computer.

- **Installer** — registers Termbridge with your system and gives you an
  uninstaller. Best if you just want it available and kept up to date.
- **Portable** — an archive you unpack and run. Best for a USB stick, a shared
  machine, or anywhere you cannot install software.
- **Bare binary** — the executable alone. Best for scripts, containers and
  building your own image.

## Which build

| Computer | Target |
| --- | --- |
| Windows PC (Intel/AMD) | `x86_64-pc-windows-msvc` |
| Windows on ARM (Surface Pro X, Snapdragon laptops) | `aarch64-pc-windows-msvc` |
| Mac with Apple Silicon (M1 to M4) | `aarch64-apple-darwin` |
| Intel Mac | `x86_64-apple-darwin` |
| Linux PC | `x86_64-unknown-linux-gnu` |
| Linux PC, any distro, no dependencies | `x86_64-unknown-linux-musl` |
| Raspberry Pi 4/5 or other 64-bit ARM board | `aarch64-unknown-linux-gnu` |
| Raspberry Pi 2/3 (32-bit OS) and ARMv7 boards | `armv7-unknown-linux-gnueabihf` |

Not sure which Linux build you need? Run `uname -m`. `x86_64` means the PC
builds, `aarch64` the ARM64 build, `armv7l` the ARMv7 build.

The `gnu` builds include the desktop GUI, the terminal UI, the MCP server and
the headless commands. The `musl` and ARM builds are TUI, MCP and CLI only:
they need no graphics libraries, which is what makes them usable on a minimal
server or an older Pi.

## Windows

**Installer.** Download `termbridge-x86_64-pc-windows-msvc.msi` and run it.
It installs to `Program Files\Termbridge` and registers an uninstaller in
Apps & features.

**Portable.** Download `termbridge-x86_64-pc-windows-msvc-portable.zip` and
extract it anywhere. Windows opens `.zip` natively; double-click
`termbridge-x86_64-pc-windows-msvc.exe`.

**Bare binary.** Download `termbridge-x86_64-pc-windows-msvc.exe` and put it
wherever you like.

Windows SmartScreen says the publisher is unknown because the binaries are not
code-signed. Click **More info**, then **Run anyway**. This applies to the
installer and the portable build alike.

To run it from any terminal, add its folder to your `PATH`: Start → "Edit the
system environment variables" → Environment Variables → `Path` → New. Open a
new terminal and check:

```powershell
termbridge --version
```

You should see `termbridge 0.1.1`.

Most USB-to-serial adapters (FTDI, CP210x, CH340, PL2303) install their drivers
automatically through Windows Update. If the port does not appear in Device
Manager under **Ports (COM & LPT)**, install the vendor driver.

## macOS

**Installer.** Download `termbridge-aarch64-apple-darwin.dmg`, open it and drag
**Termbridge** to Applications.

**Portable.** Download `termbridge-aarch64-apple-darwin-portable.zip` and
extract it wherever you like.

**Bare binary.** Download the executable and put it on your `PATH`:

```bash
sudo install -m 755 termbridge-aarch64-apple-darwin /usr/local/bin/termbridge
```

Gatekeeper blocks the first launch because the binaries are not notarized.
Clear the quarantine flag once:

```bash
xattr -d com.apple.quarantine /Applications/Termbridge.app
# or, for a bare binary:
xattr -d com.apple.quarantine /usr/local/bin/termbridge
```

On macOS, serial ports show up as `/dev/cu.usbserial-XXXX` or
`/dev/cu.usbmodemXXXX`. Use the `cu.` names, not `tty.`; the `tty.` devices wait
for a carrier signal and often seem to hang.

## Linux and Raspberry Pi

**Package.** Download the `.deb` for your architecture and install it:

```bash
sudo apt install ./termbridge_0.1.1_arm64.deb
```

That puts `termbridge` in `/usr/bin` and the licence notices in
`/usr/share/doc/termbridge/`. Remove it with `sudo apt remove termbridge`.

**Portable or bare binary.** Unpack or copy the executable into place:

```bash
tar -xzf termbridge-aarch64-unknown-linux-gnu-portable.tar.gz
sudo install -m 755 termbridge-aarch64-unknown-linux-gnu /usr/local/bin/termbridge
```

Then give your user access to serial ports. On Debian, Ubuntu and Raspberry Pi
OS the group is `dialout`; on Arch and Fedora it is `uucp` or `dialout`:

```bash
sudo usermod -aG dialout "$USER"
```

Log out and back in (or reboot) for this to take effect. Check with `groups`;
`dialout` should be in the list.

Run it:

```bash
termbridge            # desktop app if you have a desktop session, otherwise the TUI
termbridge tui        # always the terminal UI
```

The `gnu` builds need `libudev` at run time, which every desktop and server
distro already has. The GUI build also loads X11 or Wayland and OpenGL when you
open the GUI; without a display it simply starts the TUI. If your system has
none of these (Alpine, minimal containers), use the `musl` build.

## Upgrading from Serial MCP

Termbridge is the same program under a new name. On first run it moves an
existing `serial-mcp` configuration directory into place, so your recent
connections, macros and auto-reply rules carry over. Nothing needs to be
re-entered.

The `SERIAL_MCP_CONFIG` environment variable still works alongside
`TERMBRIDGE_CONFIG`, so an existing script or service file keeps reading the
file it was pointed at.

## Build from source

You need Rust 1.95 or newer. Check with `rustc --version`; install or update
from [rustup.rs](https://rustup.rs).

```bash
git clone https://github.com/anand34577/termbridge.git
cd termbridge
cargo build --release
```

The binary is `target/release/termbridge` (`.exe` on Windows). On Linux, install
the build packages first:

```bash
sudo apt-get install -y build-essential pkg-config libudev-dev \
  libxkbcommon-dev libwayland-dev libx11-dev libxi-dev libgl1-mesa-dev
```

For a build without the desktop GUI (smaller, no graphics packages needed):

```bash
cargo build --release --no-default-features --features tui,udev
```

Next: [Getting Started](Getting-Started).
