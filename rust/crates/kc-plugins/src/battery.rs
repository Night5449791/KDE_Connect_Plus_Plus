//! `BatteryPlugin`: report a paired device's battery state to the host.
//!
//! Wire body (upstream `kdeconnect.battery`):
//! * `currentCharge` (Int) — percentage, 0..=100
//! * `isCharging` (Bool)
//! * `thresholdEvent` (Int) — [`THRESHOLD_NONE`] or [`THRESHOLD_LOW`]

use kc_core::{BatteryStatus, Platform, Plugin, PluginContext, Result};
use kc_protocol::NetworkPacket;

pub const PACKET_TYPE_BATTERY: &str = "kdeconnect.battery";
/// No threshold event.
pub const THRESHOLD_NONE: i32 = 0;
/// Battery is low.
pub const THRESHOLD_LOW: i32 = 1;

#[derive(Default, Debug)]
pub struct BatteryPlugin;

impl BatteryPlugin {
    /// Parse a `kdeconnect.battery` body into the host-facing status.
    pub fn parse(packet: &NetworkPacket) -> Option<BatteryStatus> {
        if packet.ty != PACKET_TYPE_BATTERY {
            return None;
        }
        Some(BatteryStatus {
            current_charge: packet.get_i64("currentCharge").unwrap_or(0) as i32,
            is_charging: packet.get_bool("isCharging").unwrap_or(false),
            threshold_event: packet
                .get_i64("thresholdEvent")
                .unwrap_or(THRESHOLD_NONE as i64) as i32,
        })
    }

    pub(crate) fn handle(packet: &NetworkPacket, device_id: &str, platform: &dyn Platform) -> bool {
        match Self::parse(packet) {
            Some(status) => {
                platform.on_battery(device_id, &status);
                true
            }
            None => false,
        }
    }
}

impl Plugin for BatteryPlugin {
    fn key(&self) -> &'static str {
        "BatteryPlugin"
    }
    fn display_name(&self) -> &'static str {
        "Battery"
    }
    fn description(&self) -> &'static str {
        "Report the battery status of a paired device"
    }
    fn supported_packet_types(&self) -> &'static [&'static str] {
        &[PACKET_TYPE_BATTERY]
    }
    fn outgoing_packet_types(&self) -> &'static [&'static str] {
        &[PACKET_TYPE_BATTERY]
    }

    fn on_packet(&self, ctx: &PluginContext, packet: &NetworkPacket) -> Result<bool> {
        Ok(Self::handle(packet, &ctx.device_id, ctx.platform.as_ref()))
    }
}

/// Build a `kdeconnect.battery` status packet.
pub fn battery_packet(
    current_charge: i32,
    is_charging: bool,
    threshold_event: i32,
) -> NetworkPacket {
    let mut packet = NetworkPacket::new(PACKET_TYPE_BATTERY);
    packet.set_i64("currentCharge", current_charge as i64);
    packet.set_bool("isCharging", is_charging);
    packet.set_i64("thresholdEvent", threshold_event as i64);
    packet
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    use kc_protocol::DeviceType;

    use super::*;

    #[derive(Default)]
    struct RecordingPlatform {
        batteries: AtomicUsize,
        last: Mutex<BatteryStatus>,
    }

    impl Platform for RecordingPlatform {
        fn device_name(&self) -> String {
            "test".to_owned()
        }
        fn device_type(&self) -> DeviceType {
            DeviceType::Desktop
        }

        fn on_battery(&self, _device_id: &str, status: &BatteryStatus) {
            self.batteries.fetch_add(1, Ordering::SeqCst);
            *self.last.lock().unwrap() = status.clone();
        }
    }

    #[test]
    fn battery_packet_roundtrips_through_wire() {
        let packet = battery_packet(73, true, THRESHOLD_LOW);
        let back = NetworkPacket::deserialize(&packet.serialize()).unwrap();
        assert_eq!(back.ty, PACKET_TYPE_BATTERY);
        assert_eq!(back.get_i64("currentCharge"), Some(73));
        assert_eq!(back.get_bool("isCharging"), Some(true));
        assert_eq!(back.get_i64("thresholdEvent"), Some(1));
    }

    #[test]
    fn parse_defaults_missing_fields() {
        let packet = NetworkPacket::new(PACKET_TYPE_BATTERY);
        let status = BatteryPlugin::parse(&packet).unwrap();
        assert_eq!(status.current_charge, 0);
        assert!(!status.is_charging);
        assert_eq!(status.threshold_event, THRESHOLD_NONE);

        let other = battery_packet(50, false, THRESHOLD_NONE);
        let mut wrong_type = other.clone();
        wrong_type.ty = "kdeconnect.ping".to_owned();
        assert!(BatteryPlugin::parse(&wrong_type).is_none());
    }

    #[test]
    fn handle_reports_status_to_host() {
        let platform = RecordingPlatform::default();
        let packet = battery_packet(20, false, THRESHOLD_LOW);
        assert!(BatteryPlugin::handle(&packet, "dev", &platform));
        assert_eq!(platform.batteries.load(Ordering::SeqCst), 1);
        let last = platform.last.lock().unwrap();
        assert_eq!(last.current_charge, 20);
        assert!(!last.is_charging);
        assert_eq!(last.threshold_event, THRESHOLD_LOW);
    }
}
