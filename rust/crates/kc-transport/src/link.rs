//! One established LAN connection: a TLS 1.2 TCP stream plus an optional
//! second TLS connection for payload bytes.

use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use kc_core::{CoreError, Link, LinkEvent, Result};
use kc_protocol::packet::MAX_PACKET_SIZE;
use kc_protocol::{NetworkPacket, Payload, TlsContext};
use rustls_pki_types::ServerName;
use serde_json::Map;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader, ReadHalf, WriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, Mutex as AsyncMutex};
use tokio::task::JoinHandle;
use tokio_rustls::{TlsAcceptor, TlsConnector, TlsStream};
use tracing::{debug, warn};

use crate::framing::{decode_packet_line, read_line_bounded};
use crate::net::{bind_tcp_listener, PAYLOAD_PORT_MIN, TCP_PORT_MAX};

pub(crate) type LanTlsStream = TlsStream<TcpStream>;

/// A single LAN link. The write half is shared by all senders; the read half is
/// owned by a dedicated task spawned by [`LanLink::start_read_loop`].
pub struct LanLink {
    id: u64,
    peer_device_id: String,
    peer_addr: IpAddr,
    cert_der: Vec<u8>,
    tls: TlsContext,
    write: AsyncMutex<WriteHalf<LanTlsStream>>,
    read_task: Mutex<Option<JoinHandle<()>>>,
    sink: mpsc::UnboundedSender<LinkEvent>,
    disconnected: AtomicBool,
}

impl std::fmt::Debug for LanLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LanLink")
            .field("id", &self.id)
            .field("peer_device_id", &self.peer_device_id)
            .field("peer_addr", &self.peer_addr)
            .field("cert_der_len", &self.cert_der.len())
            .finish()
    }
}

impl LanLink {
    pub(crate) fn new(
        id: u64,
        peer_device_id: String,
        peer_addr: IpAddr,
        cert_der: Vec<u8>,
        tls: TlsContext,
        write: WriteHalf<LanTlsStream>,
        sink: mpsc::UnboundedSender<LinkEvent>,
    ) -> Self {
        Self {
            id,
            peer_device_id,
            peer_addr,
            cert_der,
            tls,
            write: AsyncMutex::new(write),
            read_task: Mutex::new(None),
            sink,
            disconnected: AtomicBool::new(false),
        }
    }

    /// Spawn the per-link read loop. `reader` is buffered; framing itself does
    /// not read past the newline of a packet.
    pub(crate) fn start_read_loop(self: &std::sync::Arc<Self>, reader: ReadHalf<LanTlsStream>) {
        let link = self.clone();
        let handle = tokio::spawn(async move {
            lan_read_loop(link, BufReader::new(reader)).await;
        });
        *self.read_task.lock().unwrap() = Some(handle);
    }

    pub(crate) fn emit_disconnected(&self) {
        if self.disconnected.swap(true, Ordering::SeqCst) {
            return;
        }
        let _ = self.sink.send(LinkEvent::Disconnected {
            link_id: self.id,
            device_id: self.peer_device_id.clone(),
        });
    }

    /// Fetch the raw payload of an incoming packet. There is no length header:
    /// upstream streams exactly `payloadSize` bytes over a second TLS socket.
    pub(crate) async fn fetch_payload(&self, port: u16, size: u64) -> Result<Vec<u8>> {
        fetch_payload(self.peer_addr, port, size, &self.tls, &self.cert_der).await
    }
}

#[async_trait]
impl Link for LanLink {
    fn id(&self) -> u64 {
        self.id
    }

    fn priority(&self) -> u8 {
        20
    }

    fn peer_device_id(&self) -> &str {
        &self.peer_device_id
    }

    async fn send(&self, packet: &NetworkPacket) -> Result<()> {
        // `send` takes &NetworkPacket, so clone before filling in the
        // transfer info allocated by this link.
        let mut wire_packet = packet.clone();
        let payload_source = match packet.payload.as_ref() {
            Some(payload) => {
                let size = match payload {
                    Payload::Bytes(bytes) => bytes.len() as u64,
                    Payload::File(path) => tokio::fs::metadata(path).await?.len(),
                };
                let (listener, port) = bind_tcp_listener(PAYLOAD_PORT_MIN..=TCP_PORT_MAX).await?;
                let mut info = Map::new();
                info.insert("port".to_owned(), serde_json::Value::from(port));
                wire_packet.payload_transfer_info = Some(info);
                wire_packet.payload_size = Some(size);
                Some((listener, payload.clone(), size))
            }
            None => None,
        };

        // NetworkPacket::serialize() already appends '\n'.
        let wire = wire_packet.serialize();
        {
            let mut writer = self.write.lock().await;
            writer.write_all(wire.as_bytes()).await?;
            writer.flush().await?;
        }

        if let Some((listener, payload, size)) = payload_source {
            let tls = self.tls.clone();
            let cert_der = self.cert_der.clone();
            let device_id = self.peer_device_id.clone();
            tokio::spawn(async move {
                if let Err(e) =
                    send_payload(listener, payload, size, tls, cert_der, &device_id).await
                {
                    warn!(%device_id, %e, "payload transfer failed");
                }
            });
        }
        Ok(())
    }

