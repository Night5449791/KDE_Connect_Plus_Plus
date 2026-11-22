//! `PingPlugin`: lightweight connectivity check.
//!
//! Incoming `kdeconnect.ping` packets are reported to the host through
//! [`kc_core::Platform::on_ping`]; [`ping_packet`] builds the outgoing form.

use kc_core::{Platform, Plugin, PluginContext, Result};
use kc_protocol::NetworkPacket;

/// Packet type handled and emitted by this plugin.
pub const PACKET_TYPE_PING: &str = "kdeconnect.ping";

#[derive(Default, Debug)]
pub struct PingPlugin;

impl PingPlugin {
    /// Core of `on_packet`, factored out so it can be tested without a full
    /// `PluginContext` (which needs a tokio channel).
    pub(crate) fn handle(packet: &NetworkPacket, device_id: &str, platform: &dyn Platform) -> bool {
        if packet.ty != PACKET_TYPE_PING {
            return false;
        }
        let message = packet.get_string("message");
        platform.on_ping(device_id, message.as_deref());
        true
    }
}

impl Plugin for PingPlugin {
    fn key(&self) -> &'static str {
        "PingPlugin"
    }
    fn display_name(&self) -> &'static str {
        "Ping"
    }
    fn description(&self) -> &'static str {
        "Check the connection with a paired device"
    }
    fn supported_packet_types(&self) -> &'static [&'static str] {
        &[PACKET_TYPE_PING]
    }
    fn outgoing_packet_types(&self) -> &'static [&'static str] {
        &[PACKET_TYPE_PING]
    }

    fn on_packet(&self, ctx: &PluginContext, packet: &NetworkPacket) -> Result<bool> {
        Ok(Self::handle(packet, &ctx.device_id, ctx.platform.as_ref()))
    }
}

/// Build a `kdeconnect.ping` packet. `None` leaves the body empty, matching
/// upstream when no message is supplied.
pub fn ping_packet(message: Option<&str>) -> NetworkPacket {
    let mut packet = NetworkPacket::new(PACKET_TYPE_PING);
    if let Some(message) = message {
        packet.set_str("message", message);
    }
    packet
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    use kc_protocol::DeviceType;

    use super::*;

    #[derive(Default)]
    struct RecordingPlatform {
        pings: AtomicUsize,
        last_message: Mutex<Option<String>>,
    }

    impl Platform for RecordingPlatform {
        fn device_name(&self) -> String {
            "test".to_owned()
        }
        fn device_type(&self) -> DeviceType {
            DeviceType::Desktop
        }

        fn on_ping(&self, _device_id: &str, message: Option<&str>) {
            self.pings.fetch_add(1, Ordering::SeqCst);
            *self.last_message.lock().unwrap() = message.map(str::to_owned);
        }
    }

    #[test]
    fn ping_packet_roundtrips_through_wire() {
        let packet = ping_packet(Some("hello"));
        let back = NetworkPacket::deserialize(&packet.serialize()).unwrap();
        assert_eq!(back.ty, PACKET_TYPE_PING);
        assert_eq!(back.get_string("message").as_deref(), Some("hello"));
    }

    #[test]
    fn ping_packet_without_message_has_empty_body() {
        let packet = ping_packet(None);
        assert!(packet.body.is_empty());
        let back = NetworkPacket::deserialize(&packet.serialize()).unwrap();
        assert!(back.body.is_empty());
        assert_eq!(back.get_str("message"), None);
    }

    #[test]
    fn handle_notifies_host_and_ignores_other_types() {
        let platform = RecordingPlatform::default();
        assert!(PingPlugin::handle(
            &ping_packet(Some("ping me")),
            "dev",
            &platform
        ));
        assert_eq!(platform.pings.load(Ordering::SeqCst), 1);
        assert_eq!(
            platform.last_message.lock().unwrap().as_deref(),
            Some("ping me")
        );

        let other = NetworkPacket::new("kdeconnect.battery");
        assert!(!PingPlugin::handle(&other, "dev", &platform));
        assert_eq!(platform.pings.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn plugin_metadata_keeps_upstream_type_lists() {
        let plugin = PingPlugin;
        assert_eq!(plugin.supported_packet_types(), &[PACKET_TYPE_PING]);
        assert_eq!(plugin.outgoing_packet_types(), &[PACKET_TYPE_PING]);
    }
}
