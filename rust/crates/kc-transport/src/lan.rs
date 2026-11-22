//! LAN transport: UDP identity broadcast, TCP listener, mutual TLS 1.2 links
//! and payload transfer. Faithful to upstream `LanLinkProvider` / `LanLink`.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicU16, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use kc_core::{Link, LinkEvent, LinkProvider, Result, Store};
use kc_protocol::identity::is_valid_identity_packet;
use kc_protocol::packet::MAX_IDENTITY_SIZE;
use kc_protocol::{DeviceInfo, Identity, NetworkPacket, ProtocolError, TlsContext};
use rustls_pki_types::ServerName;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio_rustls::{TlsAcceptor, TlsConnector};
use tracing::{debug, warn};

use crate::framing::{decode_packet_line, read_line_bounded};
use crate::identity::{
    check_targets, device_info_from_identity, identity_with_tcp_port, pinned_cert_der,
    targeted_identity_packet,
};
use crate::link::{LanLink, LanTlsStream};
use crate::mdns;
use crate::net::{bind_tcp_listener, bind_udp_reuse, TCP_PORT_MAX, TCP_PORT_MIN, UDP_PORT};

const RATE_LIMIT_WINDOW: Duration = Duration::from_secs(1);
const MAX_RATE_LIMIT_ENTRIES: usize = 255;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const BROADCAST_INTERVAL: Duration = Duration::from_secs(5);

/// UDP discovery on port 1716 plus TLS TCP links.
pub struct LanLinkProvider {
    info: DeviceInfo,
    store: Arc<dyn Store>,
    broadcast: bool,
    tls: TlsContext,
    state: Arc<State>,
}

struct State {
    stop_tx: watch::Sender<bool>,
    our_device_id: String,
    tasks: Mutex<Vec<JoinHandle<()>>>,
    links: Mutex<HashMap<u64, Arc<LanLink>>>,
    rate_by_ip: Mutex<HashMap<IpAddr, Instant>>,
    next_link_id: AtomicU64,
    tcp_port: AtomicU16,
}

impl State {
    fn new(our_device_id: String) -> Self {
        let (stop_tx, _) = watch::channel(false);
        Self {
            stop_tx,
            our_device_id,
            tasks: Mutex::new(Vec::new()),
            links: Mutex::new(HashMap::new()),
            rate_by_ip: Mutex::new(HashMap::new()),
            next_link_id: AtomicU64::new(1),
            tcp_port: AtomicU16::new(0),
        }
    }

    /// Upstream `rateLimitByIp`: at most one connection per IP per second.
    fn rate_limit(&self, ip: IpAddr) -> bool {
        let now = Instant::now();
        let mut map = self.rate_by_ip.lock().unwrap();
        if let Some(last) = map.get(&ip) {
            if now.duration_since(*last) < RATE_LIMIT_WINDOW {
                return true;
            }
        }
        map.insert(ip, now);
        if map.len() > MAX_RATE_LIMIT_ENTRIES {
            map.retain(|_, last| now.duration_since(*last) >= RATE_LIMIT_WINDOW);
        }
        false
    }
}

/// Immutable data shared by every spawned handshake / listener task.
pub(crate) struct HandshakeCtx {
    our_info: DeviceInfo,
    our_identity_wire: String,
    state: Arc<State>,
    tls: TlsContext,
    store: Arc<dyn Store>,
    sink: mpsc::UnboundedSender<LinkEvent>,
    pub(crate) stop_rx: watch::Receiver<bool>,
}

impl HandshakeCtx {
    /// Answer an mDNS discovery by sending our UDP identity to the peer.
    pub(crate) async fn send_identity_to(&self, addr: IpAddr) {
        let Ok(socket) = UdpSocket::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0))).await else {
            return;
        };
        let _ = socket.set_broadcast(true);
        if let Err(e) = socket
            .send_to(self.our_identity_wire.as_bytes(), (addr, UDP_PORT))
            .await
        {
            debug!(%e, %addr, "mDNS identity reply failed");
        }
    }
}

