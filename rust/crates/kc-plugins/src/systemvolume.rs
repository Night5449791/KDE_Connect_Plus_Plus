//! `SystemVolumePlugin`: list and control the audio sinks of a paired device.
//!
//! Incoming `kdeconnect.systemvolume` packets carry either a `sinkList` JSON
//! array of sinks, or a single `name` with optional `volume`, `muted` and
//! `enabled` fields. Outgoing control packets use
//! `kdeconnect.systemvolume.request`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use kc_core::{Platform, Plugin, PluginContext, Result};
use kc_protocol::NetworkPacket;

pub const PACKET_TYPE_SYSTEM_VOLUME: &str = "kdeconnect.systemvolume";
pub const PACKET_TYPE_REQUEST: &str = "kdeconnect.systemvolume.request";

/// A single audio sink as reported by the remote device.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Sink {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub volume: i32,
    #[serde(default)]
    pub muted: bool,
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Default, Debug)]
pub struct SystemVolumePlugin;

impl SystemVolumePlugin {
    /// Parse the `sinkList` array of a full status update.
    pub fn parse_sink_list(packet: &NetworkPacket) -> Option<Vec<Sink>> {
        let array = packet.get_array("sinkList")?;
        serde_json::from_value(Value::Array(array.clone())).ok()
    }

    /// Parse a single-sink update (`name` plus optional fields).
    pub fn parse_single_sink(packet: &NetworkPacket) -> Option<Sink> {
        let name = packet.get_str("name")?;
        let mut sink = Sink {
            name: name.to_owned(),
            ..Sink::default()
        };
        if let Some(volume) = packet.get_i64("volume") {
            sink.volume = volume as i32;
        }
        if let Some(muted) = packet.get_bool("muted") {
            sink.muted = muted;
        }
        if let Some(enabled) = packet.get_bool("enabled") {
            sink.enabled = enabled;
        }
        Some(sink)
    }

    pub(crate) fn handle(packet: &NetworkPacket, device_id: &str, platform: &dyn Platform) -> bool {
        if packet.ty != PACKET_TYPE_SYSTEM_VOLUME {
            return false;
        }
        if Self::parse_sink_list(packet).is_none() && Self::parse_single_sink(packet).is_none() {
            return false;
        }
        platform.on_system_volume(device_id, packet);
        true
    }
}

impl Plugin for SystemVolumePlugin {
    fn key(&self) -> &'static str {
        "SystemVolumePlugin"
    }
    fn display_name(&self) -> &'static str {
        "System volume"
    }
    fn description(&self) -> &'static str {
        "List and control the audio sinks of a paired device"
    }
    fn supported_packet_types(&self) -> &'static [&'static str] {
        &[PACKET_TYPE_SYSTEM_VOLUME]
    }
    fn outgoing_packet_types(&self) -> &'static [&'static str] {
        &[PACKET_TYPE_REQUEST]
    }

    fn on_packet(&self, ctx: &PluginContext, packet: &NetworkPacket) -> Result<bool> {
        Ok(Self::handle(packet, &ctx.device_id, ctx.platform.as_ref()))
    }
}

/// Ask the remote device for its list of sinks.
pub fn request_sinks_packet() -> NetworkPacket {
    NetworkPacket::new(PACKET_TYPE_REQUEST)
}

/// Set the volume of the named sink.
pub fn set_volume_packet(name: &str, volume: i32) -> NetworkPacket {
    let mut packet = NetworkPacket::new(PACKET_TYPE_REQUEST);
    packet.set_str("name", name);
    packet.set_i64("volume", volume as i64);
    packet
}

/// Mute or unmute the named sink.
pub fn set_muted_packet(name: &str, muted: bool) -> NetworkPacket {
    let mut packet = NetworkPacket::new(PACKET_TYPE_REQUEST);
    packet.set_str("name", name);
    packet.set_bool("muted", muted);
    packet
}

