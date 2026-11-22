//! `FindMyPhonePlugin`: handle a remote "ring my phone" request.
//!
//! The incoming `kdeconnect.findmyphone.request` packet has an empty body;
//! the host decides how to alert the user.

use kc_core::{Platform, Plugin, PluginContext, Result};
use kc_protocol::NetworkPacket;

/// Shared packet type: also emitted by the FindRemoteDevice plugin.
pub const PACKET_TYPE_FIND_MY_PHONE: &str = "kdeconnect.findmyphone.request";

#[derive(Default, Debug)]
pub struct FindMyPhonePlugin;

impl FindMyPhonePlugin {
    pub(crate) fn handle(packet: &NetworkPacket, device_id: &str, platform: &dyn Platform) -> bool {
        if packet.ty != PACKET_TYPE_FIND_MY_PHONE {
            return false;
        }
        platform.on_find_my_phone(device_id);
        true
    }
}

impl Plugin for FindMyPhonePlugin {
    fn key(&self) -> &'static str {
        "FindMyPhonePlugin"
    }
    fn display_name(&self) -> &'static str {
        "Find my phone"
    }
    fn description(&self) -> &'static str {
        "Ring this device when requested by a paired device"
    }
    fn supported_packet_types(&self) -> &'static [&'static str] {
        &[PACKET_TYPE_FIND_MY_PHONE]
    }
    fn outgoing_packet_types(&self) -> &'static [&'static str] {
        &[]
    }

    fn on_packet(&self, ctx: &PluginContext, packet: &NetworkPacket) -> Result<bool> {
        Ok(Self::handle(packet, &ctx.device_id, ctx.platform.as_ref()))
    }
}

/// Build the empty `kdeconnect.findmyphone.request` packet.
pub fn find_packet() -> NetworkPacket {
    NetworkPacket::new(PACKET_TYPE_FIND_MY_PHONE)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use kc_protocol::DeviceType;

    use super::*;

    #[derive(Default)]
    struct RecordingPlatform {
        finds: AtomicUsize,
    }

    impl Platform for RecordingPlatform {
        fn device_name(&self) -> String {
            "test".to_owned()
        }
        fn device_type(&self) -> DeviceType {
            DeviceType::Phone
        }

        fn on_find_my_phone(&self, _device_id: &str) {
            self.finds.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn find_packet_roundtrips_with_empty_body() {
        let packet = find_packet();
        assert_eq!(packet.ty, PACKET_TYPE_FIND_MY_PHONE);
        assert!(packet.body.is_empty());
        let back = NetworkPacket::deserialize(&packet.serialize()).unwrap();
        assert_eq!(back.ty, PACKET_TYPE_FIND_MY_PHONE);
        assert!(back.body.is_empty());
    }

    #[test]
    fn handle_rings_host_only_for_request_type() {
        let platform = RecordingPlatform::default();
        assert!(FindMyPhonePlugin::handle(&find_packet(), "dev", &platform));
        assert_eq!(platform.finds.load(Ordering::SeqCst), 1);

        let other = NetworkPacket::new("kdeconnect.ping");
        assert!(!FindMyPhonePlugin::handle(&other, "dev", &platform));
        assert_eq!(platform.finds.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn plugin_declares_no_outgoing_types() {
        assert!(FindMyPhonePlugin.outgoing_packet_types().is_empty());
    }
}
