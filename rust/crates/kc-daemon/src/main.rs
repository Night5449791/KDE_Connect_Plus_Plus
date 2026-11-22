//! Headless KDE Connect daemon and command-line client.
//!
//! `serve` owns the link provider, the [`DeviceManager`] and three pump tasks:
//! transport `LinkEvent`s go into the manager, plugin `OutgoingPacket`s come
//! back out and are handed to `manager.send_packet`, and `DeviceEvent`s are
//! printed on stdout. Other subcommands share the same `FileStore` but are
//! separate processes, so they cannot reach a running daemon's live links
//! (see [`IPC_HINT`]).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use clap::{Parser, Subcommand};
use kc_core::{
    BatteryStatus, CoreError, DeviceEvent, DeviceManager, FileStore, LinkEvent, LinkProvider,
    OutgoingPacket, Platform, Result, Store,
};
use kc_protocol::identity::generate_device_id;
use kc_protocol::{generate_identity, DeviceInfo, DeviceType, NetworkPacket};
use tokio::sync::{broadcast, mpsc};
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

const IPC_HINT: &str = "hint: this process has no IPC with a running daemon and no live link;\n\
       pairing and packet-sending commands need an active link in this process";

// ---------------------------------------------------------------------------
// CLI definition
// ---------------------------------------------------------------------------

#[derive(Debug, Parser)]
#[command(
    name = "kc-daemon",
    version,
    about = "Headless KDE Connect daemon and CLI"
)]
struct Cli {
    /// JSON file holding our identity and the trusted-device list
    #[arg(long, global = true)]
    store: Option<PathBuf>,

    /// Advertised device name (falls back to KC_DEVICE_NAME / KDE_CONNECT_DEVICE_NAME)
    #[arg(long, global = true)]
    device_name: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// List remembered devices (id, name, paired, reachable, plugins) and exit
    Devices,
    /// Run the discovery and packet pumps forever (Ctrl-C to stop)
    Serve {
        /// Do not send UDP discovery broadcasts; only listen for peers
        #[arg(long)]
        no_broadcast: bool,
    },
    /// Request pairing with a remembered device
    Pair { device_id: String },
    /// Accept a pairing request from a remembered device
    Accept { device_id: String },
    /// Remove a device from the trusted store
    Unpair { device_id: String },
    /// Send a `kdeconnect.ping` packet
    Ping {
        device_id: String,
        /// Optional message body
        message: Option<String>,
    },
    /// Clipboard operations
    Clipboard {
        #[command(subcommand)]
        action: ClipboardCommand,
    },
    /// Ring a paired phone via `kdeconnect.findmyphone.request`
    Find { device_id: String },
}

#[derive(Debug, Subcommand)]
enum ClipboardCommand {
    /// Set the local clipboard and share it with one or all remembered devices
    Set {
        /// Text to send
        text: String,
        /// Target device id; when omitted the packet is sent to every device
        #[arg(long, short = 'd')]
        device: Option<String>,
    },
}

// ---------------------------------------------------------------------------
// Host callbacks
// ---------------------------------------------------------------------------

/// Terminal implementation of [`Platform`]: every callback is printed to
/// stdout so a headless user can see what plugins are doing.
struct CliPlatform {
    device_name: String,
    clipboard: Mutex<Option<String>>,
}

impl CliPlatform {
    fn new(device_name: String) -> Self {
        Self {
            device_name,
            clipboard: Mutex::new(None),
        }
    }
}

impl Platform for CliPlatform {
    fn device_name(&self) -> String {
        self.device_name.clone()
    }

    fn device_type(&self) -> DeviceType {
        DeviceType::Desktop
    }

    fn clipboard_get(&self) -> Option<String> {
        self.clipboard.lock().ok().and_then(|slot| slot.clone())
    }

    fn clipboard_set(&self, text: &str) -> bool {
        println!("[clipboard] local clipboard set: {text}");
        if let Ok(mut slot) = self.clipboard.lock() {
            *slot = Some(text.to_owned());
        }
        true
    }

    fn on_ping(&self, device_id: &str, message: Option<&str>) {
        match message {
            Some(message) => println!("[ping] from {device_id}: {message}"),
            None => println!("[ping] from {device_id}"),
        }
    }

