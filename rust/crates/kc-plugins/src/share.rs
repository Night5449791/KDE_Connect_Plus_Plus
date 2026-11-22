//! Share plugin: sending and receiving text, URLs and files.
//!
//! Wire types (upstream `SharePlugin`):
//! * `kdeconnect.share.request` — text / url / file share
//! * `kdeconnect.share.request.update` — progress update for a file share

use kc_core::{Plugin, PluginContext, Result};
use kc_protocol::NetworkPacket;

const SHARE_REQUEST: &str = "kdeconnect.share.request";
const SHARE_UPDATE: &str = "kdeconnect.share.request.update";

/// Decoded content of an incoming share packet.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ShareContent {
    #[default]
    None,
    Text(String),
    Url(String),
    File {
        filename: String,
        last_modified: i64,
        open: bool,
        number_of_files: i32,
        total_payload_size: i64,
    },
    Update {
        number_of_files: i32,
        total_payload_size: i64,
    },
}

impl ShareContent {
    /// Upstream branch order is filename (file), then text, then url.
    pub fn from_packet(packet: &NetworkPacket) -> Self {
        if packet.ty == SHARE_UPDATE {
            return Self::Update {
                number_of_files: packet.get_i64("numberOfFiles").unwrap_or(0) as i32,
                total_payload_size: packet.get_i64("totalPayloadSize").unwrap_or(0),
            };
        }
        if let Some(filename) = packet.get_string("filename") {
            return Self::File {
                filename,
                last_modified: packet.get_i64("lastModified").unwrap_or(0),
                open: packet.get_bool_or("open", false),
                number_of_files: packet.get_i64("numberOfFiles").unwrap_or(1) as i32,
                total_payload_size: packet.get_i64("totalPayloadSize").unwrap_or(0),
            };
        }
        if let Some(text) = packet.get_string("text") {
            return Self::Text(text);
        }
        if let Some(url) = packet.get_string("url") {
            return Self::Url(url);
        }
        Self::None
    }
}

/// Share a plain text snippet.
pub fn share_text(text: impl Into<String>) -> NetworkPacket {
    let mut packet = NetworkPacket::new(SHARE_REQUEST);
    packet.set_str("text", text);
    packet
}

/// Share a URL.
pub fn share_url(url: impl Into<String>) -> NetworkPacket {
    let mut packet = NetworkPacket::new(SHARE_REQUEST);
    packet.set_str("url", url);
    packet
}

/// Share a file; the bytes travel as the packet payload.
pub fn share_file(filename: impl Into<String>, bytes: &[u8], last_modified: i64) -> NetworkPacket {
    let mut packet = NetworkPacket::new(SHARE_REQUEST);
    packet.set_str("filename", filename);
    packet.set_i64("lastModified", last_modified);
    packet.set_payload_bytes(bytes.to_vec());
    packet
}

/// Progress update for an ongoing (possibly multi-file) file share.
pub fn share_update(number_of_files: i32, total_payload_size: i64) -> NetworkPacket {
    let mut packet = NetworkPacket::new(SHARE_UPDATE);
    packet.set_i64("numberOfFiles", number_of_files as i64);
    packet.set_i64("totalPayloadSize", total_payload_size);
    packet
}

#[derive(Default, Debug)]
pub struct SharePlugin;

impl Plugin for SharePlugin {
    fn key(&self) -> &'static str {
        "SharePlugin"
    }
    fn display_name(&self) -> &'static str {
        "Share"
    }
    fn description(&self) -> &'static str {
        "Send and receive files, text and URLs"
    }
    fn supported_packet_types(&self) -> &'static [&'static str] {
        &[SHARE_REQUEST, SHARE_UPDATE]
    }
    fn outgoing_packet_types(&self) -> &'static [&'static str] {
        &[SHARE_REQUEST]
    }

    fn on_packet(&self, ctx: &PluginContext, packet: &NetworkPacket) -> Result<bool> {
        match packet.ty.as_str() {
            SHARE_REQUEST | SHARE_UPDATE => {
                let content = ShareContent::from_packet(packet);
                tracing::debug!(device = %ctx.device_id, ?content, "share packet received");
                ctx.platform.on_share(&ctx.device_id, packet);
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kc_protocol::Payload;
    use serde_json::{Map, Value};

    #[test]
    fn text_and_url_constructors_roundtrip() {
        let text = share_text("hello");
        assert_eq!(text.ty, SHARE_REQUEST);
        let back = NetworkPacket::deserialize(&text.serialize()).unwrap();
        assert_eq!(back.get_str("text"), Some("hello"));
        assert_eq!(
            ShareContent::from_packet(&back),
            ShareContent::Text("hello".into())
        );

        let url = share_url("https://example.com");
        let back = NetworkPacket::deserialize(&url.serialize()).unwrap();
        assert_eq!(back.get_str("url"), Some("https://example.com"));
        assert_eq!(
            ShareContent::from_packet(&back),
            ShareContent::Url("https://example.com".into())
        );
    }

    #[test]
    fn file_constructor_carries_payload_and_metadata() {
        let packet = share_file("report.pdf", b"abc", 1700000000);
        assert_eq!(packet.get_str("filename"), Some("report.pdf"));
        assert_eq!(packet.get_i64("lastModified"), Some(1700000000));
        assert_eq!(packet.payload_size, Some(3));
        assert!(matches!(&packet.payload, Some(Payload::Bytes(b)) if b.as_slice() == b"abc"));

        // A payload on the wire needs transfer info; add a dummy one for the roundtrip.
        let mut wire = packet.clone();
        wire.payload_transfer_info = Some(Map::from_iter([(
            "port".to_string(),
            Value::Number(1741.into()),
        )]));
        let back = NetworkPacket::deserialize(&wire.serialize()).unwrap();
        assert_eq!(back.payload_size, Some(3));
        assert_eq!(back.get_str("filename"), Some("report.pdf"));
    }

    #[test]
    fn update_carrier_roundtrips() {
        let packet = share_update(3, 4096);
        assert_eq!(packet.ty, SHARE_UPDATE);
        let back = NetworkPacket::deserialize(&packet.serialize()).unwrap();
        assert_eq!(back.get_i64("numberOfFiles"), Some(3));
        assert_eq!(back.get_i64("totalPayloadSize"), Some(4096));
        assert_eq!(
            ShareContent::from_packet(&back),
            ShareContent::Update {
                number_of_files: 3,
                total_payload_size: 4096
            }
        );
    }

    #[test]
    fn parse_file_flags_and_unknown_packet() {
        let mut packet = NetworkPacket::new(SHARE_REQUEST);
        packet.set_str("filename", "a.txt");
        packet.set_i64("lastModified", 42);
        packet.set_bool("open", true);
        packet.set_i64("numberOfFiles", 2);
        packet.set_i64("totalPayloadSize", 99);
        assert_eq!(
            ShareContent::from_packet(&packet),
            ShareContent::File {
                filename: "a.txt".into(),
                last_modified: 42,
                open: true,
                number_of_files: 2,
                total_payload_size: 99,
            }
        );

        assert_eq!(
            ShareContent::from_packet(&NetworkPacket::new(SHARE_REQUEST)),
            ShareContent::None
        );
    }
}
