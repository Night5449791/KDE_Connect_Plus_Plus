//! JSON view of the device list handed to Kotlin across JNI.
//!
//! `kc_core::DeviceSnapshot` deliberately has no serde derives (the core crate
//! stays transport-agnostic), so the Android bridge defines its own DTO. That
//! also gives us a JVM-free unit test target: the JNI `devices()` function is a
//! thin wrapper around [`snapshots_to_json`].

use kc_core::DeviceSnapshot;
use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DeviceSnapshotJson {
    pub device_id: String,
    pub name: String,
    pub device_type: String,
    pub protocol_version: u32,
    pub paired: bool,
    pub reachable: bool,
    pub link_count: usize,
    pub loaded_plugins: Vec<String>,
}

impl From<&DeviceSnapshot> for DeviceSnapshotJson {
    fn from(s: &DeviceSnapshot) -> Self {
        Self {
            device_id: s.device_id.clone(),
            name: s.name.clone(),
            device_type: s.device_type.clone(),
            protocol_version: s.protocol_version,
            paired: s.paired,
            reachable: s.reachable,
            link_count: s.link_count,
            loaded_plugins: s.loaded_plugins.clone(),
        }
    }
}

/// Serialize the whole device list. Never panics: a bogus snapshot would still
/// yield a valid (empty) JSON array so Kotlin can keep polling.
pub fn snapshots_to_json(snapshots: &[DeviceSnapshot]) -> String {
    let items: Vec<DeviceSnapshotJson> = snapshots.iter().map(DeviceSnapshotJson::from).collect();
    serde_json::to_string(&items).unwrap_or_else(|_| "[]".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(paired: bool) -> DeviceSnapshot {
        DeviceSnapshot {
            device_id: "0123456789abcdef0123456789abcdef".to_owned(),
            name: "Desktop".to_owned(),
            device_type: "desktop".to_owned(),
            protocol_version: 8,
            paired,
            reachable: true,
            link_count: 1,
            loaded_plugins: vec!["PingPlugin".to_owned(), "ClipboardPlugin".to_owned()],
        }
    }

    #[test]
    fn empty_list_is_a_json_array() {
        assert_eq!(snapshots_to_json(&[]), "[]");
    }

    #[test]
    fn snapshot_fields_are_exposed_to_kotlin() {
        let json = snapshots_to_json(&[sample(true), sample(false)]);
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        let arr = value.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["device_id"], "0123456789abcdef0123456789abcdef");
        assert_eq!(arr[0]["name"], "Desktop");
        assert_eq!(arr[0]["device_type"], "desktop");
        assert_eq!(arr[0]["protocol_version"], 8);
        assert_eq!(arr[0]["paired"], true);
        assert_eq!(arr[0]["reachable"], true);
        assert_eq!(arr[0]["link_count"], 1);
        assert_eq!(arr[1]["paired"], false);
        assert_eq!(arr[0]["loaded_plugins"][0], "PingPlugin");
    }
}