    fn on_battery(&self, device_id: &str, status: &BatteryStatus) {
        println!(
            "[battery] {device_id}: {}% charging={} threshold={}",
            status.current_charge, status.is_charging, status.threshold_event
        );
    }

    fn on_connectivity(&self, device_id: &str, packet: &NetworkPacket) {
        println!("[connectivity] {device_id}: {}", packet.ty);
    }

    fn on_clipboard(&self, device_id: &str, content: &str) {
        println!("[clipboard] from {device_id}: {content}");
        self.clipboard_set(content);
    }

    fn on_notification(&self, device_id: &str, packet: &NetworkPacket) {
        println!("[notification] {device_id}: {}", packet.ty);
    }

    fn on_mpris(&self, device_id: &str, packet: &NetworkPacket) {
        println!("[mpris] {device_id}: {}", packet.ty);
    }

    fn on_system_volume(&self, device_id: &str, packet: &NetworkPacket) {
        println!("[systemvolume] {device_id}: {}", packet.ty);
    }

    fn on_telephony(&self, device_id: &str, packet: &NetworkPacket) {
        println!("[telephony] {device_id}: {}", packet.ty);
    }

    fn on_sms(&self, device_id: &str, packet: &NetworkPacket) {
        println!("[sms] {device_id}: {}", packet.ty);
    }

    fn on_share(&self, device_id: &str, packet: &NetworkPacket) {
        println!("[share] {device_id}: {}", packet.ty);
    }

    fn on_run_command(&self, device_id: &str, packet: &NetworkPacket) {
        println!("[runcommand] {device_id}: {}", packet.ty);
    }

    fn on_find_my_phone(&self, device_id: &str) {
        println!("[findmyphone] {device_id} asked us to ring");
    }

    fn on_file_transfer(&self, device_id: &str, packet: &NetworkPacket) {
        println!("[filetransfer] {device_id}: {}", packet.ty);
    }

    fn on_pairing_request(&self, device_id: &str, verification_key: &str) {
        println!("[pairing] request from {device_id}; verification key: {verification_key}");
    }
}

// ---------------------------------------------------------------------------
// Shared setup
// ---------------------------------------------------------------------------

struct Context {
    store: Arc<FileStore>,
    manager: Arc<DeviceManager>,
    platform: Arc<CliPlatform>,
}

fn default_store_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("kc-daemon").join("store.json")
}

fn default_device_name() -> String {
    std::env::var("KC_DEVICE_NAME")
        .or_else(|_| std::env::var("KDE_CONNECT_DEVICE_NAME"))
        .unwrap_or_else(|_| "KDE Connect Rust".to_owned())
}

async fn build_context(cli: &Cli) -> Result<Context> {
    let store = Arc::new(FileStore::open(
        cli.store.clone().unwrap_or_else(default_store_path),
    )?);

    let identity = match store.load_identity()? {
        Some(identity) => identity,
        None => {
            let device_id = generate_device_id();
            let identity = generate_identity(&device_id)?;
            store.save_identity(&identity)?;
            info!(%device_id, "generated and persisted a new device identity");
            identity
        }
    };

    let platform = Arc::new(CliPlatform::new(
        cli.device_name.clone().unwrap_or_else(default_device_name),
    ));
    let registry = kc_plugins::default_registry();
    let manager = Arc::new(DeviceManager::new(
        identity,
        store.clone(),
        platform.clone(),
        registry,
    ));
    manager.load_remembered_devices().await?;

    Ok(Context {
        store,
        manager,
        platform,
    })
}

fn yes_no(value: bool) -> &'static str {
    if value {
        "yes"
    } else {
        "no"
    }
}