impl LanLinkProvider {
    /// Create a provider. Failures here are limited to TLS configuration
    /// problems (invalid identity material).
    pub fn new(
        identity: Identity,
        info: DeviceInfo,
        store: Arc<dyn Store>,
        broadcast: bool,
    ) -> Result<Self> {
        let tls = TlsContext::new(identity.clone());
        // Fail early if the identity's key/certificate cannot be used for TLS.
        let _ = tls.client_config_untrusted()?;
        let _ = tls.server_config_untrusted()?;
        Ok(Self {
            info,
            store,
            broadcast,
            tls,
            state: Arc::new(State::new(identity.device_id)),
        })
    }

    /// TCP port the local listener ended up on (0 before `start`).
    pub fn tcp_port(&self) -> u16 {
        self.state.tcp_port.load(Ordering::SeqCst)
    }

    async fn stop_tasks(&self) {
        for handle in self.state.tasks.lock().unwrap().drain(..) {
            handle.abort();
        }
        let links: Vec<_> = self.state.links.lock().unwrap().values().cloned().collect();
        for link in links {
            link.close().await;
        }
        self.state.links.lock().unwrap().clear();
    }
}

#[async_trait]
impl LinkProvider for LanLinkProvider {
    fn name(&self) -> &'static str {
        "LanLinkProvider"
    }

    async fn start(&self, sink: mpsc::UnboundedSender<LinkEvent>) -> Result<()> {
        if *self.state.stop_tx.borrow() {
            return Ok(());
        }

        // Receiving broadcasts is best-effort (port 1716 may be occupied).
        let udp = match bind_udp_reuse(SocketAddr::from((Ipv4Addr::UNSPECIFIED, UDP_PORT))) {
            Ok(socket) => Some(Arc::new(socket)),
            Err(e) => {
                warn!(%e, "cannot bind UDP 1716; continuing without broadcast reception");
                None
            }
        };

        // The TCP listener picks the first free port in 1716..=1764.
        let (tcp, tcp_port) = bind_tcp_listener(TCP_PORT_MIN..=TCP_PORT_MAX).await?;
        self.state.tcp_port.store(tcp_port, Ordering::SeqCst);
        debug!(tcp_port, "LAN TCP listener ready");

        let mut our_info = self.info.clone();
        our_info.tcp_port = Some(tcp_port);
        let our_identity_wire = identity_with_tcp_port(&our_info, tcp_port).serialize();

        let stop_rx = self.state.stop_tx.subscribe();
        let ctx = Arc::new(HandshakeCtx {
            our_info,
            our_identity_wire,
            state: self.state.clone(),
            tls: self.tls.clone(),
            store: self.store.clone(),
            sink,
            stop_rx,
        });

        let mut tasks = Vec::new();

        if let Some(udp) = udp {
            let ctx = ctx.clone();
            tasks.push(tokio::spawn(async move {
                udp_recv_loop(udp, ctx).await;
            }));
        }

        let tcp = Arc::new(tcp);
        let ctx2 = ctx.clone();
        tasks.push(tokio::spawn(async move {
            tcp_accept_loop(tcp, ctx2).await;
        }));

        if self.broadcast {
            let ctx3 = ctx.clone();
            tasks.push(tokio::spawn(async move {
                broadcast_loop(ctx3).await;
            }));
        }

        // mDNS is optional and never fatal.
        let ctx4 = ctx;
        tasks.push(tokio::spawn(async move {
            mdns::discovery_loop(ctx4).await;
        }));

        *self.state.tasks.lock().unwrap() = tasks;

        // `start` returns only once `stop` signals us.
        let mut stop_wait = self.state.stop_tx.subscribe();
        while !*stop_wait.borrow() {
            if stop_wait.changed().await.is_err() {
                break;
            }
        }

        self.stop_tasks().await;
        Ok(())
    }

    async fn stop(&self) {
        let _ = self.state.stop_tx.send(true);
        self.stop_tasks().await;
    }
}

