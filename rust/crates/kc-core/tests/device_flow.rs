//! End-to-end state-machine tests without a real network.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use kc_core::{
    DeviceEvent, Link, LinkEvent, MemoryStore, NoopPlatform, Platform, Plugin, PluginContext,
    PluginRegistry, Result, Store,
};
use kc_protocol::{generate_identity, DeviceInfo, DeviceType, NetworkPacket, PACKET_TYPE_IDENTITY};

#[derive(Debug)]
struct MockLink {
    id: u64,
    peer: String,
    sent: Mutex<Vec<NetworkPacket>>,
}

#[async_trait]
impl Link for MockLink {
    fn id(&self) -> u64 {
        self.id
    }
    fn priority(&self) -> u8 {
        20
    }
    fn peer_device_id(&self) -> &str {
        &self.peer
    }
    async fn send(&self, packet: &NetworkPacket) -> Result<()> {
        self.sent.lock().unwrap().push(packet.clone());
        Ok(())
    }
    async fn close(&self) {}
}

#[derive(Default)]
struct RecordingPlatform {
    pings: AtomicUsize,
    pairing_requests: AtomicUsize,
    last_key: Mutex<String>,
}

impl Platform for RecordingPlatform {
    fn device_name(&self) -> String {
        "Test Host".to_owned()
    }
    fn device_type(&self) -> DeviceType {
        DeviceType::Desktop
    }
    fn on_ping(&self, _device_id: &str, _message: Option<&str>) {
        self.pings.fetch_add(1, Ordering::SeqCst);
    }
    fn on_pairing_request(&self, _device_id: &str, verification_key: &str) {
        self.pairing_requests.fetch_add(1, Ordering::SeqCst);
        *self.last_key.lock().unwrap() = verification_key.to_owned();
    }
}

#[derive(Default, Debug)]
struct PingResponder;

impl Plugin for PingResponder {
    fn key(&self) -> &'static str {
        "PingResponder"
    }
    fn display_name(&self) -> &'static str {
        "Ping"
    }
    fn description(&self) -> &'static str {
        "test"
    }
    fn supported_packet_types(&self) -> &'static [&'static str] {
        &["kdeconnect.ping"]
    }
    fn outgoing_packet_types(&self) -> &'static [&'static str] {
        &["kdeconnect.ping"]
    }
    fn on_packet(&self, ctx: &PluginContext, packet: &NetworkPacket) -> Result<bool> {
        ctx.platform
            .on_ping(&ctx.device_id, packet.get_str("message"));
        Ok(true)
    }
}

fn registry() -> Arc<PluginRegistry> {
    let mut r = PluginRegistry::new();
    r.register(Arc::new(PingResponder));
    Arc::new(r)
}

fn identity() -> kc_protocol::Identity {
    generate_identity("0123456789abcdef0123456789abcdef").unwrap()
}

fn info(id: &str) -> DeviceInfo {
    let mut info = DeviceInfo::new(id, "Peer", DeviceType::Phone);
    info.incoming_capabilities
        .insert("kdeconnect.ping".to_owned());
    info.outgoing_capabilities
        .insert("kdeconnect.ping".to_owned());
    info
}

#[tokio::test]
async fn plugin_packet_is_dispatched_only_when_paired() {
    let platform = Arc::new(RecordingPlatform::default());
    let store = Arc::new(MemoryStore::new());
    let manager = kc_core::DeviceManager::new(identity(), store, platform.clone(), registry());

    let link = Arc::new(MockLink {
        id: 1,
        peer: "peer-1".to_owned(),
        sent: Mutex::new(Vec::new()),
    });
    manager
        .handle_link_event(LinkEvent::Connected {
            link: link.clone(),
            info: info("peer-1"),
            cert_der: identity().cert_der,
        })
        .await
        .unwrap();

    let mut ping = NetworkPacket::new("kdeconnect.ping");
    ping.set_str("message", "hi");
    manager.on_packet("peer-1", ping).await.unwrap();
    assert_eq!(
        platform.pings.load(Ordering::SeqCst),
        0,
        "unpaired packet must be ignored"
    );

    // Simulate a successful pairing via the manager (no real link response needed
    // for the state transition itself: we inject the pair accept packet).
    let mut pair = NetworkPacket::new(kc_protocol::PACKET_TYPE_PAIR);
    pair.set_bool("pair", true);
    pair.set_i64("timestamp", kc_protocol::pairing::now_seconds());
    manager
        .get("peer-1")
        .await
        .unwrap()
        .request_pairing()
        .await
        .unwrap();
    manager.on_packet("peer-1", pair).await.unwrap();
    assert!(manager.get("peer-1").await.unwrap().is_paired().await);

    let mut ping = NetworkPacket::new("kdeconnect.ping");
    ping.set_str("message", "hi");
    manager.on_packet("peer-1", ping).await.unwrap();
    assert_eq!(platform.pings.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn remembered_devices_are_loaded_from_store() {
    let store = Arc::new(MemoryStore::new());
    let own = identity();
    let peer = identity();
    store
        .trust_device("peer-2", "Remembered", "phone", &peer.cert_der)
        .unwrap();
    let manager = kc_core::DeviceManager::new(own, store, Arc::new(NoopPlatform), registry());
    manager.load_remembered_devices().await.unwrap();

    let devices = manager.devices().await;
    assert_eq!(devices.len(), 1);
    let snap = devices[0].snapshot().await;
    assert_eq!(snap.name, "Remembered");
    assert!(snap.paired);
    assert!(!snap.reachable);
}

#[tokio::test]
async fn identity_packet_type_is_stable() {
    assert_eq!(PACKET_TYPE_IDENTITY, "kdeconnect.identity");
    let packet = info("peer-3").to_identity_packet();
    assert_eq!(packet.ty, PACKET_TYPE_IDENTITY);
}

#[tokio::test]
async fn device_events_are_emitted_on_connect() {
    let manager = kc_core::DeviceManager::new(
        identity(),
        Arc::new(MemoryStore::new()),
        Arc::new(NoopPlatform),
        registry(),
    );
    let mut rx = manager.subscribe();
    let link = Arc::new(MockLink {
        id: 7,
        peer: "peer-4".to_owned(),
        sent: Mutex::new(Vec::new()),
    });
    manager
        .handle_link_event(LinkEvent::Connected {
            link,
            info: info("peer-4"),
            cert_der: identity().cert_der,
        })
        .await
        .unwrap();
    let event = rx.try_recv().unwrap();
    assert!(matches!(
        event,
        DeviceEvent::Discovered { device_id, .. } if device_id == "peer-4"
    ));
}
