//! Virtual touchpad plugin (sender side).
//!
//! Builds `kdeconnect.mousepad.request` packets for pointer movement,
//! scrolling, clicks and key presses, and receives the peer's keyboard state
//! into a mutex so the host can query whether an on-screen keyboard is open.

use std::sync::Mutex;

use kc_core::{Plugin, PluginContext, Result};
use kc_protocol::NetworkPacket;

const TYPE_REQUEST: &str = "kdeconnect.mousepad.request";
const TYPE_KEYBOARD_STATE: &str = "kdeconnect.mousepad.keyboardstate";

/// Numeric button values used by the upstream mousepad protocol.
pub const BUTTON_LEFT: i32 = 1;
pub const BUTTON_RIGHT: i32 = 2;
pub const BUTTON_MIDDLE: i32 = 3;

/// Modifier flags carried alongside a key press.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub super_key: bool,
}

#[derive(Default, Debug)]
pub struct MousePadPlugin {
    keyboard_state: Mutex<bool>,
}

impl MousePadPlugin {
    /// Last keyboard-state value advertised by the peer (false initially).
    pub fn keyboard_state(&self) -> bool {
        *self
            .keyboard_state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }
}

/// Move the pointer by a relative delta.
pub fn move_packet(dx: f64, dy: f64) -> NetworkPacket {
    let mut packet = NetworkPacket::new(TYPE_REQUEST);
    packet.set_f64("dx", dx);
    packet.set_f64("dy", dy);
    packet.set_bool("scroll", false);
    packet.set_bool("sendAck", false);
    packet
}

/// Scroll by a relative delta (carried in `dx`/`dy` with `scroll` set).
pub fn scroll_packet(dx: f64, dy: f64) -> NetworkPacket {
    let mut packet = move_packet(dx, dy);
    packet.set_bool("scroll", true);
    packet
}

/// Single click with the given button.
pub fn click_packet(button: i32) -> NetworkPacket {
    let mut packet = NetworkPacket::new(TYPE_REQUEST);
    packet.set_i64("button", button as i64);
    packet.set_bool("singleclick", true);
    packet.set_bool("scroll", false);
    packet.set_bool("sendAck", true);
    packet
}

pub fn double_click_packet(button: i32) -> NetworkPacket {
    let mut packet = click_packet(button);
    packet.body.remove("singleclick");
    packet.set_bool("doubleclick", true);
    packet
}

pub fn middle_click_packet() -> NetworkPacket {
    let mut packet = click_packet(BUTTON_MIDDLE);
    packet.body.remove("singleclick");
    packet.set_bool("middleclick", true);
    packet
}

pub fn right_click_packet() -> NetworkPacket {
    let mut packet = click_packet(BUTTON_RIGHT);
    packet.body.remove("singleclick");
    packet.set_bool("rightclick", true);
    packet
}

/// Send a key code with optional modifiers.
pub fn key_packet(key: i32, modifiers: Modifiers) -> NetworkPacket {
    let mut packet = NetworkPacket::new(TYPE_REQUEST);
    packet.set_i64("key", key as i64);
    packet.set_bool("ctrl", modifiers.ctrl);
    packet.set_bool("alt", modifiers.alt);
    packet.set_bool("shift", modifiers.shift);
    packet.set_bool("super", modifiers.super_key);
    packet.set_bool("scroll", false);
    packet.set_bool("sendAck", true);
    packet
}

/// Advertise the local on-screen keyboard state to the peer.
pub fn keyboard_state_packet(state: bool) -> NetworkPacket {
    let mut packet = NetworkPacket::new(TYPE_KEYBOARD_STATE);
    packet.set_bool("state", state);
    packet
}

pub fn parse_keyboard_state(packet: &NetworkPacket) -> Option<bool> {
    if packet.ty == TYPE_KEYBOARD_STATE {
        packet.get_bool("state")
    } else {
        None
    }
}

impl Plugin for MousePadPlugin {
    fn key(&self) -> &'static str {
        "MousePadPlugin"
    }
    fn display_name(&self) -> &'static str {
        "MousePadPlugin"
    }
    fn description(&self) -> &'static str {
        "KDE Connect plugin"
    }
    fn supported_packet_types(&self) -> &'static [&'static str] {
        &["kdeconnect.mousepad.keyboardstate"]
    }
    fn outgoing_packet_types(&self) -> &'static [&'static str] {
        &["kdeconnect.mousepad.request"]
    }

    fn on_packet(&self, _ctx: &PluginContext, packet: &NetworkPacket) -> Result<bool> {
        if packet.ty != TYPE_KEYBOARD_STATE {
            return Ok(false);
        }
        if let Some(state) = parse_keyboard_state(packet) {
            *self
                .keyboard_state
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = state;
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn move_and_scroll_packets() {
        let movement = move_packet(1.5, -2.0);
        assert_eq!(movement.ty, TYPE_REQUEST);
        assert_eq!(movement.get_f64("dx"), Some(1.5));
        assert_eq!(movement.get_f64("dy"), Some(-2.0));
        assert_eq!(movement.get_bool("scroll"), Some(false));

        let scroll = scroll_packet(0.0, 3.0);
        assert_eq!(scroll.get_bool("scroll"), Some(true));
        assert_eq!(scroll.get_f64("dy"), Some(3.0));
    }

    #[test]
    fn click_variants_set_the_expected_flag() {
        assert_eq!(
            click_packet(BUTTON_LEFT).get_bool("singleclick"),
            Some(true)
        );
        assert_eq!(
            double_click_packet(BUTTON_LEFT).get_bool("doubleclick"),
            Some(true)
        );
        assert_eq!(middle_click_packet().get_bool("middleclick"), Some(true));
        assert_eq!(right_click_packet().get_bool("rightclick"), Some(true));
        assert_eq!(
            right_click_packet().get_i64("button"),
            Some(BUTTON_RIGHT as i64)
        );
    }

    #[test]
    fn key_packet_modifiers_roundtrip() {
        let modifiers = Modifiers {
            ctrl: true,
            alt: false,
            shift: true,
            super_key: true,
        };
        let packet = key_packet(40, modifiers);
        assert_eq!(packet.ty, TYPE_REQUEST);
        assert_eq!(packet.get_i64("key"), Some(40));
        assert_eq!(packet.get_bool("ctrl"), Some(true));
        assert_eq!(packet.get_bool("alt"), Some(false));
        assert_eq!(packet.get_bool("shift"), Some(true));
        assert_eq!(packet.get_bool("super"), Some(true));

        let back = NetworkPacket::deserialize(&packet.serialize()).unwrap();
        assert_eq!(back.get_i64("key"), Some(40));
        assert_eq!(back.get_bool("super"), Some(true));
    }

    #[test]
    fn keyboard_state_packet_and_parse() {
        let packet = keyboard_state_packet(true);
        assert_eq!(packet.ty, TYPE_KEYBOARD_STATE);
        assert_eq!(parse_keyboard_state(&packet), Some(true));
        assert_eq!(parse_keyboard_state(&move_packet(0.0, 0.0)), None);
    }
}
