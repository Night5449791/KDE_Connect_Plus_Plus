//! `DeviceInfo` / identity packet handling.

use std::collections::BTreeSet;

use rand::RngCore;

use crate::packet::NetworkPacket;
use crate::ProtocolError;

pub const PROTOCOL_VERSION: u32 = 8;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeviceType {
    Phone,
    Tablet,
    Tv,
    Laptop,
    Desktop,
    Unknown(String),
}

impl DeviceType {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Phone => "phone",
            Self::Tablet => "tablet",
            Self::Tv => "tv",
            Self::Laptop => "laptop",
            Self::Desktop => "desktop",
            Self::Unknown(s) => s,
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Self {
        match s {
            "phone" => Self::Phone,
            "tablet" => Self::Tablet,
            "tv" => Self::Tv,
            "laptop" => Self::Laptop,
            "desktop" => Self::Desktop,
            other => Self::Unknown(other.to_owned()),
        }
    }
}

#[derive(Clone, Debug)]
pub struct DeviceInfo {
    pub device_id: String,
    pub device_name: String,
    pub protocol_version: u32,
    pub device_type: DeviceType,
    /// Union of `supportedPacketTypes` of every enabled plugin.
    pub incoming_capabilities: BTreeSet<String>,
    /// Union of `outgoingPacketTypes` of every enabled plugin.
    pub outgoing_capabilities: BTreeSet<String>,
    /// Advertised in UDP broadcasts so the receiver knows where to connect.
    pub tcp_port: Option<u16>,
    /// PEM-encoded certificate; used on the Bluetooth path, not on LAN.
    pub certificate: Option<String>,
}

impl DeviceInfo {
    pub fn new(
        device_id: impl Into<String>,
        device_name: impl AsRef<str>,
        device_type: DeviceType,
    ) -> Self {
        Self {
            device_id: device_id.into(),
            device_name: sanitize_device_name(device_name),
            protocol_version: PROTOCOL_VERSION,
            device_type,
            incoming_capabilities: BTreeSet::new(),
            outgoing_capabilities: BTreeSet::new(),
            tcp_port: None,
            certificate: None,
        }
    }

    pub fn to_identity_packet(&self) -> NetworkPacket {
        let mut packet = NetworkPacket::new(crate::packet::PACKET_TYPE_IDENTITY);
        packet.set_str("deviceId", &self.device_id);
        packet.set_str("deviceName", &self.device_name);
        packet.set_str("deviceType", self.device_type.as_str());
        packet.set_i64("protocolVersion", self.protocol_version as i64);
        packet.set(
            "incomingCapabilities",
            self.incoming_capabilities
                .iter()
                .cloned()
                .collect::<Vec<_>>(),
        );
        packet.set(
            "outgoingCapabilities",
            self.outgoing_capabilities
                .iter()
                .cloned()
                .collect::<Vec<_>>(),
        );
        if let Some(port) = self.tcp_port {
            packet.set_i64("tcpPort", port as i64);
        }
        if let Some(cert) = &self.certificate {
            packet.set_str("certificate", cert);
        }
        packet
    }

    pub fn from_packet(packet: &NetworkPacket) -> Result<Self, ProtocolError> {
        if !is_valid_identity_packet(packet) {
            return Err(ProtocolError::InvalidIdentity(
                "not a valid kdeconnect.identity packet".to_owned(),
            ));
        }
        let device_id = packet.get_string("deviceId").unwrap_or_default();
        let device_name = packet.get_string("deviceName").unwrap_or_default();
        let device_type = DeviceType::from_str(packet.get_str("deviceType").unwrap_or("unknown"));
        let protocol_version = packet.get_i64("protocolVersion").unwrap_or(0) as u32;
        let incoming = string_set(packet, "incomingCapabilities");
        let outgoing = string_set(packet, "outgoingCapabilities");

        Ok(Self {
            device_id,
            device_name: sanitize_device_name(device_name),
            protocol_version,
            device_type,
            incoming_capabilities: incoming,
            outgoing_capabilities: outgoing,
            tcp_port: packet.get_i64("tcpPort").map(|p| p as u16),
            certificate: packet.get_string("certificate"),
        })
    }

    /// Capabilities this device can advertise based on installed plugins.
    pub fn set_capabilities(&mut self, incoming: &[&str], outgoing: &[&str]) {
        self.incoming_capabilities = incoming.iter().map(|s| (*s).to_owned()).collect();
        self.outgoing_capabilities = outgoing.iter().map(|s| (*s).to_owned()).collect();
    }
}

fn string_set(packet: &NetworkPacket, key: &str) -> BTreeSet<String> {
    packet
        .get_string_list(key)
        .map(|v| v.into_iter().collect())
        .unwrap_or_default()
}

/// Upstream allows 32-38 chars from `[a-zA-Z0-9_-]`.
pub fn is_valid_device_id(id: &str) -> bool {
    (32..=38).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

pub fn is_valid_identity_packet(packet: &NetworkPacket) -> bool {
    packet.ty == crate::packet::PACKET_TYPE_IDENTITY
        && packet
            .get_str("deviceName")
            .map(|n| !sanitize_device_name(n).is_empty())
            .unwrap_or(false)
        && packet
            .get_str("deviceId")
            .map(is_valid_device_id)
            .unwrap_or(false)
        && packet.get_i64("protocolVersion").is_some()
}

/// Generate a 32-hex-character device ID, matching upstream.
pub fn generate_device_id() -> String {
    let mut bytes = [0u8; 16];
    rand::rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

/// Upstream strips `"',;:.!?()[]<>` from device names.
pub fn sanitize_device_name(name: impl AsRef<str>) -> String {
    name.as_ref()
        .chars()
        .filter(|c| !"\"',;:.!?()[]<>".contains(*c))
        .take(32)
        .collect::<String>()
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_id_generation_and_validation() {
        let id = generate_device_id();
        assert_eq!(id.len(), 32);
        assert!(is_valid_device_id(&id));
        assert!(!is_valid_device_id("short"));
        assert!(is_valid_device_id(&"z".repeat(32)));
        assert!(!is_valid_device_id(&format!("{}!", "z".repeat(31))));
    }

    #[test]
    fn identity_roundtrip() {
        let mut info = DeviceInfo::new(
            "0123456789abcdef0123456789abcdef",
            "My Phone",
            DeviceType::Phone,
        );
        info.set_capabilities(&["kdeconnect.ping"], &["kdeconnect.battery"]);
        info.tcp_port = Some(1716);
        let packet = info.to_identity_packet();
        let back = DeviceInfo::from_packet(&packet).unwrap();
        assert_eq!(back.device_id, info.device_id);
        assert_eq!(back.device_name, info.device_name);
        assert_eq!(back.protocol_version, PROTOCOL_VERSION);
        assert!(back.incoming_capabilities.contains("kdeconnect.ping"));
        assert!(back.outgoing_capabilities.contains("kdeconnect.battery"));
        assert_eq!(back.tcp_port, Some(1716));
    }

    #[test]
    fn name_sanitization() {
        assert_eq!(sanitize_device_name("A'B\"C:D!"), "ABCD");
    }
}
