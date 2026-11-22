//! Presenter plugin (outgoing only).
//!
//! Sends presentation clicks as `kdeconnect.presenter` packets and moves the
//! pointer through `kdeconnect.mousepad.request`. The plugin itself has no
//! incoming packet types.

use kc_core::{Plugin, PluginContext, Result};
use kc_protocol::NetworkPacket;

const TYPE_PRESENTER: &str = "kdeconnect.presenter";
const TYPE_MOUSEPAD_REQUEST: &str = "kdeconnect.mousepad.request";

#[derive(Default, Debug)]
pub struct PresenterPlugin;

/// Send a presenter key (e.g. next/previous slide) with modifiers.
pub fn presenter_key_packet(
    key: i32,
    ctrl: bool,
    alt: bool,
    shift: bool,
    super_key: bool,
) -> NetworkPacket {
    let mut packet = NetworkPacket::new(TYPE_PRESENTER);
    packet.set_i64("key", key as i64);
    packet.set_bool("ctrl", ctrl);
    packet.set_bool("alt", alt);
    packet.set_bool("shift", shift);
    packet.set_bool("super", super_key);
    packet
}

/// Move the pointer by a relative delta while presenting.
pub fn presenter_pointer_packet(dx: f64, dy: f64) -> NetworkPacket {
    let mut packet = NetworkPacket::new(TYPE_MOUSEPAD_REQUEST);
    packet.set_f64("dx", dx);
    packet.set_f64("dy", dy);
    packet.set_bool("scroll", false);
    packet.set_bool("sendAck", false);
    packet
}

/// Scroll the presentation (wheel delta carried by `dx`/`dy`).
pub fn presenter_scroll_packet(dx: f64, dy: f64) -> NetworkPacket {
    let mut packet = presenter_pointer_packet(dx, dy);
    packet.set_bool("scroll", true);
    packet
}

impl Plugin for PresenterPlugin {
    fn key(&self) -> &'static str {
        "PresenterPlugin"
    }
    fn display_name(&self) -> &'static str {
        "PresenterPlugin"
    }
    fn description(&self) -> &'static str {
        "KDE Connect plugin"
    }
    fn supported_packet_types(&self) -> &'static [&'static str] {
        &[]
    }
    fn outgoing_packet_types(&self) -> &'static [&'static str] {
        &["kdeconnect.mousepad.request", "kdeconnect.presenter"]
    }

    fn on_packet(&self, _ctx: &PluginContext, _packet: &NetworkPacket) -> Result<bool> {
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presenter_key_packet_body_and_modifiers() {
        let packet = presenter_key_packet(33, false, true, false, true);
        assert_eq!(packet.ty, TYPE_PRESENTER);
        assert_eq!(packet.get_i64("key"), Some(33));
        assert_eq!(packet.get_bool("ctrl"), Some(false));
        assert_eq!(packet.get_bool("alt"), Some(true));
        assert_eq!(packet.get_bool("shift"), Some(false));
        assert_eq!(packet.get_bool("super"), Some(true));

        let back = NetworkPacket::deserialize(&packet.serialize()).unwrap();
        assert_eq!(back.get_i64("key"), Some(33));
        assert_eq!(back.get_bool("super"), Some(true));
    }

    #[test]
    fn pointer_and_scroll_packets_use_mousepad_type() {
        let pointer = presenter_pointer_packet(1.0, 2.0);
        assert_eq!(pointer.ty, TYPE_MOUSEPAD_REQUEST);
        assert_eq!(pointer.get_f64("dx"), Some(1.0));
        assert_eq!(pointer.get_bool("scroll"), Some(false));

        let scroll = presenter_scroll_packet(0.0, -4.0);
        assert_eq!(scroll.ty, TYPE_MOUSEPAD_REQUEST);
        assert_eq!(scroll.get_bool("scroll"), Some(true));
        assert_eq!(scroll.get_f64("dy"), Some(-4.0));
    }
}