async fn udp_recv_loop(udp: Arc<UdpSocket>, ctx: Arc<HandshakeCtx>) {
    let mut stop = ctx.stop_rx.clone();
    loop {
        let mut buf = vec![0u8; MAX_IDENTITY_SIZE];
        tokio::select! {
            _ = stop.changed() => break,
            received = udp.recv_from(&mut buf) => {
                let (len, source) = match received {
                    Ok(v) => v,
                    Err(e) => {
                        debug!(%e, "UDP receive failed");
                        continue;
                    }
                };
                let data = buf[..len].to_vec();
                let ctx = ctx.clone();
                tokio::spawn(async move {
                    handle_udp_packet(data, source, ctx).await;
                });
            }
        }
    }
}

async fn tcp_accept_loop(listener: Arc<tokio::net::TcpListener>, ctx: Arc<HandshakeCtx>) {
    let mut stop = ctx.stop_rx.clone();
    loop {
        tokio::select! {
            _ = stop.changed() => break,
            accepted = listener.accept() => {
                let (stream, peer) = match accepted {
                    Ok(v) => v,
                    Err(e) => {
                        debug!(%e, "TCP accept failed");
                        continue;
                    }
                };
                let ctx = ctx.clone();
                tokio::spawn(async move {
                    handle_incoming_tcp(stream, peer, ctx).await;
                });
            }
        }
    }
}

async fn broadcast_loop(ctx: Arc<HandshakeCtx>) {
    let Ok(socket) = UdpSocket::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0))).await else {
        warn!("cannot create UDP broadcast socket");
        return;
    };
    if let Err(e) = socket.set_broadcast(true) {
        warn!(%e, "cannot enable UDP broadcast");
    }
    let target = SocketAddr::from(([255, 255, 255, 255], UDP_PORT));
    let mut interval = tokio::time::interval(BROADCAST_INTERVAL);
    let mut stop = ctx.stop_rx.clone();
    loop {
        tokio::select! {
            _ = stop.changed() => break,
            _ = interval.tick() => {
                if let Err(e) = socket.send_to(ctx.our_identity_wire.as_bytes(), target).await {
                    debug!(%e, "UDP identity broadcast failed");
                }
            }
        }
    }
}

