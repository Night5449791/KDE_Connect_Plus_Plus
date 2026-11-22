//! Receive-notifications plugin: consumes `kdeconnect.notification` packets
//! from a peer and offers the request constructor that starts the exchange.

use kc_core::{Plugin, PluginContext, Result};
use kc_protocol::NetworkPacket;

const NOTIFICATION: &str = "kdeconnect.notification";
const NOTIFICATION_REQUEST: &str = "kdeconnect.notification.request";

/// Fields of an incoming notification, as sent by `notification_packet`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NotificationInfo {
    pub id: String,
    pub app: String,
    pub title: String,
    pub text: String,
}

impl NotificationInfo {
    pub fn from_packet(packet: &NetworkPacket) -> Self {
        Self {
            id: packet.get_string("id").unwrap_or_default(),
            app: packet.get_string("app").unwrap_or_default(),
            title: packet.get_string("title").unwrap_or_default(),
            text: packet.get_string("text").unwrap_or_default(),
        }
    }
}

/// Ask the peer to send its current notifications.
pub fn request_notifications_packet() -> NetworkPacket {
    NetworkPacket::new(NOTIFICATION_REQUEST)
}

#[derive(Default, Debug)]
pub struct ReceiveNotificationsPlugin;

impl Plugin for ReceiveNotificationsPlugin {
    fn key(&self) -> &'static str {
        "ReceiveNotificationsPlugin"
    }
    fn display_name(&self) -> &'static str {
        "Receive notifications"
    }
    fn description(&self) -> &'static str {
        "Show notifications sent by a paired device"
    }
    fn supported_packet_types(&self) -> &'static [&'static str] {
        &[NOTIFICATION]
    }
    fn outgoing_packet_types(&self) -> &'static [&'static str] {
        &[NOTIFICATION_REQUEST]
    }

    fn on_packet(&self, ctx: &PluginContext, packet: &NetworkPacket) -> Result<bool> {
        if packet.ty != NOTIFICATION {
            return Ok(false);
        }
        tracing::debug!(device = %ctx.device_id, id = ?packet.get_str("id"), "notification received");
        ctx.platform.on_notification(&ctx.device_id, packet);
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_packet_roundtrips() {
        let packet = request_notifications_packet();
        assert_eq!(packet.ty, NOTIFICATION_REQUEST);
        assert!(packet.body.is_empty());
        let back = NetworkPacket::deserialize(&packet.serialize()).unwrap();
        assert_eq!(back.ty, NOTIFICATION_REQUEST);
        assert!(back.body.is_empty());
    }

    #[test]
    fn notification_fields_are_parsed() {
        let mut packet = NetworkPacket::new(NOTIFICATION);
        packet.set_str("id", "7");
        packet.set_str("app", "com.example");
        packet.set_str("title", "Ping");
        packet.set_str("text", "Hello");
        let back = NetworkPacket::deserialize(&packet.serialize()).unwrap();
        assert_eq!(
            NotificationInfo::from_packet(&back),
            NotificationInfo {
                id: "7".into(),
                app: "com.example".into(),
                title: "Ping".into(),
                text: "Hello".into()
            }
        );
    }

    #[test]
    fn missing_fields_default_to_empty() {
        let info = NotificationInfo::from_packet(&NetworkPacket::new(NOTIFICATION));
        assert_eq!(info, NotificationInfo::default());
    }
}
