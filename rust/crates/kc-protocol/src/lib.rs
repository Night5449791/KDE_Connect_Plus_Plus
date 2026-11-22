//! KDE Connect wire protocol implementation.
//!
//! This crate is the Rust counterpart of the Kotlin classes
//! `NetworkPacket`, `DeviceInfo`, `PairingHandler`, `RsaHelper` and `SslHelper`
//! in the upstream KDE Connect Android 1.37.3 source.
//!
//! The wire format is a newline-terminated UTF-8 JSON object. Large data is
//! never embedded in the JSON: `payloadSize` and `payloadTransferInfo` tell
//! the receiver where to fetch the raw bytes (a separate TLS connection on
//! LAN, a multiplexed RFCOMM channel on Bluetooth).

pub mod identity;
pub mod packet;
pub mod pairing;
pub mod security;

pub use identity::{
    generate_device_id, is_valid_device_id, is_valid_identity_packet, sanitize_device_name,
    DeviceInfo, DeviceType, PROTOCOL_VERSION,
};
pub use packet::{
    NetworkPacket, Payload, SerializedPacket, PACKET_TYPE_IDENTITY, PACKET_TYPE_PAIR,
};
pub use pairing::{PairState, PairingError, PairingEvent, PairingHandler, PairingResult};
pub use security::{
    cert_der_from_pem, cert_pem_from_der, generate_identity, parse_certificate_der, Identity,
    TlsContext,
};

use thiserror::Error;

/// Errors produced while decoding or encoding protocol packets.
#[derive(Debug, Error)]
pub enum ProtocolError {
    #[error("invalid UTF-8 in packet: {0}")]
    InvalidUtf8(#[from] std::str::Utf8Error),
    #[error("invalid JSON packet: {0}")]
    Json(#[from] serde_json::Error),
    #[error("packet has payloadSize but no payloadTransferInfo")]
    MissingPayloadTransferInfo,
    #[error("packet has payloadTransferInfo but no payloadSize")]
    MissingPayloadSize,
    #[error("invalid identity packet: {0}")]
    InvalidIdentity(String),
    #[error("certificate error: {0}")]
    Certificate(String),
    #[error("TLS error: {0}")]
    Tls(String),
}
