//! Serial MCP command line interface: GUI, MCP server and headless terminals.

use std::io::Write;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context};
use clap::{Args, Parser, Subcommand, ValueEnum};
use tokio::io::{AsyncBufReadExt, BufReader};

use serial_mcp_core::checksum::ChecksumKind;
use serial_mcp_core::codec::{self, ViewFormat};
use serial_mcp_core::config::ConfigStore;
use serial_mcp_core::session::{FlowControl, Parity};
use serial_mcp_core::{
    Direction, SerialConfig, SessionManager, TcpClientConfig, TcpServerConfig, UdpConfig,
};
use serial_mcp_mcp::McpServer;

#[derive(Parser)]
#[command(
    name = "serial-mcp",
    version,
    about = "Serial / TCP / UDP terminal with an MCP server",
    long_about = "A cross-platform serial, TCP and UDP terminal. Run the desktop GUI, expose serial and \
network terminals to AI agents over MCP, or use it headlessly from scripts."
)]
struct Cli {
    /// Configuration file (defaults to the platform config directory)
    #[arg(long, global = true, value_name = "FILE")]
    config: Option<PathBuf>,

    /// Verbose logging
    #[arg(short, long, global = true)]
    verbose: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Launch the desktop GUI (default when a display is available)
    Gui,
    /// Launch the terminal UI (works over SSH and on headless machines)
    Tui {
        /// Also serve MCP over HTTP on this address so agents share the sessions
        #[arg(long, value_name = "ADDR")]
        mcp: Option<String>,
        /// Colour theme: Midnight, Arctic, Ember, Light or "High contrast"
        #[arg(long)]
        theme: Option<String>,
        /// Colour depth: truecolor, 256, 16 or none (default: detect)
        #[arg(long, value_name = "DEPTH")]
        colors: Option<String>,
        /// Use plain ASCII glyphs and borders (for limited consoles)
        #[arg(long)]
        ascii: bool,
    },
    /// Run the MCP server (stdio or HTTP)
    Mcp {
        #[arg(long, value_enum, default_value_t = Transport::Stdio)]
        transport: Transport,
        /// HTTP bind address (only for --transport http)
        #[arg(long, default_value = "127.0.0.1:8765")]
        bind: String,
    },
    /// List serial ports
    Ports,
    /// Serial terminal
    Serial(SerialArgs),
    /// TCP client terminal
    TcpClient(TcpClientArgs),
    /// TCP server terminal
    TcpServer(TcpServerArgs),
    /// UDP terminal
    Udp(UdpArgs),
    /// UDP broadcast discovery
    Scan(ScanArgs),
    /// Probe a TCP endpoint (connect latency and optional banner)
    TcpProbe(TcpProbeArgs),
    /// Listen for UDP datagrams on a local port
    UdpListen(UdpListenArgs),
    /// Compute checksums over data
    Checksum(ChecksumArgs),
    /// Print the configuration path and an MCP client snippet
    Config,
}

#[derive(Clone, Copy, ValueEnum)]
enum Transport {
    Stdio,
    Http,
}

#[derive(Clone, Copy, ValueEnum)]
enum ParityArg {
    None,
    Even,
    Odd,
    Mark,
    Space,
}

