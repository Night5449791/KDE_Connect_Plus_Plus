//! `ConnectivityReportPlugin`: report mobile signal strengths to the host.
//!
//! Wire body (upstream `kdeconnect.connectivity_report`): `signalStrengths`
//! is an object mapping a subscription-id string to an object with
//! `networkType` (String) and `signalStrength` (Int).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use kc_core::{Platform, Plugin, PluginContext, Result};
use kc_protocol::NetworkPacket;

pub const PACKET_TYPE_CONNECTIVITY_REPORT: &str = "kdeconnect.connectivity_report";

/// One subscription's network type and signal strength, as sent on the wire.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct SignalInfo {
    #[serde(rename = "networkType")]
    pub network_type: String,
    #[serde(rename = "signalStrength")]
    pub signal_strength: i32,
}

/// Subscription id -> signal information.
pub type SignalMap = BTreeMap<String, SignalInfo>;

#[derive(Default, Debug)]
pub struct ConnectivityReportPlugin;

impl ConnectivityReportPlugin {
    /// Parse the `signalStrengths` object of a connectivity report.
    pub fn parse(packet: &NetworkPacket) -> Option<SignalMap> {
        let object = packet.get_object("signalStrengths")?;
        serde_json::from_value(Value::Object(object.clone())).ok()
    }

    pub(crate) fn handle(packet: &NetworkPacket, device_id: &str, platform: &dyn Platform) -> bool {
        if packet.ty != PACKET_TYPE_CONNECTIVITY_REPORT {
            return false;
        }
        // Validate the payload shape before handing the raw packet to the host.
        if Self::parse(packet).is_none() {
            return false;
        }
        platform.on_connectivity(device_id, packet);
        true
    }
}

impl Plugin for ConnectivityReportPlugin {
    fn key(&self) -> &'static str {
        "ConnectivityReportPlugin"
    }
    fn display_name(&self) -> &'static str {
        "Connectivity report"
    }
    fn description(&self) -> &'static str {
        "Report the mobile network signal strengths"
    }
    fn supported_packet_types(&self) -> &'static [&'static str] {
        &[PACKET_TYPE_CONNECTIVITY_REPORT]
    }
    fn outgoing_packet_types(&self) -> &'static [&'static str] {
        &[PACKET_TYPE_CONNECTIVITY_REPORT]
    }

    fn on_packet(&self, ctx: &PluginContext, packet: &NetworkPacket) -> Result<bool> {
        Ok(Self::handle(packet, &ctx.device_id, ctx.platform.as_ref()))
    }
}

/// Build a `kdeconnect.connectivity_report` packet from a subscription map.
pub fn connectivity_report_packet(signals: &SignalMap) -> NetworkPacket {
    let mut packet = NetworkPacket::new(PACKET_TYPE_CONNECTIVITY_REPORT);
    let mut body = Map::new();
    for (subscription_id, info) in signals {
        let value = serde_json::to_value(info).unwrap_or(Value::Null);
        body.insert(subscription_id.clone(), value);
    }
    packet.set("signalStrengths", body);
    packet
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use kc_protocol::DeviceType;

    use super::*;

    struct RecordingPlatform {
        reports: AtomicUsize,
    }

    impl RecordingPlatform {
        fn new() -> Self {
            Self {
                reports: AtomicUsize::new(0),
            }
        }
    }

    impl Platform for RecordingPlatform {
        fn device_name(&self) -> String {
            "test".to_owned()
        }
        fn device_type(&self) -> DeviceType {
            DeviceType::Phone
        }

        fn on_connectivity(&self, _device_id: &str, _packet: &NetworkPacket) {
            self.reports.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn sample_map() -> SignalMap {
        BTreeMap::from([
            (
                "1".to_owned(),
                SignalInfo {
                    network_type: "GSM".to_owned(),
                    signal_strength: 18,
                },
            ),
            (
                "2".to_owned(),
                SignalInfo {
                    network_type: "LTE".to_owned(),
                    signal_strength: 7,
                },
            ),
        ])
    }

    #[test]
    fn signal_info_uses_upstream_json_field_names() {
        let info = SignalInfo {
            network_type: "LTE".to_owned(),
            signal_strength: 7,
        };
        let value = serde_json::to_value(&info).unwrap();
        assert_eq!(value["networkType"], "LTE");
        assert_eq!(value["signalStrength"], 7);
        assert!(value.get("network_type").is_none());

        let back: SignalInfo = serde_json::from_value(value).unwrap();
        assert_eq!(back, info);
    }

    #[test]
    fn connectivity_packet_roundtrips_through_wire() {
        let packet = connectivity_report_packet(&sample_map());
        let back = NetworkPacket::deserialize(&packet.serialize()).unwrap();
        assert_eq!(back.ty, PACKET_TYPE_CONNECTIVITY_REPORT);

        let parsed = ConnectivityReportPlugin::parse(&back).unwrap();
        assert_eq!(parsed, sample_map());
        assert_eq!(parsed["1"].network_type, "GSM");
        assert_eq!(parsed["2"].signal_strength, 7);
    }

    #[test]
    fn malformed_payload_is_not_consumed() {
        let platform = RecordingPlatform::new();
        let good = connectivity_report_packet(&sample_map());
        assert!(ConnectivityReportPlugin::handle(&good, "dev", &platform));
        assert_eq!(platform.reports.load(Ordering::SeqCst), 1);

        let mut malformed = NetworkPacket::new(PACKET_TYPE_CONNECTIVITY_REPORT);
        malformed.set_str("signalStrengths", "not-an-object");
        assert!(!ConnectivityReportPlugin::handle(
            &malformed, "dev", &platform
        ));
        assert_eq!(platform.reports.load(Ordering::SeqCst), 1);

        let other = NetworkPacket::new("kdeconnect.ping");
        assert!(!ConnectivityReportPlugin::handle(&other, "dev", &platform));
    }
}
