//! `NetworkPacket` and its newline-delimited JSON wire representation.
//!
//! Mirrors `org.kde.kdeconnect.NetworkPacket` from upstream. The Kotlin
//! implementation assigns `id` at serialization time from wall-clock
//! milliseconds and discards it on deserialization; we do the same.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::ProtocolError;

pub const PACKET_TYPE_IDENTITY: &str = "kdeconnect.identity";
pub const PACKET_TYPE_PAIR: &str = "kdeconnect.pair";

/// Maximum size of a packet on the main link (32 MiB upstream).
pub const MAX_PACKET_SIZE: usize = 32 * 1024 * 1024;
/// Maximum size of an identity packet (512 KiB upstream).
pub const MAX_IDENTITY_SIZE: usize = 512 * 1024;

/// The exact JSON shape sent on the wire.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SerializedPacket {
    #[serde(default)]
    pub id: i64,
    #[serde(rename = "type")]
    pub ty: String,
    #[serde(default)]
    pub body: Map<String, Value>,
    #[serde(
        default,
        rename = "payloadSize",
        skip_serializing_if = "Option::is_none"
    )]
    pub payload_size: Option<u64>,
    #[serde(
        default,
        rename = "payloadTransferInfo",
        skip_serializing_if = "Option::is_none"
    )]
    pub payload_transfer_info: Option<Map<String, Value>>,
}

/// Raw bytes associated with a packet.
///
/// Upstream supports ByteArray, InputStream and Socket payloads. For a
/// platform-neutral core we keep either owned bytes or a path to a file;
/// the transport layer is responsible for streaming the bytes to the peer.
#[derive(Clone, Debug)]
pub enum Payload {
    Bytes(Vec<u8>),
    File(PathBuf),
}