fn print_device_event(event: &DeviceEvent) {
    match event {
        DeviceEvent::Discovered {
            device_id,
            name,
            paired,
        } => println!(
            "[event] discovered {device_id} ({name}) paired={}",
            yes_no(*paired)
        ),
        DeviceEvent::PairingRequested {
            device_id,
            verification_key,
        } => println!("[event] pairing request from {device_id}: key={verification_key}"),
        DeviceEvent::Paired { device_id } => println!("[event] paired with {device_id}"),
        DeviceEvent::Unpaired { device_id } => println!("[event] unpaired from {device_id}"),
        DeviceEvent::Connected { device_id } => println!("[event] connected {device_id}"),
        DeviceEvent::Disconnected { device_id } => println!("[event] disconnected {device_id}"),
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

async fn cmd_devices(ctx: &Context) -> Result<()> {
    let mut snapshots = Vec::new();
    for device in ctx.manager.devices().await {
        snapshots.push(device.snapshot().await);
    }
    snapshots.sort_by(|a, b| a.device_id.cmp(&b.device_id));

    println!(
        "{:<34} {:<22} {:<7} {:<10} PLUGINS",
        "DEVICE ID", "NAME", "PAIRED", "REACHABLE"
    );
    if snapshots.is_empty() {
        println!("(no remembered devices; run `kc-daemon serve` to discover peers)");
        return Ok(());
    }
    for snapshot in snapshots {
        println!(
            "{:<34} {:<22} {:<7} {:<10} {}",
            snapshot.device_id,
            snapshot.name,
            yes_no(snapshot.paired),
            yes_no(snapshot.reachable),
            snapshot.loaded_plugins.join(", ")
        );
    }
    Ok(())
}

async fn cmd_serve(ctx: &Context, no_broadcast: bool) -> Result<()> {
    let my_id = ctx.manager.identity().device_id.clone();
    let mut info = DeviceInfo::new(
        my_id.clone(),
        ctx.platform.device_name(),
        ctx.platform.device_type(),
    );
    let (incoming, outgoing) = ctx.manager.registry().capabilities();
    info.incoming_capabilities = incoming;
    info.outgoing_capabilities = outgoing;

    let provider = Arc::new(kc_transport::LanLinkProvider::new(
        (*ctx.manager.identity()).clone(),
        info,
        ctx.store.clone(),
        !no_broadcast,
    )?);

    // Subscribe and take the outgoing receiver before starting the provider,
    // so no event or plugin packet can be missed during startup.
    let mut events_rx = ctx.manager.subscribe();
    let outgoing_rx = ctx.manager.take_outgoing_receiver().await;

    let (link_tx, mut link_rx) = mpsc::unbounded_channel::<LinkEvent>();
    let provider_for_start = provider.clone();
    let mut provider_handle = tokio::spawn(async move { provider_for_start.start(link_tx).await });

    let manager_for_links = ctx.manager.clone();
    let link_task = tokio::spawn(async move {
        while let Some(event) = link_rx.recv().await {
            if let Err(error) = manager_for_links.handle_link_event(event).await {
                warn!(%error, "failed to handle link event");
            }
        }
    });

    let event_task = tokio::spawn(async move {
        loop {
            match events_rx.recv().await {
                Ok(event) => print_device_event(&event),
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    warn!(skipped, "device event receiver lagged");
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    let manager_for_sends = ctx.manager.clone();
    let outgoing_task = tokio::spawn(async move {
        let Some(mut receiver) = outgoing_rx else {
            return;
        };
        while let Some(OutgoingPacket { device_id, packet }) = receiver.recv().await {
            if let Err(error) = manager_for_sends.send_packet(&device_id, packet).await {
                warn!(%device_id, %error, "failed to send outgoing packet");
            }
        }
    });

    info!(device_id = %my_id, broadcast = !no_broadcast, "kc-daemon is serving");

    tokio::select! {
        result = tokio::signal::ctrl_c() => {
            match result {
                Ok(()) => info!("Ctrl-C received, shutting down"),
                Err(error) => warn!(%error, "Ctrl-C handler failed"),
            }
        }
        result = &mut provider_handle => {
            match result {
                Ok(Ok(())) => info!("link provider stopped"),
                Ok(Err(error)) => error!(%error, "link provider failed"),
                Err(error) => error!(%error, "link provider task panicked"),
            }
        }
    }

    provider.stop().await;
    provider_handle.abort();
    link_task.abort();
    event_task.abort();
    outgoing_task.abort();
    Ok(())
}

async fn cmd_pair(ctx: &Context, device_id: &str) -> Result<()> {
    ctx.manager
        .request_pairing(device_id)
        .await
        .map_err(|error| {
            eprintln!("{error}\n{IPC_HINT}");
            error
        })?;
    println!("pairing requested from {device_id}");
    Ok(())
}

async fn cmd_accept(ctx: &Context, device_id: &str) -> Result<()> {
    ctx.manager
        .accept_pairing(device_id)
        .await
        .map_err(|error| {
            eprintln!("{error}\n{IPC_HINT}");
            error
        })?;
    println!("accepted pairing with {device_id}");
    Ok(())
}

async fn cmd_unpair(ctx: &Context, device_id: &str) -> Result<()> {
    let trusted = ctx.store.is_trusted(device_id);
    if !trusted && ctx.manager.get(device_id).await.is_none() {
        return Err(CoreError::DeviceNotFound(device_id.to_owned()));
    }

    if ctx.manager.get(device_id).await.is_some() {
        if let Err(error) = ctx.manager.unpair(device_id).await {
            warn!(%device_id, %error, "manager unpair failed; removing from store directly");
        }
    }
    // Works even when the device has no live link in this process.
    ctx.store.remove_trusted_device(device_id)?;
    println!("{device_id} unpaired and removed from the trusted store");
    Ok(())
}

async fn cmd_ping(ctx: &Context, device_id: &str, message: Option<&str>) -> Result<()> {
    let packet = kc_plugins::ping::ping_packet(message);
    ctx.manager
        .send_packet(device_id, packet)
        .await
        .map_err(|error| {
            eprintln!("{error}\n{IPC_HINT}");
            error
        })?;
    println!("ping sent to {device_id}");
    Ok(())
}

async fn cmd_clipboard_set(ctx: &Context, text: &str, device: Option<&str>) -> Result<()> {
    ctx.platform.clipboard_set(text);
    let packet = kc_plugins::clipboard::clipboard_packet(text);

    match device {
        Some(device_id) => {
            ctx.manager
                .send_packet(device_id, packet)
                .await
                .map_err(|error| {
                    eprintln!("{error}\n{IPC_HINT}");
                    error
                })?;
            println!("clipboard sent to {device_id}");
        }
        None => {
            let devices = ctx.manager.devices().await;
            let total = devices.len();
            if total == 0 {
                println!("(local clipboard set; no remembered devices to send to)");
                return Ok(());
            }
            let mut delivered = 0;
            for device in devices {
                let device_id = device.device_id();
                match ctx.manager.send_packet(&device_id, packet.clone()).await {
                    Ok(()) => {
                        delivered += 1;
                        println!("clipboard sent to {device_id}");
                    }
                    Err(error) => warn!(%device_id, %error, "clipboard send failed"),
                }
            }
            println!("clipboard delivered to {delivered}/{total} remembered device(s)");
        }
    }
    Ok(())
}

async fn cmd_find(ctx: &Context, device_id: &str) -> Result<()> {
    let packet = NetworkPacket::new("kdeconnect.findmyphone.request");
    ctx.manager
        .send_packet(device_id, packet)
        .await
        .map_err(|error| {
            eprintln!("{error}\n{IPC_HINT}");
            error
        })?;
    println!("find-my-phone request sent to {device_id}");
    Ok(())
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<()> {
    // Some transitive crates enable rustls' aws-lc-rs default feature while
    // kc-protocol uses ring. Pin the process-wide provider explicitly so TLS
    // setup in the transport cannot panic with an ambiguous configuration.
    let _ = rustls::crypto::ring::default_provider().install_default();

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();
    let ctx = build_context(&cli).await?;

    match &cli.command {
        Command::Devices => cmd_devices(&ctx).await,
        Command::Serve { no_broadcast } => cmd_serve(&ctx, *no_broadcast).await,
        Command::Pair { device_id } => cmd_pair(&ctx, device_id).await,
        Command::Accept { device_id } => cmd_accept(&ctx, device_id).await,
        Command::Unpair { device_id } => cmd_unpair(&ctx, device_id).await,
        Command::Ping { device_id, message } => cmd_ping(&ctx, device_id, message.as_deref()).await,
        Command::Clipboard { action } => match action {
            ClipboardCommand::Set { text, device } => {
                cmd_clipboard_set(&ctx, text, device.as_deref()).await
            }
        },
        Command::Find { device_id } => cmd_find(&ctx, device_id).await,
    }
}