impl From<ParityArg> for Parity {
    fn from(value: ParityArg) -> Self {
        match value {
            ParityArg::None => Parity::None,
            ParityArg::Even => Parity::Even,
            ParityArg::Odd => Parity::Odd,
            ParityArg::Mark => Parity::Mark,
            ParityArg::Space => Parity::Space,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum FlowArg {
    None,
    RtsCts,
    XonXoff,
}

impl From<FlowArg> for FlowControl {
    fn from(value: FlowArg) -> Self {
        match value {
            FlowArg::None => FlowControl::None,
            FlowArg::RtsCts => FlowControl::RtsCts,
            FlowArg::XonXoff => FlowControl::XonXoff,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum FormatArg {
    Ascii,
    Hex,
    Dec,
}

impl From<FormatArg> for ViewFormat {
    fn from(value: FormatArg) -> Self {
        match value {
            FormatArg::Ascii => ViewFormat::Ascii,
            FormatArg::Hex => ViewFormat::Hex,
            FormatArg::Dec => ViewFormat::Dec,
        }
    }
}

#[derive(Args)]
struct SerialArgs {
    /// Serial port, e.g. COM3 or /dev/ttyUSB0
    port: String,
    #[arg(long, default_value_t = 115200)]
    baud: u32,
    #[arg(long, default_value_t = 8)]
    data_bits: u8,
    #[arg(long, value_enum, default_value_t = ParityArg::None)]
    parity: ParityArg,
    #[arg(long, default_value_t = 1)]
    stop_bits: u8,
    #[arg(long, value_enum, default_value_t = FlowArg::None)]
    flow: FlowArg,
    #[arg(long)]
    dtr: bool,
    #[arg(long)]
    rts: bool,
    /// Data to send (supports \r \n \xNN and $hh escapes)
    #[arg(long, value_name = "TEXT")]
    send: Option<String>,
    /// Interpret --send as hex
    #[arg(long)]
    hex: bool,
    /// Line ending appended to --send: none, cr, lf or crlf
    #[arg(long, default_value = "none")]
    eol: String,
    /// How long to wait for incoming data
    #[arg(long, default_value_t = 1000)]
    read_ms: u64,
    /// Write session traffic to this file
    #[arg(long, value_name = "FILE")]
    log: Option<PathBuf>,
    /// Interactive line-mode terminal (send lines from stdin)
    #[arg(long)]
    interactive: bool,
    /// Output format for received data
    #[arg(long, value_enum, default_value_t = FormatArg::Ascii)]
    format: FormatArg,
}

#[derive(Args)]
struct TcpClientArgs {
    host: String,
    port: u16,
    /// Handle Telnet / NVT negotiation
    #[arg(long)]
    nvt: bool,
    /// Reconnect automatically after a disconnect
    #[arg(long)]
    reconnect: bool,
    #[arg(long, value_name = "TEXT")]
    send: Option<String>,
    #[arg(long)]
    hex: bool,
    #[arg(long, default_value = "none")]
    eol: String,
    #[arg(long, default_value_t = 1000)]
    read_ms: u64,
    #[arg(long, value_name = "FILE")]
    log: Option<PathBuf>,
    #[arg(long)]
    interactive: bool,
    #[arg(long, value_enum, default_value_t = FormatArg::Ascii)]
    format: FormatArg,
}

#[derive(Args)]
struct TcpServerArgs {
    #[arg(long, default_value = "0.0.0.0")]
    bind: String,
    port: u16,
    /// Echo received bytes back to the sender
    #[arg(long)]
    echo: bool,
    /// Stop after N seconds (0 = until Ctrl+C)
    #[arg(long, default_value_t = 0)]
    duration: u64,
    #[arg(long, value_name = "FILE")]
    log: Option<PathBuf>,
    #[arg(long, value_enum, default_value_t = FormatArg::Ascii)]
    format: FormatArg,
}

#[derive(Args)]
struct UdpArgs {
    #[arg(long, default_value = "0.0.0.0")]
    bind: String,
    /// Local port to bind (0 = ephemeral)
    #[arg(long, default_value_t = 0)]
    local_port: u16,
    /// Default destination as host:port
    #[arg(long, value_name = "HOST:PORT")]
    remote: Option<String>,
    #[arg(long)]
    echo: bool,
    #[arg(long)]
    broadcast: bool,
    #[arg(long, value_name = "TEXT")]
    send: Option<String>,
    #[arg(long)]
    hex: bool,
    #[arg(long, default_value = "none")]
    eol: String,
    #[arg(long, default_value_t = 1000)]
    read_ms: u64,
    #[arg(long, value_name = "FILE")]
    log: Option<PathBuf>,
    #[arg(long)]
    interactive: bool,
    #[arg(long, value_enum, default_value_t = FormatArg::Ascii)]
    format: FormatArg,
}

#[derive(Args)]
struct ScanArgs {
    /// Broadcast or unicast target
    #[arg(long, default_value = "255.255.255.255")]
    target: String,
    #[arg(long, default_value_t = 4000)]
    port: u16,
    /// Probe payload as hex
    #[arg(long, value_name = "HEX")]
    payload_hex: Option<String>,
    #[arg(long, default_value_t = 1000)]
    timeout_ms: u64,
    #[arg(long, default_value_t = 0)]
    local_port: u16,
}

#[derive(Args)]
struct TcpProbeArgs {
    host: String,
    port: u16,
    #[arg(long, default_value_t = 2000)]
    timeout_ms: u64,
    /// Do not wait for a banner after connecting
    #[arg(long)]
    no_banner: bool,
}

#[derive(Args)]
struct UdpListenArgs {
    port: u16,
    #[arg(long, default_value_t = 5000)]
    timeout_ms: u64,
}

#[derive(Args)]
struct ChecksumArgs {
    /// Data to hash (supports \r \n \xNN and $hh escapes)
    data: String,
    /// Interpret the data as hex
    #[arg(long)]
    hex: bool,
    /// Algorithm name, or "all" (default)
    #[arg(long, default_value = "all")]
    algorithm: String,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    init_tracing(cli.verbose);
    let config = Arc::new(ConfigStore::load(cli.config.as_deref()));
    let buffer = config.get().buffer_bytes;
    let manager = SessionManager::with_buffer(buffer);

    let command = cli.command.unwrap_or_else(|| {
        if has_display() {
            Command::Gui
        } else {
            Command::Tui {
                mcp: None,
                theme: None,
                colors: None,
                ascii: false,
            }
        }
    });
    match command {
        Command::Gui => run_gui(config),
        Command::Tui {
            mcp,
            theme,
            colors,
            ascii,
        } => run_tui(config, mcp, theme, colors, ascii),
        other => {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .context("failed to start async runtime")?;
            runtime.block_on(run_command(other, manager, config))
        }
    }
}

fn init_tracing(verbose: bool) {
    let level = if verbose { "debug" } else { "info" };
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(level));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}

async fn run_command(
    command: Command,
    manager: SessionManager,
    config: Arc<ConfigStore>,
) -> anyhow::Result<()> {
    match command {
        Command::Gui | Command::Tui { .. } => unreachable!(),
        Command::Mcp { transport, bind } => {
            let server = Arc::new(McpServer::new(manager, config));
            match transport {
                Transport::Stdio => serial_mcp_mcp::serve_stdio(server).await?,
                Transport::Http => {
                    let addr: SocketAddr = bind
                        .parse()
                        .with_context(|| format!("invalid bind address: {bind}"))?;
                    serial_mcp_mcp::serve_http(server, addr).await?;
                }
            }
        }
        Command::Ports => run_ports()?,
        Command::Serial(args) => run_serial(args, &manager).await?,
        Command::TcpClient(args) => run_tcp_client(args, &manager).await?,
        Command::TcpServer(args) => run_tcp_server(args, &manager).await?,
        Command::Udp(args) => run_udp(args, &manager).await?,
        Command::Scan(args) => run_scan(args).await?,
        Command::TcpProbe(args) => run_tcp_probe(args).await?,
        Command::UdpListen(args) => run_udp_listen(args).await?,
        Command::Checksum(args) => run_checksum(args)?,
        Command::Config => run_config(&config)?,
    }
    Ok(())
}

/// Whether a graphical session is available. Windows and macOS always have
/// one; on Linux and BSD we look for X11 or Wayland.
fn has_display() -> bool {
    if !cfg!(feature = "gui") {
        return false;
    }
    if cfg!(any(target_os = "windows", target_os = "macos")) {
        return true;
    }
    std::env::var_os("DISPLAY").is_some_and(|v| !v.is_empty())
        || std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty())
}

fn run_tui(
    config: Arc<ConfigStore>,
    mcp: Option<String>,
    theme: Option<String>,
    colors: Option<String>,
    ascii: bool,
) -> anyhow::Result<()> {
    #[cfg(feature = "tui")]
    {
        serial_mcp_tui::run_with(
            config,
            serial_mcp_tui::Options {
                mcp,
                theme,
                colors,
                ascii,
            },
        )
    }
    #[cfg(not(feature = "tui"))]
    {
        let _ = (config, mcp, theme, colors, ascii);
        bail!("this build was compiled without the TUI feature; rebuild with --features tui")
    }
}

fn run_gui(config: Arc<ConfigStore>) -> anyhow::Result<()> {
    #[cfg(feature = "gui")]
    {
        serial_mcp_gui::run(config).map_err(|err| anyhow::anyhow!("GUI error: {err}"))
    }
    #[cfg(not(feature = "gui"))]
    {
        let _ = config;
        bail!("this build was compiled without the GUI feature; rebuild with --features gui")
    }
}

fn run_ports() -> anyhow::Result<()> {
    let ports = serial_mcp_core::net::list_ports()?;
    if ports.is_empty() {
        println!("No serial ports found.");
        return Ok(());
    }
    println!("{:<24} {:<8} {:<10} DESCRIPTION", "PORT", "KIND", "VID:PID");
    for port in ports {
        let vid_pid = match (port.vid, port.pid) {
            (Some(vid), Some(pid)) => format!("{vid:04X}:{pid:04X}"),
            _ => "-".to_string(),
        };
        let description = port
            .product
            .clone()
            .or_else(|| port.manufacturer.clone())
            .or_else(|| port.serial_number.clone())
            .unwrap_or_default();
        println!(
            "{:<24} {:<8} {:<10} {}",
            port.port_name, port.kind, vid_pid, description
        );
    }
    Ok(())
}

async fn run_serial(args: SerialArgs, manager: &SessionManager) -> anyhow::Result<()> {
    let cfg = SerialConfig {
        port: args.port,
        baud: args.baud,
        data_bits: args.data_bits,
        parity: args.parity.into(),
        stop_bits: args.stop_bits,
        flow_control: args.flow.into(),
        dtr: args.dtr,
        rts: args.rts,
        read_timeout_ms: 10,
        label: None,
    };
    check(serial_mcp_core::validate::serial(&cfg, &[], &[]))?;
    check_payload(&args.send, args.hex, &args.eol)?;
    let info = manager.open_serial(cfg).await?;
    println!("session {}: {}", info.id, info.label);
    manager
        .wait_connected(&info.id, Duration::from_secs(3))
        .await?;
    if let Some(log) = args.log {
        manager.set_logging(&info.id, Some(log), false)?;
    }
    if args.interactive {
        repl(manager, &info.id, args.hex, args.eol, args.format.into()).await?;
    } else {
        send_once(manager, &info.id, args.send, args.hex, &args.eol).await?;
        drain_for(manager, &info.id, args.read_ms, args.format.into()).await?;
    }
    manager.close(&info.id)?;
    Ok(())
}

async fn run_tcp_client(args: TcpClientArgs, manager: &SessionManager) -> anyhow::Result<()> {
    let cfg = TcpClientConfig {
        host: args.host,
        port: args.port,
        nvt: args.nvt,
        reconnect: args.reconnect,
        connect_timeout_ms: 3000,
        label: None,
    };
    check(serial_mcp_core::validate::tcp_client(&cfg))?;
    check_payload(&args.send, args.hex, &args.eol)?;
    let info = manager.open_tcp_client(cfg).await?;
    println!("session {}: {}", info.id, info.label);
    // With auto-reconnect the worker keeps trying in the background; blocking
    // on the first attempt would defeat the option.
    if !args.reconnect {
        manager
            .wait_connected(&info.id, Duration::from_secs(6))
            .await?;
    }
    if let Some(log) = args.log {
        manager.set_logging(&info.id, Some(log), false)?;
    }
    if args.interactive {
        repl(manager, &info.id, args.hex, args.eol, args.format.into()).await?;
    } else {
        send_once(manager, &info.id, args.send, args.hex, &args.eol).await?;
        drain_for(manager, &info.id, args.read_ms, args.format.into()).await?;
    }
    manager.close(&info.id)?;
    Ok(())
}

async fn run_tcp_server(args: TcpServerArgs, manager: &SessionManager) -> anyhow::Result<()> {
    let cfg = TcpServerConfig {
        bind: args.bind,
        port: args.port,
        echo: args.echo,
        max_clients: 32,
        label: None,
    };
    check(serial_mcp_core::validate::tcp_server(&cfg, &[]))?;
    let info = manager.open_tcp_server(cfg).await?;
    println!("session {}: {}", info.id, info.label);
    if let Some(log) = args.log {
        manager.set_logging(&info.id, Some(log), false)?;
    }
    let deadline = if args.duration > 0 {
        Some(Instant::now() + Duration::from_secs(args.duration))
    } else {
        None
    };
    let mut since = 0u64;
    loop {
        if let Some(deadline) = deadline {
            if Instant::now() >= deadline {
                break;
            }
        }
        drain_once(manager, &info.id, &mut since, args.format.into())?;
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            _ = tokio::time::sleep(Duration::from_millis(50)) => {}
        }
    }
    manager.close(&info.id)?;
    Ok(())
}

async fn run_udp(args: UdpArgs, manager: &SessionManager) -> anyhow::Result<()> {
    let (remote_host, remote_port) = match args.remote {
        Some(remote) => {
            let (host, port) = split_host_port(&remote)?;
            let port: u16 = port.parse().context("invalid remote port")?;
            (Some(host), Some(port))
        }
        None => (None, None),
    };
    let cfg = UdpConfig {
        bind: args.bind,
        local_port: args.local_port,
        remote_host,
        remote_port,
        echo: args.echo,
        broadcast: args.broadcast,
        label: None,
    };
    check(serial_mcp_core::validate::udp(&cfg, &[]))?;
    check_payload(&args.send, args.hex, &args.eol)?;
    let info = manager.open_udp(cfg).await?;
    println!("session {}: {}", info.id, info.label);
    manager
        .wait_connected(&info.id, Duration::from_secs(2))
        .await?;
    if let Some(log) = args.log {
        manager.set_logging(&info.id, Some(log), false)?;
    }
    if args.interactive {
        repl(manager, &info.id, args.hex, args.eol, args.format.into()).await?;
    } else {
        send_once(manager, &info.id, args.send, args.hex, &args.eol).await?;
        drain_for(manager, &info.id, args.read_ms, args.format.into()).await?;
    }
    manager.close(&info.id)?;
    Ok(())
}

async fn run_scan(args: ScanArgs) -> anyhow::Result<()> {
    let payload = match args.payload_hex {
        Some(hex) => codec::parse_hex(&hex)?,
        None => Vec::new(),
    };
    let request = serial_mcp_core::discovery::ScanRequest {
        target: args.target,
        port: args.port,
        payload,
        timeout_ms: args.timeout_ms,
        local_port: args.local_port,
    };
    let responses = serial_mcp_core::discovery::udp_scan(request).await?;
    if responses.is_empty() {
        println!("No responses.");
        return Ok(());
    }
    println!("{:<24} {:>6} {:>7}  PAYLOAD", "ADDRESS", "BYTES", "RTT");
    for response in responses {
        println!(
            "{:<24} {:>6} {:>6}ms  {}",
            response.addr, response.bytes, response.rtt_ms, response.ascii
        );
    }
    Ok(())
}

async fn run_tcp_probe(args: TcpProbeArgs) -> anyhow::Result<()> {
    let result = serial_mcp_core::net::tcp_probe(
        &args.host,
        args.port,
        Duration::from_millis(args.timeout_ms.clamp(1, 600_000)),
        !args.no_banner,
    )
    .await?;
    println!(
        "{}:{} connected in {} ms",
        result.host, result.port, result.rtt_ms
    );
    if let Some(banner) = &result.banner {
        if !banner.is_empty() {
            println!("banner: {}", banner.trim_end_matches('\n'));
        }
    }
    if let Some(hex) = &result.banner_hex {
        if !hex.is_empty() {
            println!("hex:    {hex}");
        }
    }
    Ok(())
}

async fn run_udp_listen(args: UdpListenArgs) -> anyhow::Result<()> {
    let responses = serial_mcp_core::discovery::udp_listen(
        args.port,
        Duration::from_millis(args.timeout_ms.clamp(1, 600_000)),
    )
    .await?;
    if responses.is_empty() {
        println!("No datagrams.");
        return Ok(());
    }
    println!("{:<24} {:>6} {:>7}  PAYLOAD", "ADDRESS", "BYTES", "RTT");
    for response in responses {
        println!(
            "{:<24} {:>6} {:>6}ms  {}",
            response.addr, response.bytes, response.rtt_ms, response.ascii
        );
    }
    Ok(())
}

fn run_checksum(args: ChecksumArgs) -> anyhow::Result<()> {
    let bytes = codec::parse_input(&args.data, args.hex)?;
    let kinds = if args.algorithm.eq_ignore_ascii_case("all") {
        ChecksumKind::ALL[1..].to_vec()
    } else {
        vec![ChecksumKind::parse(&args.algorithm)?]
    };
    println!("input: {} bytes  {}", bytes.len(), codec::to_hex(&bytes));
    println!("{:<16} {:<12} FRAME", "ALGORITHM", "CHECKSUM");
    for kind in kinds {
        let sum = serial_mcp_core::checksum::compute(kind, &bytes);
        let frame = serial_mcp_core::checksum::append(kind, &bytes);
        println!(
            "{:<16} {:<12} {}",
            kind.label(),
            codec::to_hex(&sum),
            codec::to_hex(&frame)
        );
    }
    Ok(())
}

fn run_config(config: &ConfigStore) -> anyhow::Result<()> {
    println!("config file: {}", config.path().display());
    println!();
    println!("{}", toml::to_string_pretty(&config.get())?);
    let exe = std::env::current_exe()?;
    let snippet = serde_json::json!({
        "mcpServers": {
            "serial-mcp": {
                "command": exe.display().to_string(),
                "args": ["mcp", "--transport", "stdio"]
            }
        }
    });
    println!("MCP client configuration:");
    println!("{}", serde_json::to_string_pretty(&snippet)?);
    Ok(())
}

/// Fail early, before opening anything, when settings or data are invalid.
fn check(report: serial_mcp_core::validate::Report) -> anyhow::Result<()> {
    let errors: Vec<String> = report
        .issues
        .iter()
        .filter(|i| i.severity == serial_mcp_core::validate::Severity::Error)
        .map(|i| format!("{}: {}", i.field, i.message))
        .collect();
    if errors.is_empty() {
        Ok(())
    } else {
        bail!("invalid settings:\n  {}", errors.join("\n  "))
    }
}

fn check_payload(send: &Option<String>, hex: bool, eol: &str) -> anyhow::Result<()> {
    if let Some(text) = send {
        codec::parse_input(text, hex).with_context(|| format!("cannot parse --send {text:?}"))?;
    }
    let mut probe = Vec::new();
    append_eol(&mut probe, eol)
}

async fn send_once(
    manager: &SessionManager,
    id: &str,
    send: Option<String>,
    hex: bool,
    eol: &str,
) -> anyhow::Result<()> {
    if let Some(text) = send {
        let mut bytes = codec::parse_input(&text, hex)?;
        append_eol(&mut bytes, eol)?;
        manager.write(id, &bytes)?;
    }
    Ok(())
}

fn append_eol(bytes: &mut Vec<u8>, eol: &str) -> anyhow::Result<()> {
    match eol.to_ascii_lowercase().as_str() {
        "" | "none" => {}
        "cr" => bytes.push(b'\r'),
        "lf" => bytes.push(b'\n'),
        "crlf" => bytes.extend_from_slice(b"\r\n"),
        other => bail!("invalid --eol value '{other}' (expected none, cr, lf or crlf)"),
    }
    Ok(())
}

/// Split `host:port`, accepting `[::1]:4000` for IPv6 literals.
fn split_host_port(value: &str) -> anyhow::Result<(String, String)> {
    let value = value.trim();
    if let Some(rest) = value.strip_prefix('[') {
        let (host, port) = rest
            .split_once("]:")
            .with_context(|| format!("expected [host]:port, got {value}"))?;
        if host.is_empty() || port.is_empty() {
            bail!("expected [host]:port, got {value}");
        }
        return Ok((host.to_string(), port.to_string()));
    }
    let (host, port) = value
        .rsplit_once(':')
        .with_context(|| format!("expected host:port, got {value}"))?;
    if host.is_empty() || port.is_empty() {
        bail!("expected host:port, got {value}");
    }
    Ok((host.to_string(), port.to_string()))
}

async fn drain_for(
    manager: &SessionManager,
    id: &str,
    read_ms: u64,
    format: ViewFormat,
) -> anyhow::Result<()> {
    let deadline = Instant::now() + Duration::from_millis(read_ms);
    let mut since = 0u64;
    loop {
        drain_once(manager, id, &mut since, format)?;
        if Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    Ok(())
}

fn drain_once(
    manager: &SessionManager,
    id: &str,
    since: &mut u64,
    format: ViewFormat,
) -> anyhow::Result<()> {
    let batch = manager.read_since(id, *since, 256 * 1024)?;
    *since = batch.next_seq;
    let mut wrote = false;
    for chunk in batch.chunks {
        match chunk.dir {
            Direction::Rx => {
                print!("{}", codec::format_bytes(&chunk.data, format));
                wrote = true;
            }
            Direction::Info => eprintln!("[info] {}", codec::escape_bytes(&chunk.data)),
            Direction::Error => eprintln!("[error] {}", codec::escape_bytes(&chunk.data)),
            Direction::Nvt => eprintln!("[nvt] {}", codec::escape_bytes(&chunk.data)),
            Direction::Tx => {}
        }
    }
    if wrote {
        std::io::stdout().flush()?;
    }
    Ok(())
}

async fn repl(
    manager: &SessionManager,
    id: &str,
    mut hex_input: bool,
    mut eol: String,
    mut format: ViewFormat,
) -> anyhow::Result<()> {
    eprintln!("Interactive terminal. /help for commands, /quit to exit.");
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut since = 0u64;
    loop {
        tokio::select! {
            line = lines.next_line() => {
                let Some(line) = line? else { break };
                let line = line.trim_end_matches(['\r', '\n']).to_string();
                if let Some(command) = line.strip_prefix('/') {
                    let mut parts = command.split_whitespace();
                    match parts.next().unwrap_or("") {
                        "quit" | "exit" | "q" => break,
                        "help" | "h" => {
                            eprintln!("commands: /hex on|off  /format ascii|hex|dec  /eol none|cr|lf|crlf");
                            eprintln!("          /rts on|off  /dtr on|off  /break on|off  /log PATH  /quit");
                        }
                        "hex" => hex_input = matches!(parts.next(), Some("on" | "1" | "true")),
                        "format" => {
                            if let Some(value) = parts.next() {
                                match ViewFormat::parse(value) {
                                    Ok(next) => format = next,
                                    Err(err) => eprintln!("[error] {err}"),
                                }
                            }
                        }
                        "eol" => {
                            if let Some(value) = parts.next() {
                                let mut probe = Vec::new();
                                match append_eol(&mut probe, value) {
                                    Ok(()) => eol = value.to_string(),
                                    Err(err) => eprintln!("[error] {err}"),
                                }
                            }
                        }
                        "rts" | "dtr" | "break" => {
                            let on = matches!(parts.next(), Some("on" | "1" | "true"));
                            let op = match command.split_whitespace().next().unwrap_or("") {
                                "rts" => serial_mcp_core::ControlOp::Rts(on),
                                "dtr" => serial_mcp_core::ControlOp::Dtr(on),
                                _ => serial_mcp_core::ControlOp::Break(on),
                            };
                            if let Err(err) = manager.set_control(id, op) {
                                eprintln!("[error] {err}");
                            }
                        }
                        "log" => {
                            if let Some(path) = parts.next() {
                                match manager.set_logging(id, Some(PathBuf::from(path)), false) {
                                    Ok(()) => eprintln!("logging to {path}"),
                                    Err(err) => eprintln!("[error] {err}"),
                                }
                            }
                        }
                        other => eprintln!("unknown command: /{other}"),
                    }
                    continue;
                }
                match codec::parse_input(&line, hex_input) {
                    Ok(mut bytes) => {
                        if let Err(err) = append_eol(&mut bytes, &eol) {
                            eprintln!("[error] {err}");
                        } else if let Err(err) = manager.write(id, &bytes) {
                            eprintln!("[error] {err}");
                        }
                    }
                    Err(err) => eprintln!("[error] {err}"),
                }
            }
            _ = tokio::signal::ctrl_c() => break,
            _ = tokio::time::sleep(Duration::from_millis(30)) => {}
        }
        if let Err(err) = drain_once(manager, id, &mut since, format) {
            eprintln!("[error] {err}");
            break;
        }
    }
    Ok(())
}
