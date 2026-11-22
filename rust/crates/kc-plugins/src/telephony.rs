//! `TelephonyPlugin`: share call state with paired devices and mute a
//! ringing remote device.
//!
//! Outgoing `kdeconnect.telephony` body: `event` ("ringing" | "talking" |
//! "missedCall"), optional `phoneNumber`, `contactName`, `phoneThumbnail`
//! (base64). Cancellation sets `isCancel` to the *string* "true", as upstream.
//! Incoming `kdeconnect.telephony.request_mute` has an empty body.

use kc_core::{Platform, Plugin, PluginContext, Result};
use kc_protocol::NetworkPacket;

pub const PACKET_TYPE_TELEPHONY: &str = "kdeconnect.telephony";
pub const PACKET_TYPE_REQUEST_MUTE: &str = "kdeconnect.telephony.request_mute";

pub const EVENT_RINGING: &str = "ringing";
pub const EVENT_TALKING: &str = "talking";
pub const EVENT_MISSED_CALL: &str = "missedCall";

#[derive(Default, Debug)]
pub struct TelephonyPlugin;

impl TelephonyPlugin {
    pub(crate) fn handle(packet: &NetworkPacket, device_id: &str, platform: &dyn Platform) -> bool {
        if packet.ty != PACKET_TYPE_REQUEST_MUTE {
            return false;
        }
        platform.on_telephony(device_id, packet);
        true
    }
}

impl Plugin for TelephonyPlugin {
    fn key(&self) -> &'static str {
        "TelephonyPlugin"
    }
    fn display_name(&self) -> &'static str {
        "Telephony"
    }
    fn description(&self) -> &'static str {
        "Share call state and mute ringing devices"
    }
    fn supported_packet_types(&self) -> &'static [&'static str] {
        &[PACKET_TYPE_REQUEST_MUTE]
    }
    fn outgoing_packet_types(&self) -> &'static [&'static str] {
        &[PACKET_TYPE_TELEPHONY]
    }

    fn on_packet(&self, ctx: &PluginContext, packet: &NetworkPacket) -> Result<bool> {
        Ok(Self::handle(packet, &ctx.device_id, ctx.platform.as_ref()))
    }
}

/// Build a telephony event packet, optionally including a base64 contact thumbnail.
pub fn telephony_packet_with_thumbnail(
    event: &str,
    phone_number: Option<&str>,
    contact_name: Option<&str>,
    phone_thumbnail: Option<&str>,
) -> NetworkPacket {
    let mut packet = NetworkPacket::new(PACKET_TYPE_TELEPHONY);
    packet.set_str("event", event);
    if let Some(number) = phone_number {
        packet.set_str("phoneNumber", number);
    }
    if let Some(name) = contact_name {
        packet.set_str("contactName", name);
    }
    if let Some(thumbnail) = phone_thumbnail {
        packet.set_str("phoneThumbnail", thumbnail);
    }
    packet
}

/// Build a telephony event packet without a contact thumbnail.
pub fn telephony_packet(
    event: &str,
    phone_number: Option<&str>,
    contact_name: Option<&str>,
) -> NetworkPacket {
    telephony_packet_with_thumbnail(event, phone_number, contact_name, None)
}

/// Cancel a previously announced telephony event (`isCancel` = "true").
pub fn cancel_telephony_packet_with_thumbnail(
    event: &str,
    phone_number: Option<&str>,
    contact_name: Option<&str>,
    phone_thumbnail: Option<&str>,
) -> NetworkPacket {
    let mut packet =
        telephony_packet_with_thumbnail(event, phone_number, contact_name, phone_thumbnail);
    packet.set_str("isCancel", "true");
    packet
}

/// Cancel a telephony event without a contact thumbnail.
pub fn cancel_telephony_packet(
    event: &str,
    phone_number: Option<&str>,
    contact_name: Option<&str>,
) -> NetworkPacket {
    cancel_telephony_packet_with_thumbnail(event, phone_number, contact_name, None)
}

/// The `event` field of a telephony packet.
pub fn telephony_event(packet: &NetworkPacket) -> Option<&str> {
    packet.get_str("event")
}

/// Whether the packet cancels an earlier event. Upstream sends the *string*
/// "true", not a JSON boolean.
pub fn is_cancel(packet: &NetworkPacket) -> bool {
    packet.get_str("isCancel") == Some("true")
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    use kc_protocol::DeviceType;

    use super::*;

    #[derive(Default)]
    struct RecordingPlatform {
        telephony: AtomicUsize,
        last_type: Mutex<String>,
    }

    impl Platform for RecordingPlatform {
        fn device_name(&self) -> String {
            "test".to_owned()
        }
        fn device_type(&self) -> DeviceType {
            DeviceType::Phone
        }

        fn on_telephony(&self, _device_id: &str, packet: &NetworkPacket) {
            self.telephony.fetch_add(1, Ordering::SeqCst);
            *self.last_type.lock().unwrap() = packet.ty.clone();
        }
    }

    #[test]
    fn telephony_packet_roundtrips_through_wire() {
        let packet = telephony_packet(EVENT_RINGING, Some("+123456789"), Some("Alice"));
        let back = NetworkPacket::deserialize(&packet.serialize()).unwrap();
        assert_eq!(back.ty, PACKET_TYPE_TELEPHONY);
        assert_eq!(telephony_event(&back), Some(EVENT_RINGING));
        assert_eq!(
            back.get_string("phoneNumber").as_deref(),
            Some("+123456789")
        );
        assert_eq!(back.get_string("contactName").as_deref(), Some("Alice"));
        assert_eq!(back.get_str("phoneThumbnail"), None);
        assert!(!is_cancel(&back));
    }

    #[test]
    fn thumbnail_and_optional_fields_roundtrip() {
        let packet =
            telephony_packet_with_thumbnail(EVENT_MISSED_CALL, None, Some("Bob"), Some("YWJjZA=="));
        let back = NetworkPacket::deserialize(&packet.serialize()).unwrap();
        assert_eq!(
            back.get_string("phoneThumbnail").as_deref(),
            Some("YWJjZA==")
        );
        assert_eq!(back.get_str("phoneNumber"), None);
        assert_eq!(telephony_event(&back), Some(EVENT_MISSED_CALL));
    }

    #[test]
    fn cancellation_uses_string_true() {
        let packet = cancel_telephony_packet(EVENT_TALKING, Some("+1"), None);
        let back = NetworkPacket::deserialize(&packet.serialize()).unwrap();
        assert_eq!(back.get_str("isCancel"), Some("true"));
        assert_eq!(back.get_bool("isCancel"), None);
        assert!(is_cancel(&back));
        assert_eq!(telephony_event(&back), Some(EVENT_TALKING));
    }

    #[test]
    fn handle_forwards_request_mute_to_host() {
        let platform = RecordingPlatform::default();
        let mute = NetworkPacket::new(PACKET_TYPE_REQUEST_MUTE);
        assert!(TelephonyPlugin::handle(&mute, "dev", &platform));
        assert_eq!(platform.telephony.load(Ordering::SeqCst), 1);
        assert_eq!(
            *platform.last_type.lock().unwrap(),
            PACKET_TYPE_REQUEST_MUTE
        );

        let other = telephony_packet(EVENT_RINGING, None, None);
        assert!(!TelephonyPlugin::handle(&other, "dev", &platform));
        assert_eq!(platform.telephony.load(Ordering::SeqCst), 1);
    }
}
