//! Transport abstraction. Implemented by `kc-transport` for LAN/mDNS/loopback
//! and by the Android shell for Bluetooth.

use std::fmt::Debug;
use std::sync::Arc;

use async_trait::async_trait;
use kc_protocol::{DeviceInfo, NetworkPacket};
use tokio::sync::mpsc;

use crate::Result;

/// One established connection to a remote device.
#[async_trait]
pub trait Link: Debug + Send + Sync {
    /// Stable identifier for this connection instance.
    fn id(&self) -> u64;
    /// Higher priority links are preferred when sending (Bluetooth 10, LAN 20).
    fn priority(&self) -> u8;
    /// Device ID of the peer on this link.
    fn peer_device_id(&self) -> &str;
    /// Send one serialized packet. The transport may add framing / payload transfer.
    async fn send(&self, packet: &NetworkPacket) -> Result<()>;
    /// Close the connection.
    async fn close(&self);
}

/// Events emitted by a link provider into the device manager.
#[derive(Debug)]
pub enum LinkEvent {
    Connected {
        link: Arc<dyn Link>,
        info: DeviceInfo,
        /// DER bytes of the peer certificate, when available (TLS/Bluetooth).
        cert_der: Vec<u8>,
    },
    /// A fully decoded packet from a link, including any streamed payload.
    Packet {
        device_id: String,
        packet: NetworkPacket,
    },
    Disconnected {
        link_id: u64,
        device_id: String,
    },
}

/// A discovery mechanism (UDP broadcast + mDNS, Bluetooth RFCOMM, loopback).
#[async_trait]
pub trait LinkProvider: Send + Sync {
    fn name(&self) -> &'static str;

    /// Start discovering. Emit events on `sink`; return when stopped.
    async fn start(&self, sink: mpsc::UnboundedSender<LinkEvent>) -> Result<()>;

    /// Stop discovery and all links created by this provider.
    async fn stop(&self);
}
