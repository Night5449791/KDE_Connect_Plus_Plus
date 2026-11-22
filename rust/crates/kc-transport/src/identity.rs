//! Identity packet construction and target validation.
//!
//! Upstream sends three slightly different identity packets:
//! * UDP broadcast: the plain identity with `tcpPort` added.
//! * TCP plaintext (connector side): the same packet plus `targetDeviceId`
//!   and `targetProtocolVersion`.
//! * Inside TLS (protocol v8): the plain identity again, with `tcpPort`.

use kc_core::{Result, Store};
use kc_protocol::identity::is_valid_identity_packet;
use kc_protocol::{cert_der_from_pem, DeviceInfo, NetworkPacket, ProtocolError, PROTOCOL_VERSION};

/// Build the identity packet advertised over UDP / used inside the TLS tunnel,
/// with the listening TCP port filled in.
pub fn identity_with_tcp_port(info: &DeviceInfo, tcp_port: u16) -> NetworkPacket {
    let mut info = info.clone();
    info.tcp_port = Some(tcp_port);
    info.to_identity_packet()
}

/// Add the fields the passive side validates before starting TLS.
///
/// Kotlin writes `targetProtocolVersion` as a JSON *string* (it assigns
/// `identityPacket.getString("protocolVersion")`), so we do the same here;
/// [`target_protocol_version`] accepts both forms on receive.
pub fn targeted_identity_packet(
    info: &DeviceInfo,
    tcp_port: u16,
    target_device_id: &str,
    target_protocol_version: u32,
) -> NetworkPacket {
    let mut packet = identity_with_tcp_port(info, tcp_port);
    packet.set_str("targetDeviceId", target_device_id);
    packet.set_str("targetProtocolVersion", target_protocol_version.to_string());
    packet
}

/// Read `targetProtocolVersion` from either a JSON number or a numeric string.
pub fn target_protocol_version(packet: &NetworkPacket) -> Option<u32> {
    if let Some(n) = packet.get_u64("targetProtocolVersion") {
        return u32::try_from(n).ok();
    }
    packet
        .get_str("targetProtocolVersion")
        .and_then(|s| s.parse::<u32>().ok())
}

/// Validate the `targetDeviceId` / `targetProtocolVersion` fields on the
/// passive (TCP-accepting) side. Missing fields are allowed, matching upstream.
pub fn check_targets(packet: &NetworkPacket, our_device_id: &str) -> Result<()> {
    if let Some(target) = packet.get_str("targetDeviceId") {
        if target != our_device_id {
            return Err(ProtocolError::InvalidIdentity(format!(
                "connection targeted at device {target}, not {our_device_id}"
            ))
            .into());
        }
    }
    if let Some(version) = target_protocol_version(packet) {
        if version != PROTOCOL_VERSION {
            return Err(ProtocolError::InvalidIdentity(format!(
                "target protocol version {version} != {PROTOCOL_VERSION}"
            ))
            .into());
        }
    }
    Ok(())
}

/// Convert a received identity packet into [`DeviceInfo`], rejecting malformed
/// identity packets the same way upstream does.
pub fn device_info_from_identity(packet: &NetworkPacket) -> Result<DeviceInfo> {
    if !is_valid_identity_packet(packet) {
        return Err(ProtocolError::InvalidIdentity(
            "invalid kdeconnect.identity packet".to_owned(),
        )
        .into());
    }
    Ok(DeviceInfo::from_packet(packet)?)
}

/// Pull the pinned certificate DER out of the store for a trusted device.
pub fn pinned_cert_der(store: &dyn Store, device_id: &str) -> Result<Vec<u8>> {
    let stored = store.stored_device(device_id).ok_or_else(|| {
        ProtocolError::InvalidIdentity(format!(
            "device {device_id} is trusted but has no stored certificate"
        ))
    })?;
    Ok(cert_der_from_pem(&stored.cert_pem)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kc_core::{CoreError, MemoryStore};
    use kc_protocol::{generate_identity, DeviceType};

    fn info() -> DeviceInfo {
        DeviceInfo::new(
            "0123456789abcdef0123456789abcdef",
            "Test Phone",
            DeviceType::Phone,
        )
    }

    #[test]
    fn identity_packet_carries_tcp_port() {
        let packet = identity_with_tcp_port(&info(), 1716);
        assert_eq!(packet.ty, kc_protocol::PACKET_TYPE_IDENTITY);
        assert_eq!(packet.get_u64("tcpPort"), Some(1716));
        let wire = packet.serialize();
        assert!(wire.ends_with('\n'));

        let back =
            device_info_from_identity(&kc_protocol::NetworkPacket::deserialize(&wire).unwrap())
                .unwrap();
        assert_eq!(back.tcp_port, Some(1716));
        assert_eq!(back.device_id, "0123456789abcdef0123456789abcdef");
    }

    #[test]
    fn udp_broadcast_identity_does_not_contain_target_fields() {
        let wire = identity_with_tcp_port(&info(), 1740).serialize();
        assert!(!wire.contains("targetDeviceId"));
        assert!(!wire.contains("targetProtocolVersion"));
    }

    #[test]
    fn targeted_identity_packet_sets_both_targets() {
        let packet = targeted_identity_packet(&info(), 1716, "feedface", 8);
        assert_eq!(packet.get_str("targetDeviceId"), Some("feedface"));
        assert_eq!(packet.get_str("targetProtocolVersion"), Some("8"));
        // Kotlin's getIntOrNull parses the string form.
        assert_eq!(target_protocol_version(&packet), Some(8));
        check_targets(&packet, "feedface").unwrap();
        assert!(check_targets(&packet, "other-device").is_err());
    }

    #[test]
    fn target_protocol_version_accepts_json_number() {
        let mut packet = identity_with_tcp_port(&info(), 1716);
        packet.set_i64("targetProtocolVersion", PROTOCOL_VERSION as i64);
        assert_eq!(target_protocol_version(&packet), Some(PROTOCOL_VERSION));
        check_targets(&packet, "anything").unwrap();
    }

    #[test]
    fn target_protocol_version_rejects_mismatch() {
        let mut packet = identity_with_tcp_port(&info(), 1716);
        packet.set_str("targetProtocolVersion", "7");
        assert!(matches!(
            check_targets(&packet, "any"),
            Err(CoreError::Protocol(ProtocolError::InvalidIdentity(_)))
        ));
    }

    #[test]
    fn pinned_cert_der_comes_from_store_pem() {
        let identity = generate_identity("0123456789abcdef0123456789abcdef").unwrap();
        let store = MemoryStore::new();
        store
            .trust_device(
                &identity.device_id,
                "Test Phone",
                "phone",
                &identity.cert_der,
            )
            .unwrap();
        let der = pinned_cert_der(&store, &identity.device_id).unwrap();
        assert_eq!(der, identity.cert_der);
    }
}
