//! Digitizer plugin: outgoing pen/touch input packets. There are no incoming
//! packet types for this plugin upstream.

use kc_core::{Plugin, PluginContext, Result};
use kc_protocol::NetworkPacket;

const DIGITIZER: &str = "kdeconnect.digitizer";
const DIGITIZER_SESSION: &str = "kdeconnect.digitizer.session";

/// One digitizer input sample (screen coordinates and pressure).
pub fn digitizer_packet(x: f32, y: f32, pressure: f32) -> NetworkPacket {
    let mut packet = NetworkPacket::new(DIGITIZER);
    packet.set_f64("x", x as f64);
    packet.set_f64("y", y as f64);
    packet.set_f64("pressure", pressure as f64);
    packet
}

/// Start or stop a digitizer session.
pub fn digitizer_session_packet(start: bool) -> NetworkPacket {
    let mut packet = NetworkPacket::new(DIGITIZER_SESSION);
    packet.set_bool("start", start);
    packet
}

/// Start/stop a session and tag it with an id (for multiple concurrent pens).
pub fn digitizer_session_packet_with_id(start: bool, session_id: i64) -> NetworkPacket {
    let mut packet = digitizer_session_packet(start);
    packet.set_i64("sessionId", session_id);
    packet
}

#[derive(Default, Debug)]
pub struct DigitizerPlugin;

impl Plugin for DigitizerPlugin {
    fn key(&self) -> &'static str {
        "DigitizerPlugin"
    }
    fn display_name(&self) -> &'static str {
        "Digitizer"
    }
    fn description(&self) -> &'static str {
        "Send pen and touch input to a paired device"
    }
    fn supported_packet_types(&self) -> &'static [&'static str] {
        &[]
    }
    fn outgoing_packet_types(&self) -> &'static [&'static str] {
        &[DIGITIZER_SESSION, DIGITIZER]
    }

    fn on_packet(&self, _ctx: &PluginContext, _packet: &NetworkPacket) -> Result<bool> {
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digitizer_packet_roundtrips() {
        let packet = digitizer_packet(10.5, 20.25, 0.75);
        assert_eq!(packet.ty, DIGITIZER);
        let back = NetworkPacket::deserialize(&packet.serialize()).unwrap();
        assert_eq!(back.get_f64("x"), Some(10.5));
        assert_eq!(back.get_f64("y"), Some(20.25));
        assert_eq!(back.get_f64("pressure"), Some(0.75));
    }

    #[test]
    fn session_packet_roundtrips() {
        let start = digitizer_session_packet(true);
        let back = NetworkPacket::deserialize(&start.serialize()).unwrap();
        assert_eq!(back.ty, DIGITIZER_SESSION);
        assert_eq!(back.get_bool("start"), Some(true));
        assert!(back.get_i64("sessionId").is_none());

        let stop = digitizer_session_packet_with_id(false, 7);
        let back = NetworkPacket::deserialize(&stop.serialize()).unwrap();
        assert_eq!(back.get_bool("start"), Some(false));
        assert_eq!(back.get_i64("sessionId"), Some(7));
    }

    #[test]
    fn plugin_declares_no_incoming_types() {
        let plugin = DigitizerPlugin;
        assert!(plugin.supported_packet_types().is_empty());
        assert_eq!(
            plugin.outgoing_packet_types(),
            &[DIGITIZER_SESSION, DIGITIZER]
        );
    }
}
