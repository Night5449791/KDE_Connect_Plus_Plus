//! Mouse input receiver (desktop-side touchpad sink).
//!
//! `Platform` has no mouse-specific callback, so this plugin deliberately
//! does not hijack `on_share`/`on_notification` for input events. Instead it
//! decodes the packet into [`MouseEvent`] (for the host to consume) and emits
//! a `tracing::debug!` record.

use kc_core::{Plugin, PluginContext, Result};
use kc_protocol::NetworkPacket;

const TYPE_REQUEST: &str = "kdeconnect.mousepad.request";

/// Decoded contents of a `kdeconnect.mousepad.request` packet.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MouseEvent {
    pub dx: f64,
    pub dy: f64,
    pub scroll: bool,
    pub button: Option<i32>,
    pub key: Option<i32>,
    pub singleclick: bool,
    pub doubleclick: bool,
    pub middleclick: bool,
    pub rightclick: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub super_key: bool,
    pub send_ack: bool,
}

/// Parse every field upstream may put in a mousepad request. Missing fields
/// fall back to neutral values (zero deltas, no button/key, no modifiers).
pub fn parse_mouse_packet(packet: &NetworkPacket) -> MouseEvent {
    MouseEvent {
        dx: packet.get_f64("dx").unwrap_or(0.0),
        dy: packet.get_f64("dy").unwrap_or(0.0),
        scroll: packet.get_bool_or("scroll", false),
        button: packet.get_i64("button").map(|v| v as i32),
        key: packet.get_i64("key").map(|v| v as i32),
        singleclick: packet.get_bool_or("singleclick", false),
        doubleclick: packet.get_bool_or("doubleclick", false),
        middleclick: packet.get_bool_or("middleclick", false),
        rightclick: packet.get_bool_or("rightclick", false),
        ctrl: packet.get_bool_or("ctrl", false),
        alt: packet.get_bool_or("alt", false),
        shift: packet.get_bool_or("shift", false),
        super_key: packet.get_bool_or("super", false),
        send_ack: packet.get_bool_or("sendAck", false),
    }
}

#[derive(Default, Debug)]
pub struct MouseReceiverPlugin;

impl Plugin for MouseReceiverPlugin {
    fn key(&self) -> &'static str {
        "MouseReceiverPlugin"
    }
    fn display_name(&self) -> &'static str {
        "MouseReceiverPlugin"
    }
    fn description(&self) -> &'static str {
        "KDE Connect plugin"
    }
    fn supported_packet_types(&self) -> &'static [&'static str] {
        &["kdeconnect.mousepad.request"]
    }
    fn outgoing_packet_types(&self) -> &'static [&'static str] {
        &[]
    }

    fn on_packet(&self, _ctx: &PluginContext, packet: &NetworkPacket) -> Result<bool> {
        if packet.ty != TYPE_REQUEST {
            return Ok(false);
        }
        let event = parse_mouse_packet(packet);
        tracing::debug!(?event, "received mousepad input event");
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_mousepad_request() {
        let mut packet = NetworkPacket::new(TYPE_REQUEST);
        packet.set_f64("dx", 2.5);
        packet.set_f64("dy", -1.0);
        packet.set_bool("scroll", true);
        packet.set_i64("button", 1);
        packet.set_i64("key", 40);
        packet.set_bool("rightclick", true);
        packet.set_bool("ctrl", true);
        packet.set_bool("shift", true);
        packet.set_bool("sendAck", true);

        let event = parse_mouse_packet(&packet);
        assert_eq!(event.dx, 2.5);
        assert_eq!(event.dy, -1.0);
        assert!(event.scroll);
        assert_eq!(event.button, Some(1));
        assert_eq!(event.key, Some(40));
        assert!(event.rightclick);
        assert!(!event.singleclick);
        assert!(event.ctrl && event.shift && !event.alt && !event.super_key);
        assert!(event.send_ack);
    }

    #[test]
    fn missing_fields_are_neutral() {
        let event = parse_mouse_packet(&NetworkPacket::new(TYPE_REQUEST));
        assert_eq!(event, MouseEvent::default());
    }

    #[test]
    fn body_roundtrip_preserves_parsed_values() {
        let mut packet = NetworkPacket::new(TYPE_REQUEST);
        packet.set_f64("dx", 7.0);
        packet.set_bool("middleclick", true);
        let back = NetworkPacket::deserialize(&packet.serialize()).unwrap();
        let event = parse_mouse_packet(&back);
        assert_eq!(event.dx, 7.0);
        assert!(event.middleclick);
    }
}
