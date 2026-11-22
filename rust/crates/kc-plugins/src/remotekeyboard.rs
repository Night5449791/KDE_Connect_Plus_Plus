//! Remote keyboard plugin (receives key events from the peer).
//!
//! Echoes every mousepad request back with `sent = true` so the sender can
//! stop waiting for an acknowledgement. When the request asks for one, the
//! host is also notified through [`kc_core::Platform::on_share`] as a generic input
//! hook (there is no keyboard-specific callback).

use kc_core::{Plugin, PluginContext, Result};
use kc_protocol::NetworkPacket;
use serde_json::Value;

const TYPE_REQUEST: &str = "kdeconnect.mousepad.request";
const TYPE_ECHO: &str = "kdeconnect.mousepad.echo";
const TYPE_KEYBOARD_STATE: &str = "kdeconnect.mousepad.keyboardstate";

/// Modifier flags decoded from a key packet.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KeyModifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub super_key: bool,
}

#[derive(Default, Debug)]
pub struct RemoteKeyboardPlugin;

pub fn parse_key(packet: &NetworkPacket) -> Option<i32> {
    packet.get_i64("key").map(|v| v as i32)
}

pub fn parse_modifiers(packet: &NetworkPacket) -> KeyModifiers {
    KeyModifiers {
        ctrl: packet.get_bool_or("ctrl", false),
        alt: packet.get_bool_or("alt", false),
        shift: packet.get_bool_or("shift", false),
        super_key: packet.get_bool_or("super", false),
    }
}

/// Copy all body fields of the originating request and mark it as sent.
pub fn echo_packet(original: &NetworkPacket) -> NetworkPacket {
    let mut packet = NetworkPacket::new(TYPE_ECHO);
    for (key, value) in &original.body {
        packet.body.insert(key.clone(), value.clone());
    }
    packet.set_bool("sent", true);
    packet
}

pub fn keyboard_state_packet(state: bool) -> NetworkPacket {
    let mut packet = NetworkPacket::new(TYPE_KEYBOARD_STATE);
    packet.set_bool("state", state);
    packet
}

/// Echo for a specific key plus explicit modifiers.
pub fn key_echo_packet(
    key: i32,
    modifiers: KeyModifiers,
    original: Option<&Value>,
) -> NetworkPacket {
    let mut body = serde_json::Map::new();
    body.insert("key".to_owned(), Value::from(key));
    body.insert("ctrl".to_owned(), Value::Bool(modifiers.ctrl));
    body.insert("alt".to_owned(), Value::Bool(modifiers.alt));
    body.insert("shift".to_owned(), Value::Bool(modifiers.shift));
    body.insert("super".to_owned(), Value::Bool(modifiers.super_key));
    body.insert("sent".to_owned(), Value::Bool(true));
    if let Some(original) = original {
        body.insert("originalMessage".to_owned(), original.clone());
    }
    NetworkPacket {
        ty: TYPE_ECHO.to_owned(),
        body,
        ..NetworkPacket::default()
    }
}

fn send(ctx: &PluginContext, packet: NetworkPacket) -> Result<()> {
    ctx.sender
        .send(kc_core::OutgoingPacket {
            device_id: ctx.device_id.clone(),
            packet,
        })
        .map_err(|_| kc_core::CoreError::Transport("outgoing channel closed".to_owned()))
}

impl Plugin for RemoteKeyboardPlugin {
    fn key(&self) -> &'static str {
        "RemoteKeyboardPlugin"
    }
    fn display_name(&self) -> &'static str {
        "RemoteKeyboardPlugin"
    }
    fn description(&self) -> &'static str {
        "KDE Connect plugin"
    }
    fn supported_packet_types(&self) -> &'static [&'static str] {
        &["kdeconnect.mousepad.request"]
    }
    fn outgoing_packet_types(&self) -> &'static [&'static str] {
        &[
            "kdeconnect.mousepad.echo",
            "kdeconnect.mousepad.keyboardstate",
        ]
    }

    fn on_packet(&self, ctx: &PluginContext, packet: &NetworkPacket) -> Result<bool> {
        match packet.ty.as_str() {
            TYPE_REQUEST => {
                if packet.get_bool_or("sendAck", false) {
                    ctx.platform.on_share(&ctx.device_id, packet);
                }
                send(ctx, echo_packet(packet))?;
                Ok(true)
            }
            TYPE_KEYBOARD_STATE => {
                let state = packet.get_bool("state").unwrap_or(false);
                send(ctx, keyboard_state_packet(state))?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key_request() -> NetworkPacket {
        let mut packet = NetworkPacket::new(TYPE_REQUEST);
        packet.set_i64("key", 40);
        packet.set_bool("ctrl", true);
        packet.set_bool("shift", true);
        packet.set_bool("sendAck", true);
        packet
    }

    #[test]
    fn key_and_modifiers_are_decoded() {
        let packet = key_request();
        assert_eq!(parse_key(&packet), Some(40));
        let modifiers = parse_modifiers(&packet);
        assert!(modifiers.ctrl && modifiers.shift);
        assert!(!modifiers.alt && !modifiers.super_key);
    }

    #[test]
    fn echo_preserves_body_and_sets_sent() {
        let packet = key_request();
        let echo = echo_packet(&packet);
        assert_eq!(echo.ty, TYPE_ECHO);
        assert_eq!(echo.get_i64("key"), Some(40));
        assert_eq!(echo.get_bool("ctrl"), Some(true));
        assert_eq!(echo.get_bool("sendAck"), Some(true));
        assert_eq!(echo.get_bool("sent"), Some(true));

        let back = NetworkPacket::deserialize(&echo.serialize()).unwrap();
        assert_eq!(back.get_i64("key"), Some(40));
        assert_eq!(back.get_bool("sent"), Some(true));
    }

    #[test]
    fn explicit_key_echo_and_keyboard_state() {
        let modifiers = KeyModifiers {
            super_key: true,
            ..KeyModifiers::default()
        };
        let echo = key_echo_packet(62, modifiers, None);
        assert_eq!(echo.get_i64("key"), Some(62));
        assert_eq!(echo.get_bool("super"), Some(true));
        assert_eq!(echo.get_bool("sent"), Some(true));

        let state = keyboard_state_packet(true);
        assert_eq!(state.ty, TYPE_KEYBOARD_STATE);
        assert_eq!(state.get_bool("state"), Some(true));
    }
}