impl Payload {
    pub fn len(&self) -> u64 {
        match self {
            Self::Bytes(b) => b.len() as u64,
            Self::File(_) => 0, // filled in by the transport when the file is stat-ed
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// In-memory representation of a KDE Connect packet.
#[derive(Clone, Debug, Default)]
pub struct NetworkPacket {
    pub id: i64,
    pub ty: String,
    pub body: Map<String, Value>,
    pub payload_size: Option<u64>,
    pub payload_transfer_info: Option<Map<String, Value>>,
    pub payload: Option<Payload>,
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

impl NetworkPacket {
    pub fn new(ty: impl Into<String>) -> Self {
        Self {
            ty: ty.into(),
            ..Self::default()
        }
    }

    pub fn with_type(ty: impl Into<String>) -> Self {
        Self::new(ty)
    }

    /// Generate the wire JSON. A trailing newline is appended, as upstream.
    pub fn serialize(&self) -> String {
        let mut wire = SerializedPacket {
            id: if self.id == 0 { now_millis() } else { self.id },
            ty: self.ty.clone(),
            body: self.body.clone(),
            payload_size: self.payload_size,
            payload_transfer_info: self.payload_transfer_info.clone(),
        };
        if wire.body.is_empty() {
            wire.body = Map::new();
        }
        let mut s = serde_json::to_string(&wire).unwrap_or_else(|_| "{}".to_string());
        s.push('\n');
        s
    }

    /// Parse one line of wire JSON. The `id` field is intentionally discarded.
    pub fn deserialize(line: &str) -> Result<Self, ProtocolError> {
        let wire: SerializedPacket = serde_json::from_str(line.trim_end())?;
        if wire.payload_size.is_some() != wire.payload_transfer_info.is_some() {
            return Err(ProtocolError::MissingPayloadTransferInfo);
        }
        Ok(Self {
            id: 0,
            ty: wire.ty,
            body: wire.body,
            payload_size: wire.payload_size,
            payload_transfer_info: wire.payload_transfer_info,
            payload: None,
        })
    }

    pub fn has_payload(&self) -> bool {
        self.payload_size.unwrap_or(0) > 0 && self.payload.is_some()
    }

    pub fn set_payload_bytes(&mut self, bytes: Vec<u8>) {
        self.payload_size = Some(bytes.len() as u64);
        self.payload = Some(Payload::Bytes(bytes));
    }

    pub fn set_payload_file(&mut self, path: impl Into<PathBuf>, size: u64) {
        self.payload_size = Some(size);
        self.payload = Some(Payload::File(path.into()));
    }

    // ---- body accessors -------------------------------------------------

    pub fn set(&mut self, key: impl Into<String>, value: impl Into<Value>) {
        self.body.insert(key.into(), value.into());
    }

    pub fn set_str(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.set(key, Value::String(value.into()));
    }

    pub fn set_bool(&mut self, key: impl Into<String>, value: bool) {
        self.set(key, Value::Bool(value));
    }

    pub fn set_i64(&mut self, key: impl Into<String>, value: i64) {
        self.set(key, Value::Number(value.into()));
    }

    pub fn set_u64(&mut self, key: impl Into<String>, value: u64) {
        self.set(key, Value::Number(value.into()));
    }

    pub fn set_f64(&mut self, key: impl Into<String>, value: f64) {
        if let Some(n) = serde_json::Number::from_f64(value) {
            self.set(key, n);
        }
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.body.get(key)
    }

    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.body.get(key).and_then(|v| v.as_str())
    }

    pub fn get_string(&self, key: &str) -> Option<String> {
        self.get_str(key).map(str::to_owned)
    }

    pub fn get_bool(&self, key: &str) -> Option<bool> {
        self.body.get(key).and_then(|v| v.as_bool())
    }

    pub fn get_bool_or(&self, key: &str, default: bool) -> bool {
        self.get_bool(key).unwrap_or(default)
    }

    pub fn get_i64(&self, key: &str) -> Option<i64> {
        self.body.get(key).and_then(|v| v.as_i64())
    }

    pub fn get_u64(&self, key: &str) -> Option<u64> {
        self.body.get(key).and_then(|v| v.as_u64())
    }

    pub fn get_f64(&self, key: &str) -> Option<f64> {
        self.body.get(key).and_then(|v| v.as_f64())
    }

    pub fn get_array(&self, key: &str) -> Option<&Vec<Value>> {
        self.body.get(key).and_then(|v| v.as_array())
    }

    pub fn get_object(&self, key: &str) -> Option<&Map<String, Value>> {
        self.body.get(key).and_then(|v| v.as_object())
    }

    pub fn get_string_list(&self, key: &str) -> Option<Vec<String>> {
        self.get_array(key).map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_without_payload() {
        let mut p = NetworkPacket::new("kdeconnect.ping");
        p.set_str("message", "hello");
        let wire = p.serialize();
        assert!(wire.ends_with('\n'));
        assert!(!wire.contains("payloadSize"));
        let back = NetworkPacket::deserialize(&wire).unwrap();
        assert_eq!(back.ty, "kdeconnect.ping");
        assert_eq!(back.get_str("message"), Some("hello"));
        assert_eq!(back.id, 0);
    }

    #[test]
    fn roundtrip_with_payload() {
        let mut p = NetworkPacket::new("kdeconnect.share.request");
        p.set_str("filename", "a.pdf");
        p.set_payload_bytes(vec![1, 2, 3]);
        p.payload_transfer_info = Some(Map::from_iter([(
            "port".to_string(),
            Value::Number(1741.into()),
        )]));
        let wire = p.serialize();
        assert!(wire.contains("\"payloadSize\":3"));
        let back = NetworkPacket::deserialize(&wire).unwrap();
        assert_eq!(back.payload_size, Some(3));
        assert_eq!(back.payload_transfer_info.unwrap()["port"], 1741);
    }

    #[test]
    fn payload_size_without_transfer_info_is_rejected() {
        let line = r#"{"id":1,"type":"x","body":{},"payloadSize":3}"#;
        assert!(matches!(
            NetworkPacket::deserialize(line),
            Err(ProtocolError::MissingPayloadTransferInfo)
        ));
    }

    #[test]
    fn upstream_identity_shape_is_accepted() {
        let line = r#"{"id":1700000000123,"type":"kdeconnect.identity","body":{"deviceId":"0123456789abcdef0123456789abcdef","deviceName":"Phone","deviceType":"phone","protocolVersion":8,"tcpPort":1716,"incomingCapabilities":["kdeconnect.ping"],"outgoingCapabilities":[]}}"#;
        let p = NetworkPacket::deserialize(line).unwrap();
        assert_eq!(
            p.get_str("deviceId"),
            Some("0123456789abcdef0123456789abcdef")
        );
        assert_eq!(p.get_i64("protocolVersion"), Some(8));
    }
}