/// We received their UDP broadcast and connect to their TCP port. Upstream
/// marks this `ConnectionStarted.Remotely`, which means `clientMode = false`:
/// we are the TCP connector but the TLS **server**.
async fn handle_udp_packet(data: Vec<u8>, source: SocketAddr, ctx: Arc<HandshakeCtx>) {
    if ctx.state.rate_limit(source.ip()) {
        debug!(%source, "rate limiting UDP identity");
        return;
    }
    let packet = match decode_packet_line(&data) {
        Ok(packet) => packet,
        Err(e) => {
            debug!(%e, "invalid UDP identity packet");
            return;
        }
    };
    let info = match device_info_from_identity(&packet) {
        Ok(info) => info,
        Err(e) => {
            debug!(%e, "invalid UDP identity");
            return;
        }
    };
    if info.device_id == ctx.state.our_device_id {
        return; // our own broadcast
    }
    let Some(tcp_port) = info.tcp_port else {
        warn!("UDP identity has no tcpPort");
        return;
    };
    if !(TCP_PORT_MIN..=TCP_PORT_MAX).contains(&tcp_port) {
        warn!(tcp_port, "advertised TCP port outside KDE Connect range");
        return;
    }

    let connect = TcpStream::connect(SocketAddr::from((source.ip(), tcp_port)));
    let stream = match tokio::time::timeout(HANDSHAKE_TIMEOUT, connect).await {
        Ok(Ok(stream)) => stream,
        Ok(Err(e)) => {
            debug!(%e, %source, tcp_port, "cannot connect to peer TCP port");
            return;
        }
        Err(_) => {
            debug!(%source, tcp_port, "connect to peer timed out");
            return;
        }
    };

    // Connector side announces who it is targeting before starting TLS.
    let my_identity = targeted_identity_packet(
        &ctx.our_info,
        ctx.our_info.tcp_port.unwrap_or(TCP_PORT_MIN),
        &info.device_id,
        info.protocol_version,
    );
    let mut stream = stream;
    if let Err(e) = stream.write_all(my_identity.serialize().as_bytes()).await {
        debug!(%e, "cannot send plaintext identity");
        return;
    }
    if let Err(e) = stream.flush().await {
        debug!(%e, "cannot flush plaintext identity");
        return;
    }

    let trusted = ctx.store.is_trusted(&info.device_id);
    let config = if trusted {
        match pinned_cert_der(ctx.store.as_ref(), &info.device_id) {
            Ok(der) => match ctx.tls.server_config_trusted(&der) {
                Ok(config) => config,
                Err(e) => {
                    warn!(%e, "cannot build trusted server config");
                    return;
                }
            },
            Err(e) => {
                warn!(%e, "trusted device has no usable certificate");
                return;
            }
        }
    } else {
        match ctx.tls.server_config_untrusted() {
            Ok(config) => config,
            Err(e) => {
                warn!(%e, "cannot build untrusted server config");
                return;
            }
        }
    };

    let mut tls_stream = match TlsAcceptor::from(config).accept(stream).await {
        Ok(stream) => stream,
        Err(e) => {
            debug!(%e, "TLS accept (outgoing connection) failed");
            return;
        }
    };

    let secure_packet = if info.protocol_version >= 8 {
        match exchange_identity_in_tls(
            &mut tls_stream,
            &ctx.our_identity_wire,
            &info.device_id,
            info.protocol_version,
        )
        .await
        {
            Ok(packet) => packet,
            Err(e) => {
                warn!(%e, "v8 identity exchange failed");
                return;
            }
        }
    } else {
        packet
    };

    let tls_stream = tokio_rustls::TlsStream::Server(tls_stream);
    let cert_der = peer_cert_der(&tls_stream);
    let secure_info = match device_info_from_identity(&secure_packet) {
        Ok(info) => info,
        Err(e) => {
            warn!(%e, "invalid in-tunnel identity");
            return;
        }
    };
    register_link(&ctx, tls_stream, secure_info, cert_der, source.ip());
}