/// Enable or disable the named sink.
pub fn set_enabled_packet(name: &str, enabled: bool) -> NetworkPacket {
    let mut packet = NetworkPacket::new(PACKET_TYPE_REQUEST);
    packet.set_str("name", name);
    packet.set_bool("enabled", enabled);
    packet
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use kc_protocol::DeviceType;

    use super::*;

    #[derive(Default)]
    struct RecordingPlatform {
        volumes: AtomicUsize,
    }

    impl Platform for RecordingPlatform {
        fn device_name(&self) -> String {
            "test".to_owned()
        }
        fn device_type(&self) -> DeviceType {
            DeviceType::Desktop
        }

        fn on_system_volume(&self, _device_id: &str, _packet: &NetworkPacket) {
            self.volumes.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn sample_sink() -> Sink {
        Sink {
            name: "alsa_output.pci".to_owned(),
            description: "Built-in audio".to_owned(),
            volume: 42,
            muted: false,
            enabled: true,
        }
    }

    #[test]
    fn sink_roundtrips_as_json_object() {
        let value = serde_json::to_value(sample_sink()).unwrap();
        assert_eq!(value["name"], "alsa_output.pci");
        assert_eq!(value["volume"], 42);
        assert_eq!(value["muted"], false);
        assert_eq!(value["enabled"], true);

        let back: Sink = serde_json::from_value(value).unwrap();
        assert_eq!(back, sample_sink());
    }

    #[test]
    fn sink_list_and_single_sink_parse_from_wire() {
        let mut full = NetworkPacket::new(PACKET_TYPE_SYSTEM_VOLUME);
        full.set("sinkList", serde_json::json!([sample_sink()]));
        let back = NetworkPacket::deserialize(&full.serialize()).unwrap();
        assert_eq!(
            SystemVolumePlugin::parse_sink_list(&back),
            Some(vec![sample_sink()])
        );

        let mut single = NetworkPacket::new(PACKET_TYPE_SYSTEM_VOLUME);
        single.set_str("name", "sink-1");
        single.set_i64("volume", 88);
        single.set_bool("muted", true);
        single.set_bool("enabled", false);
        let back = NetworkPacket::deserialize(&single.serialize()).unwrap();
        assert_eq!(
            SystemVolumePlugin::parse_single_sink(&back),
            Some(Sink {
                name: "sink-1".to_owned(),
                volume: 88,
                muted: true,
                enabled: false,
                ..Sink::default()
            })
        );
    }

    #[test]
    fn request_packets_roundtrip_through_wire() {
        let cases = [
            (request_sinks_packet(), None, None),
            (set_volume_packet("sink-1", 55), Some(55), None),
            (set_muted_packet("sink-1", true), None, Some(true)),
            (set_enabled_packet("sink-1", false), None, None),
        ];
        for (packet, expected_volume, expected_muted) in cases {
            let back = NetworkPacket::deserialize(&packet.serialize()).unwrap();
            assert_eq!(back.ty, PACKET_TYPE_REQUEST);
            if expected_volume.is_some() || expected_muted.is_some() {
                assert_eq!(back.get_string("name").as_deref(), Some("sink-1"));
            }
            if let Some(volume) = expected_volume {
                assert_eq!(back.get_i64("volume"), Some(volume));
            }
            if let Some(muted) = expected_muted {
                assert_eq!(back.get_bool("muted"), Some(muted));
            }
        }
        assert_eq!(
            set_enabled_packet("sink-1", false).get_bool("enabled"),
            Some(false)
        );
        assert!(request_sinks_packet().body.is_empty());
    }

    #[test]
    fn handle_only_consumes_well_formed_updates() {
        let platform = RecordingPlatform::default();
        let mut full = NetworkPacket::new(PACKET_TYPE_SYSTEM_VOLUME);
        full.set("sinkList", serde_json::json!([sample_sink()]));
        assert!(SystemVolumePlugin::handle(&full, "dev", &platform));

        let mut single = NetworkPacket::new(PACKET_TYPE_SYSTEM_VOLUME);
        single.set_str("name", "sink-1");
        assert!(SystemVolumePlugin::handle(&single, "dev", &platform));
        assert_eq!(platform.volumes.load(Ordering::SeqCst), 2);

        let malformed = NetworkPacket::new(PACKET_TYPE_SYSTEM_VOLUME);
        assert!(!SystemVolumePlugin::handle(&malformed, "dev", &platform));
        assert!(!SystemVolumePlugin::handle(
            &request_sinks_packet(),
            "dev",
            &platform
        ));
        assert_eq!(platform.volumes.load(Ordering::SeqCst), 2);
    }
}
