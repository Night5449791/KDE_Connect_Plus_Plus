//! Input-device sharing receiver.
//!
//! Decodes inbound mousepad events into [`InputEvent`] for the host to
//! consume, and answers `kdeconnect.shareinputdevices.request` with the local
//! list of input devices. As with the other input plugins there is no
//! dedicated `Platform` callback, so events are only logged and exposed via
//! the public parser.

use std::sync::Mutex;

use kc_core::{Plugin, PluginContext, Result};
use kc_protocol::NetworkPacket;

const TYPE_MOUSE_REQUEST: &str = "kdeconnect.mousepad.request";
const TYPE_SHARE_REQUEST: &str = "kdeconnect.shareinputdevices.request";
const TYPE_SHARE: &str = "kdeconnect.shareinputdevices";

/// A decoded input event coming from the remote device.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct InputEvent {
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

#[derive(Default, Debug)]
pub struct InputDevicesReceiver {
    input_devices: Mutex<Vec<String>>,
}

impl InputDevicesReceiver {
    /// Replace the device list advertised in share responses.
    pub fn set_input_devices(&self, devices: Vec<String>) {
        *self.input_devices.lock().unwrap_or_else(|e| e.into_inner()) = devices;
    }

    pub fn input_devices(&self) -> Vec<String> {
        self.input_devices
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

pub fn parse_input_event(packet: &NetworkPacket) -> InputEvent {
    InputEvent {
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

/// Ask the peer whether it wants to share input devices.
pub fn share_input_devices_request(share: bool) -> NetworkPacket {
    let mut packet = NetworkPacket::new(TYPE_SHARE_REQUEST);
    packet.set_bool("share", share);
    packet
}

/// Answer with the sharing flag and the local list of input devices.
pub fn share_input_devices_response(share: bool, devices: &[String]) -> NetworkPacket {
    let mut packet = NetworkPacket::new(TYPE_SHARE);
    packet.set_bool("share", share);
    packet.set("inputDevices", devices.to_vec());
    packet
}

pub fn parse_share_request(packet: &NetworkPacket) -> Option<bool> {
    if packet.ty == TYPE_SHARE_REQUEST {
        packet.get_bool("share")
    } else {
        None
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

impl Plugin for InputDevicesReceiver {
    fn key(&self) -> &'static str {
        "InputDevicesReceiver"
    }
    fn display_name(&self) -> &'static str {
        "InputDevicesReceiver"
    }
    fn description(&self) -> &'static str {
        "KDE Connect plugin"
    }
    fn supported_packet_types(&self) -> &'static [&'static str] {
        &[
            "kdeconnect.mousepad.request",
            "kdeconnect.shareinputdevices.request",
        ]
    }
    fn outgoing_packet_types(&self) -> &'static [&'static str] {
        &["kdeconnect.shareinputdevices"]
    }

    fn on_packet(&self, ctx: &PluginContext, packet: &NetworkPacket) -> Result<bool> {
        match packet.ty.as_str() {
            TYPE_MOUSE_REQUEST => {
                let event = parse_input_event(packet);
                tracing::debug!(?event, "received input device event");
                Ok(true)
            }
            TYPE_SHARE_REQUEST => {
                let share = parse_share_request(packet).unwrap_or(false);
                let response = share_input_devices_response(share, &self.input_devices());
                send(ctx, response)?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_input_event_with_modifiers() {
        let mut packet = NetworkPacket::new(TYPE_MOUSE_REQUEST);
        packet.set_f64("dx", -3.0);
        packet.set_i64("key", 40);
        packet.set_bool("ctrl", true);
        packet.set_bool("super", true);
        packet.set_bool("sendAck", true);

        let event = parse_input_event(&packet);
        assert_eq!(event.dx, -3.0);
        assert_eq!(event.key, Some(40));
        assert!(event.ctrl && event.super_key);
        assert!(event.send_ack);
        assert_eq!(event.button, None);
    }

    #[test]
    fn share_request_and_response_roundtrip() {
        let request = share_input_devices_request(true);
        assert_eq!(request.ty, TYPE_SHARE_REQUEST);
        assert_eq!(request.get_bool("share"), Some(true));
        assert_eq!(parse_share_request(&request), Some(true));

        let devices = vec!["mouse".to_owned(), "keyboard".to_owned()];
        let response = share_input_devices_response(true, &devices);
        assert_eq!(response.ty, TYPE_SHARE);
        let back = NetworkPacket::deserialize(&response.serialize()).unwrap();
        assert_eq!(back.get_bool("share"), Some(true));
        assert_eq!(
            back.get_string_list("inputDevices"),
            Some(vec!["mouse".to_owned(), "keyboard".to_owned()])
        );
    }

    #[test]
    fn device_cache_defaults_to_empty() {
        let plugin = InputDevicesReceiver::default();
        assert!(plugin.input_devices().is_empty());
        plugin.set_input_devices(vec!["pen".to_owned()]);
        assert_eq!(plugin.input_devices(), vec!["pen".to_owned()]);
    }
}