/// We accepted an incoming TCP connection: the peer connected to our listener
/// after seeing our broadcast, so we are the TLS **client** (upstream
/// `clientMode = true` for `ConnectionStarted.Locally`).
async fn handle_incoming_tcp(stream: TcpStream, peer: SocketAddr, ctx: Arc<HandshakeCtx>) {
    if ctx.state.rate_limit(peer.ip()) {
        debug!(%peer, "rate limiting TCP connection");
        return;
    }

    let mut stream = stream;
    // Plaintext identity is read byte-by-byte so no TLS ClientHello bytes are
    // consumed (upstream deliberately avoids a BufferedInputStream here).
    let line = match read_line_bounded(&mut stream, MAX_IDENTITY_SIZE).await {
        Ok(line) => line,
        Err(e) => {
            debug!(%e, "cannot read plaintext TCP identity");
            return;
        }
    };
    let packet = match decode_packet_line(&line) {
        Ok(packet) => packet,
        Err(e) => {
            debug!(%e, "invalid plaintext identity packet");
            return;
        }
    };
    let info = match device_info_from_identity(&packet) {
        Ok(info) => info,
        Err(e) => {
            debug!(%e, "invalid plaintext identity");
            return;
        }
    };
    if info.device_id == ctx.state.our_device_id {
        return;
    }
    if let Err(e) = check_targets(&packet, &ctx.state.our_device_id) {
        warn!(%e, "target validation failed");
        return;
    }
    if *ctx.stop_rx.borrow() {
        return;
    }

    let trusted = ctx.store.is_trusted(&info.device_id);
    let config = if trusted {
        match pinned_cert_der(ctx.store.as_ref(), &info.device_id) {
            Ok(der) => match ctx.tls.client_config_trusted(&der) {
                Ok(config) => config,
                Err(e) => {
                    warn!(%e, "cannot build trusted client config");
                    return;
                }
            },
            Err(e) => {
                warn!(%e, "trusted device has no usable certificate");
                return;
            }
        }
    } else {
        match ctx.tls.client_config_untrusted() {
            Ok(config) => config,
            Err(e) => {
                warn!(%e, "cannot build untrusted client config");
                return;
            }
        }
    };

    let name = ServerName::from(peer.ip());
    let mut tls_stream = match TlsConnector::from(config).connect(name, stream).await {
        Ok(stream) => stream,
        Err(e) => {
            debug!(%e, "TLS connect (incoming connection) failed");
            return;
        }
    };

    let secure_packet = if info.protocol_version >= 8 {
        match exchange_identity_in_tls(
            &mut tls_stream,
            &ctx.our_identity_wire,
            &info.device_id,
            info.protocol_version,
        )
        .await
        {
            Ok(packet) => packet,
            Err(e) => {
                warn!(%e, "v8 identity exchange failed");
                return;
            }
        }
    } else {
        packet
    };

    let tls_stream = tokio_rustls::TlsStream::Client(tls_stream);
    let cert_der = peer_cert_der(&tls_stream);
    let secure_info = match device_info_from_identity(&secure_packet) {
        Ok(info) => info,
        Err(e) => {
            warn!(%e, "invalid in-tunnel identity");
            return;
        }
    };
    register_link(&ctx, tls_stream, secure_info, cert_der, peer.ip());
}

/// Send our identity inside the TLS tunnel and validate the answer. Both peers
/// write first, so this cannot deadlock.
async fn exchange_identity_in_tls<S>(
    stream: &mut S,
    our_identity_wire: &str,
    expected_id: &str,
    expected_version: u32,
) -> Result<NetworkPacket>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    stream.write_all(our_identity_wire.as_bytes()).await?;
    stream.flush().await?;
    let line = read_line_bounded(stream, MAX_IDENTITY_SIZE).await?;
    let packet = decode_packet_line(&line)?;
    let info = device_info_from_identity(&packet)?;
    if !is_valid_identity_packet(&packet) || info.device_id != expected_id {
        return Err(ProtocolError::InvalidIdentity(format!(
            "in-tunnel identity device changed: {expected_id} -> {}",
            info.device_id
        ))
        .into());
    }
    if info.protocol_version != expected_version {
        return Err(ProtocolError::InvalidIdentity(format!(
            "protocol version changed in tunnel: {expected_version} -> {}",
            info.protocol_version
        ))
        .into());
    }
    Ok(packet)
}

/// Peer certificate from the TLS connection (works for both trust-all and
/// pinned verifiers, and is what upstream takes from `HandshakeCompletedEvent`).
fn peer_cert_der(stream: &LanTlsStream) -> Vec<u8> {
    stream
        .get_ref()
        .1
        .peer_certificates()
        .and_then(|certs| certs.first())
        .map(|cert| cert.as_ref().to_vec())
        .unwrap_or_default()
}

fn register_link(
    ctx: &HandshakeCtx,
    stream: LanTlsStream,
    info: DeviceInfo,
    cert_der: Vec<u8>,
    peer_addr: IpAddr,
) {
    let id = ctx.state.next_link_id.fetch_add(1, Ordering::SeqCst);
    let (reader, writer) = tokio::io::split(stream);
    let link = Arc::new(LanLink::new(
        id,
        info.device_id.clone(),
        peer_addr,
        cert_der.clone(),
        ctx.tls.clone(),
        writer,
        ctx.sink.clone(),
    ));
    ctx.state.links.lock().unwrap().insert(id, link.clone());
    let _ = ctx.sink.send(LinkEvent::Connected {
        link: link.clone(),
        info,
        cert_der,
    });
    link.start_read_loop(reader);
}