    async fn close(&self) {
        if let Some(handle) = self.read_task.lock().unwrap().take() {
            handle.abort();
        }
        self.emit_disconnected();
    }
}

/// Serve one payload stream on a dedicated TLS connection. The sender is the
/// TLS server (upstream `clientMode = false` in `LanLink.sendPayload`).
pub async fn send_payload(
    listener: TcpListener,
    payload: Payload,
    size: u64,
    tls: TlsContext,
    cert_der: Vec<u8>,
    device_id: &str,
) -> Result<()> {
    let (stream, _) = tokio::time::timeout(Duration::from_secs(10), listener.accept())
        .await
        .map_err(|_| CoreError::Transport("payload receiver timed out".to_owned()))??;

    let config = tls.server_config_trusted(&cert_der)?;
    let mut tls_stream = TlsAcceptor::from(config).accept(stream).await?;

    match payload {
        Payload::Bytes(bytes) => {
            tls_stream.write_all(&bytes).await?;
        }
        Payload::File(path) => {
            let mut file = tokio::fs::File::open(&path).await?;
            let copied = tokio::io::copy(&mut file, &mut tls_stream).await?;
            if size > 0 && copied < size {
                return Err(CoreError::Transport(format!(
                    "short payload file read: {copied} of {size} bytes"
                )));
            }
        }
    }
    tls_stream.flush().await?;
    let _ = tls_stream.shutdown().await;
    debug!(%device_id, size, "payload sent");
    Ok(())
}

/// Read exactly `size` raw bytes from the peer's payload server. The receiver
/// is the TLS client (upstream `clientMode = true` in `receivedNetworkPacket`).
pub async fn fetch_payload(
    peer_addr: IpAddr,
    port: u16,
    size: u64,
    tls: &TlsContext,
    cert_der: &[u8],
) -> Result<Vec<u8>> {
    if size > MAX_PACKET_SIZE as u64 {
        return Err(CoreError::Transport(format!(
            "refusing payload of {size} bytes (limit {MAX_PACKET_SIZE})"
        )));
    }
    let stream = TcpStream::connect((peer_addr, port)).await?;
    let config = tls.client_config_trusted(cert_der)?;
    let name = ServerName::from(peer_addr);
    let mut tls_stream = TlsConnector::from(config).connect(name, stream).await?;

    let mut bytes = vec![0u8; size as usize];
    tls_stream.read_exact(&mut bytes).await?;
    Ok(bytes)
}

/// Decode packets from the main TLS stream and emit them, fetching any
/// advertised payload before the `LinkEvent::Packet` is emitted.
async fn lan_read_loop(
    link: std::sync::Arc<LanLink>,
    mut reader: BufReader<ReadHalf<LanTlsStream>>,
) {
    loop {
        let line = match read_line_bounded(&mut reader, MAX_PACKET_SIZE).await {
            Ok(line) => line,
            Err(e) => {
                debug!(%e, "LAN read loop ended");
                break;
            }
        };
        if line.len() <= 1 {
            continue;
        }
        let mut packet = match decode_packet_line(&line) {
            Ok(packet) => packet,
            Err(e) => {
                warn!(%e, "dropping malformed packet from LAN link");
                continue;
            }
        };

        if packet.payload_size.unwrap_or(0) > 0 {
            if let Some(port) = packet
                .payload_transfer_info
                .as_ref()
                .and_then(|info| info.get("port"))
                .and_then(|v| v.as_u64())
                .and_then(|v| u16::try_from(v).ok())
            {
                let size = packet.payload_size.unwrap_or(0);
                match link.fetch_payload(port, size).await {
                    Ok(bytes) => packet.payload = Some(Payload::Bytes(bytes)),
                    Err(e) => warn!(%e, port, "could not fetch packet payload"),
                }
            } else {
                warn!("packet has payloadSize but no usable payloadTransferInfo port");
            }
        }

        if link
            .sink
            .send(LinkEvent::Packet {
                device_id: link.peer_device_id.clone(),
                packet,
            })
            .is_err()
        {
            break;
        }
    }
    link.emit_disconnected();
}

#[cfg(test)]
mod tests {
    use super::*;
    use kc_protocol::generate_identity;
    use std::net::Ipv4Addr;

    #[tokio::test]
    async fn payload_streams_exact_bytes_over_second_tls_connection() {
        let identity = generate_identity("0123456789abcdef0123456789abcdef").unwrap();
        let tls = TlsContext::new(identity.clone());
        let (listener, _) = bind_tcp_listener(PAYLOAD_PORT_MIN..=TCP_PORT_MAX)
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();

        let bytes: Vec<u8> = (0..4096u32).map(|i| (i % 251) as u8).collect();
        let sender = tokio::spawn(send_payload(
            listener,
            Payload::Bytes(bytes.clone()),
            bytes.len() as u64,
            tls.clone(),
            identity.cert_der.clone(),
            "peer",
        ));

        let received = fetch_payload(
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            port,
            bytes.len() as u64,
            &tls,
            &identity.cert_der,
        )
        .await
        .unwrap();
        assert_eq!(received, bytes);
        sender.await.unwrap().unwrap();
    }
}
