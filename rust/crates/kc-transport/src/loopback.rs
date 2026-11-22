//! Loopback transport, mirroring upstream `LoopbackLinkProvider` /
//! `LoopbackLink`: one self-connected link whose sent packets are echoed back.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use kc_core::{Link, LinkEvent, LinkProvider, Result};
use kc_protocol::{DeviceInfo, NetworkPacket};
use tokio::sync::{mpsc, watch};

static NEXT_LOOPBACK_ID: AtomicU64 = AtomicU64::new(1);

/// A link to ourselves. Packets sent on it are delivered straight back as
/// [`LinkEvent::Packet`], with no network involved.
pub struct LoopbackLink {
    id: u64,
    info: DeviceInfo,
    sink: mpsc::UnboundedSender<LinkEvent>,
    disconnected: AtomicBool,
}

impl std::fmt::Debug for LoopbackLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoopbackLink")
            .field("id", &self.id)
            .field("device_id", &self.info.device_id)
            .finish()
    }
}

impl LoopbackLink {
    fn new(info: DeviceInfo, sink: mpsc::UnboundedSender<LinkEvent>) -> Self {
        Self {
            id: NEXT_LOOPBACK_ID.fetch_add(1, Ordering::SeqCst),
            info,
            sink,
            disconnected: AtomicBool::new(false),
        }
    }

    fn emit_disconnected(&self) {
        if self.disconnected.swap(true, Ordering::SeqCst) {
            return;
        }
        let _ = self.sink.send(LinkEvent::Disconnected {
            link_id: self.id,
            device_id: self.info.device_id.clone(),
        });
    }
}

#[async_trait]
impl Link for LoopbackLink {
    fn id(&self) -> u64 {
        self.id
    }

    fn priority(&self) -> u8 {
        0
    }

    fn peer_device_id(&self) -> &str {
        &self.info.device_id
    }

    async fn send(&self, packet: &NetworkPacket) -> Result<()> {
        let _ = self.sink.send(LinkEvent::Packet {
            device_id: self.info.device_id.clone(),
            packet: packet.clone(),
        });
        Ok(())
    }

    async fn close(&self) {
        self.emit_disconnected();
    }
}

/// Discovery provider that immediately creates one [`LoopbackLink`].
pub struct LoopbackLinkProvider {
    info: DeviceInfo,
    stop_tx: watch::Sender<bool>,
    link: Mutex<Option<Arc<LoopbackLink>>>,
}

impl LoopbackLinkProvider {
    pub fn new(info: DeviceInfo) -> Self {
        let (stop_tx, _) = watch::channel(false);
        Self {
            info,
            stop_tx,
            link: Mutex::new(None),
        }
    }
}

#[async_trait]
impl LinkProvider for LoopbackLinkProvider {
    fn name(&self) -> &'static str {
        "LoopbackLinkProvider"
    }

    async fn start(&self, sink: mpsc::UnboundedSender<LinkEvent>) -> Result<()> {
        // Upstream's loopback link swaps the capability sets: what we can send
        // is what the loopback peer can receive, and vice versa.
        let mut info = self.info.clone();
        std::mem::swap(
            &mut info.incoming_capabilities,
            &mut info.outgoing_capabilities,
        );
        let link = Arc::new(LoopbackLink::new(info.clone(), sink));
        *self.link.lock().unwrap() = Some(link.clone());

        let event_sink = link.sink.clone();
        let _ = event_sink.send(LinkEvent::Connected {
            link,
            info,
            cert_der: Vec::new(),
        });

        let mut stop_rx = self.stop_tx.subscribe();
        while !*stop_rx.borrow() {
            if stop_rx.changed().await.is_err() {
                break;
            }
        }

        let link = self.link.lock().unwrap().take();
        if let Some(link) = link {
            link.close().await;
        }
        Ok(())
    }

    async fn stop(&self) {
        let _ = self.stop_tx.send(true);
        let link = self.link.lock().unwrap().take();
        if let Some(link) = link {
            link.close().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kc_protocol::{DeviceType, PACKET_TYPE_PAIR};

    #[tokio::test]
    async fn loopback_provider_echoes_packets() {
        let info = DeviceInfo::new(
            "0123456789abcdef0123456789abcdef",
            "Loopback",
            DeviceType::Phone,
        );
        let provider = Arc::new(LoopbackLinkProvider::new(info.clone()));
        let (tx, mut rx) = mpsc::unbounded_channel();

        let provider_clone = provider.clone();
        let start = tokio::spawn(async move { provider_clone.start(tx).await });

        let connected = rx.recv().await.unwrap();
        let link = match connected {
            LinkEvent::Connected { link, .. } => link,
            other => panic!("expected Connected, got {other:?}"),
        };
        assert_eq!(link.peer_device_id(), info.device_id);

        let mut packet = NetworkPacket::new(PACKET_TYPE_PAIR);
        packet.set_str("key", "value");
        link.send(&packet).await.unwrap();
        match rx.recv().await.unwrap() {
            LinkEvent::Packet {
                device_id,
                packet: echoed,
            } => {
                assert_eq!(device_id, info.device_id);
                assert_eq!(echoed.get_str("key"), Some("value"));
            }
            other => panic!("expected Packet, got {other:?}"),
        }

        provider.stop().await;
        start.await.unwrap().unwrap();
    }
}
