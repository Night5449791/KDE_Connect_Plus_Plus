//! Remote keyboard plugin — IME variant.
//!
//! Wire behaviour is the same as `remotekeyboard`: echo each
//! `kdeconnect.mousepad.request` with `sent = true`, notify the host through
//! the generic [`kc_core::Platform::on_share`] hook when `sendAck` is requested, and
//! mirror keyboard-state packets. Kept independent so the IME plugin can
//! evolve its own filtering without touching the base plugin.

use kc_core::{Plugin, PluginContext, Result};
use kc_protocol::NetworkPacket;
use serde_json::Value;

const TYPE_REQUEST: &str = "kdeconnect.mousepad.request";
const TYPE_ECHO: &str = "kdeconnect.mousepad.echo";
const TYPE_KEYBOARD_STATE: &str = "kdeconnect.mousepad.keyboardstate";

/// Modifier flags decoded from an IME key packet.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ImeModifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub super_key: bool,
}

#[derive(Default, Debug)]
pub struct RemoteKeyboardIMEPlugin;

pub fn parse_ime_key(packet: &NetworkPacket) -> Option<i32> {
    packet.get_i64("key").map(|v| v as i32)
}

pub fn parse_ime_modifiers(packet: &NetworkPacket) -> ImeModifiers {
    ImeModifiers {
        ctrl: packet.get_bool_or("ctrl", false),
        alt: packet.get_bool_or("alt", false),
        shift: packet.get_bool_or("shift", false),
        super_key: packet.get_bool_or("super", false),
    }
}

/// Build an echo with all original fields plus `sent = true`.
pub fn ime_echo_packet(original: &NetworkPacket) -> NetworkPacket {
    let mut packet = NetworkPacket::new(TYPE_ECHO);
    for (key, value) in &original.body {
        packet.body.insert(key.clone(), value.clone());
    }
    packet.set_bool("sent", true);
    packet
}

pub fn ime_keyboard_state_packet(state: bool) -> NetworkPacket {
    let mut packet = NetworkPacket::new(TYPE_KEYBOARD_STATE);
    packet.set_bool("state", state);
    packet
}

/// Echo for a specific key and modifiers supplied by the IME.
pub fn ime_key_echo_packet(
    key: i32,
    modifiers: ImeModifiers,
    extra: Option<&Value>,
) -> NetworkPacket {
    let mut body = serde_json::Map::new();
    body.insert("key".to_owned(), Value::from(key));
    body.insert("ctrl".to_owned(), Value::Bool(modifiers.ctrl));
    body.insert("alt".to_owned(), Value::Bool(modifiers.alt));
    body.insert("shift".to_owned(), Value::Bool(modifiers.shift));
    body.insert("super".to_owned(), Value::Bool(modifiers.super_key));
    body.insert("sent".to_owned(), Value::Bool(true));
    if let Some(extra) = extra {
        body.insert("originalMessage".to_owned(), extra.clone());
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

impl Plugin for RemoteKeyboardIMEPlugin {
    fn key(&self) -> &'static str {
        "RemoteKeyboardIMEPlugin"
    }
    fn display_name(&self) -> &'static str {
        "RemoteKeyboardIMEPlugin"
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
                send(ctx, ime_echo_packet(packet))?;
                Ok(true)
            }
            TYPE_KEYBOARD_STATE => {
                let state = packet.get_bool("state").unwrap_or(false);
                send(ctx, ime_keyboard_state_packet(state))?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ime_request() -> NetworkPacket {
        let mut packet = NetworkPacket::new(TYPE_REQUEST);
        packet.set_i64("key", 66);
        packet.set_bool("alt", true);
        packet.set_bool("sendAck", true);
        packet
    }

    #[test]
    fn ime_key_and_modifiers_are_decoded() {
        let packet = ime_request();
        assert_eq!(parse_ime_key(&packet), Some(66));
        let modifiers = parse_ime_modifiers(&packet);
        assert!(modifiers.alt);
        assert!(!modifiers.ctrl && !modifiers.shift && !modifiers.super_key);
    }

    #[test]
    fn ime_echo_preserves_body_and_sets_sent() {
        let echo = ime_echo_packet(&ime_request());
        assert_eq!(echo.ty, TYPE_ECHO);
        assert_eq!(echo.get_i64("key"), Some(66));
        assert_eq!(echo.get_bool("alt"), Some(true));
        assert_eq!(echo.get_bool("sent"), Some(true));
        let back = NetworkPacket::deserialize(&echo.serialize()).unwrap();
        assert_eq!(back.get_bool("sent"), Some(true));
    }

    #[test]
    fn explicit_ime_echo_and_keyboard_state() {
        let modifiers = ImeModifiers {
            ctrl: true,
            ..ImeModifiers::default()
        };
        let echo = ime_key_echo_packet(9, modifiers, None);
        assert_eq!(echo.get_i64("key"), Some(9));
        assert_eq!(echo.get_bool("ctrl"), Some(true));
        assert_eq!(echo.get_bool("sent"), Some(true));

        let state = ime_keyboard_state_packet(false);
        assert_eq!(state.ty, TYPE_KEYBOARD_STATE);
        assert_eq!(state.get_bool("state"), Some(false));
    }
}
