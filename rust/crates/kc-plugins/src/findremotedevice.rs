//! `FindRemoteDevicePlugin`: outgoing-only counterpart of FindMyPhone.
//!
//! It has no incoming packet types; sending the request makes the paired
//! device ring itself.

use kc_core::{Plugin, PluginContext, Result};
use kc_protocol::NetworkPacket;

use crate::findmyphone::PACKET_TYPE_FIND_MY_PHONE;

#[derive(Default, Debug)]
pub struct FindRemoteDevicePlugin;

impl Plugin for FindRemoteDevicePlugin {
    fn key(&self) -> &'static str {
        "FindRemoteDevicePlugin"
    }
    fn display_name(&self) -> &'static str {
        "Find remote device"
    }
    fn description(&self) -> &'static str {
        "Ring a paired device to locate it"
    }
    fn supported_packet_types(&self) -> &'static [&'static str] {
        &[]
    }
    fn outgoing_packet_types(&self) -> &'static [&'static str] {
        &[PACKET_TYPE_FIND_MY_PHONE]
    }

    fn on_packet(&self, _ctx: &PluginContext, _packet: &NetworkPacket) -> Result<bool> {
        Ok(false)
    }
}

/// Build the empty request that makes the remote device ring.
pub fn find_remote_packet() -> NetworkPacket {
    NetworkPacket::new(PACKET_TYPE_FIND_MY_PHONE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_remote_packet_uses_shared_type_and_empty_body() {
        let packet = find_remote_packet();
        assert_eq!(packet.ty, PACKET_TYPE_FIND_MY_PHONE);
        assert_eq!(packet.ty, "kdeconnect.findmyphone.request");
        assert!(packet.body.is_empty());

        let back = NetworkPacket::deserialize(&packet.serialize()).unwrap();
        assert_eq!(back.ty, "kdeconnect.findmyphone.request");
        assert!(back.body.is_empty());
    }

    #[test]
    fn plugin_has_no_incoming_types() {
        let plugin = FindRemoteDevicePlugin;
        assert!(plugin.supported_packet_types().is_empty());
        assert_eq!(plugin.outgoing_packet_types(), &[PACKET_TYPE_FIND_MY_PHONE]);
    }
}
