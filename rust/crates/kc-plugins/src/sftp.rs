//! SFTP plugin: consumed `kdeconnect.sftp.request` packets are handed to the
//! host's file-transfer service, which replies with an `sftp_packet`.

use kc_core::{Plugin, PluginContext, Result};
use kc_protocol::NetworkPacket;

const SFTP: &str = "kdeconnect.sftp";
const SFTP_REQUEST: &str = "kdeconnect.sftp.request";

/// Connection details advertised after an SFTP server has been started.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SftpInfo {
    pub port: u16,
    pub user: String,
    pub password: String,
    pub path: String,
}

impl SftpInfo {
    pub fn from_packet(packet: &NetworkPacket) -> Self {
        Self {
            port: packet.get_i64("port").unwrap_or(0) as u16,
            user: packet.get_string("user").unwrap_or_default(),
            password: packet.get_string("password").unwrap_or_default(),
            path: packet.get_string("path").unwrap_or_default(),
        }
    }
}

/// Reply with the SFTP endpoint of this device.
pub fn sftp_packet(
    port: u16,
    user: impl Into<String>,
    password: impl Into<String>,
    path: impl Into<String>,
) -> NetworkPacket {
    let mut packet = NetworkPacket::new(SFTP);
    packet.set_i64("port", port as i64);
    packet.set_str("user", user);
    packet.set_str("password", password);
    packet.set_str("path", path);
    packet
}

/// Ask the peer to start an SFTP server and send back its connection details.
pub fn request_sftp_packet() -> NetworkPacket {
    NetworkPacket::new(SFTP_REQUEST)
}

#[derive(Default, Debug)]
pub struct SftpPlugin;

impl Plugin for SftpPlugin {
    fn key(&self) -> &'static str {
        "SftpPlugin"
    }
    fn display_name(&self) -> &'static str {
        "SFTP"
    }
    fn description(&self) -> &'static str {
        "Browse files on a paired device over SFTP"
    }
    fn supported_packet_types(&self) -> &'static [&'static str] {
        &[SFTP_REQUEST]
    }
    fn outgoing_packet_types(&self) -> &'static [&'static str] {
        &[SFTP]
    }

    fn on_packet(&self, ctx: &PluginContext, packet: &NetworkPacket) -> Result<bool> {
        if packet.ty != SFTP_REQUEST {
            return Ok(false);
        }
        // The host starts the SFTP server and replies with `sftp_packet`; do
        // not echo the request back or two peers would loop indefinitely.
        ctx.platform.on_file_transfer(&ctx.device_id, packet);
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sftp_packet_roundtrips() {
        let packet = sftp_packet(1741, "user", "secret", "/home/user");
        let back = NetworkPacket::deserialize(&packet.serialize()).unwrap();
        assert_eq!(back.ty, SFTP);
        assert_eq!(back.get_i64("port"), Some(1741));
        assert_eq!(back.get_str("user"), Some("user"));
        assert_eq!(back.get_str("password"), Some("secret"));
        assert_eq!(back.get_str("path"), Some("/home/user"));
        assert_eq!(
            SftpInfo::from_packet(&back),
            SftpInfo {
                port: 1741,
                user: "user".into(),
                password: "secret".into(),
                path: "/home/user".into()
            }
        );
    }

    #[test]
    fn request_packet_roundtrips() {
        let packet = request_sftp_packet();
        assert_eq!(packet.ty, SFTP_REQUEST);
        let back = NetworkPacket::deserialize(&packet.serialize()).unwrap();
        assert_eq!(back.ty, SFTP_REQUEST);
    }

    #[test]
    fn request_details_are_parsed_with_defaults() {
        let mut packet = NetworkPacket::new(SFTP_REQUEST);
        packet.set_i64("port", 2222);
        packet.set_str("user", "remote");
        assert_eq!(
            SftpInfo::from_packet(&packet),
            SftpInfo {
                port: 2222,
                user: "remote".into(),
                ..SftpInfo::default()
            }
        );
        assert_eq!(
            SftpInfo::from_packet(&NetworkPacket::new(SFTP_REQUEST)),
            SftpInfo::default()
        );
    }
}