#[cfg(test)]
mod tests {
    use super::*;
    use kc_core::MemoryStore;
    use kc_protocol::{generate_identity, DeviceType};

    /// Both roles complete a v8 handshake locally: A owns the TCP listener and
    /// is the TLS client, B reacts to A's UDP identity and is the TLS server.
    #[tokio::test]
    async fn two_peers_complete_v8_tls_handshake() {
        let id_a = generate_identity("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap();
        let id_b = generate_identity("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb").unwrap();
        let store_a = Arc::new(MemoryStore::new());
        let store_b = Arc::new(MemoryStore::new());
        store_a
            .trust_device(&id_b.device_id, "B", "phone", &id_b.cert_der)
            .unwrap();
        store_b
            .trust_device(&id_a.device_id, "A", "desktop", &id_a.cert_der)
            .unwrap();

        let (listener_a, port_a) = bind_tcp_listener(TCP_PORT_MIN..=TCP_PORT_MAX)
            .await
            .unwrap();
        let info_a = DeviceInfo::new(id_a.device_id.clone(), "A", DeviceType::Desktop);
        let info_b = DeviceInfo::new(id_b.device_id.clone(), "B", DeviceType::Phone);
        let identity_wire_a = identity_with_tcp_port(&info_a, port_a).serialize();

        let (sink_a, mut rx_a) = mpsc::unbounded_channel();
        let (sink_b, mut rx_b) = mpsc::unbounded_channel();
        let (_, stop_rx_a) = watch::channel(false);
        let (_, stop_rx_b) = watch::channel(false);

        let mut a_info = info_a.clone();
        a_info.tcp_port = Some(port_a);
        let ctx_a = Arc::new(HandshakeCtx {
            our_info: a_info,
            our_identity_wire: identity_wire_a.clone(),
            state: Arc::new(State::new(id_a.device_id.clone())),
            tls: TlsContext::new(id_a.clone()),
            store: store_a,
            sink: sink_a,
            stop_rx: stop_rx_a,
        });
        let ctx_b = Arc::new(HandshakeCtx {
            our_info: info_b.clone(),
            our_identity_wire: identity_with_tcp_port(&info_b, port_a).serialize(),
            state: Arc::new(State::new(id_b.device_id.clone())),
            tls: TlsContext::new(id_b.clone()),
            store: store_b,
            sink: sink_b,
            stop_rx: stop_rx_b,
        });

        let accept_a = {
            let ctx_a = ctx_a.clone();
            tokio::spawn(async move {
                let (stream, peer) = listener_a.accept().await.unwrap();
                handle_incoming_tcp(stream, peer, ctx_a).await;
            })
        };

        // B receives A's broadcast (source port is irrelevant; tcpPort matters).
        handle_udp_packet(
            identity_wire_a.into_bytes(),
            SocketAddr::from((Ipv4Addr::LOCALHOST, 54321)),
            ctx_b,
        )
        .await;

        let (peer_on_a, cert_on_a) = match rx_a.recv().await.unwrap() {
            LinkEvent::Connected { info, cert_der, .. } => (info.device_id, cert_der),
            other => panic!("A expected Connected, got {other:?}"),
        };
        let (peer_on_b, cert_on_b) = match rx_b.recv().await.unwrap() {
            LinkEvent::Connected { info, cert_der, .. } => (info.device_id, cert_der),
            other => panic!("B expected Connected, got {other:?}"),
        };
        assert_eq!(peer_on_a, id_b.device_id);
        assert_eq!(peer_on_b, id_a.device_id);
        assert_eq!(cert_on_a, id_b.cert_der);
        assert_eq!(cert_on_b, id_a.cert_der);

        accept_a.await.unwrap();
    }
}
