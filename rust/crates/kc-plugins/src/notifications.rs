//! Notifications plugin: asks the peer for notifications and handles the
//! replies/actions coming back from it.

use kc_core::{Plugin, PluginContext, Result};
use kc_protocol::NetworkPacket;

const NOTIFICATION_REQUEST: &str = "kdeconnect.notification.request";
const NOTIFICATION_REPLY: &str = "kdeconnect.notification.reply";
const NOTIFICATION_ACTION: &str = "kdeconnect.notification.action";

/// Build a `kdeconnect.notification` packet describing one notification.
pub fn notification_packet(
    id: impl Into<String>,
    app: impl Into<String>,
    title: impl Into<String>,
    text: impl Into<String>,
) -> NetworkPacket {
    let mut packet = NetworkPacket::new("kdeconnect.notification");
    packet.set_str("id", id);
    packet.set_str("app", app);
    packet.set_str("title", title);
    packet.set_str("text", text);
    packet
}

/// Reply to a notification that supports inline replies.
pub fn reply_packet(id: impl Into<String>, text: impl Into<String>) -> NetworkPacket {
    let mut packet = NetworkPacket::new(NOTIFICATION_REPLY);
    packet.set_str("id", id);
    packet.set_str("text", text);
    packet
}

/// Trigger a custom action of a notification.
pub fn action_packet(id: impl Into<String>, action: impl Into<String>) -> NetworkPacket {
    let mut packet = NetworkPacket::new(NOTIFICATION_ACTION);
    packet.set_str("id", id);
    packet.set_str("action", action);
    packet
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NotificationReply {
    pub id: String,
    pub text: String,
}

impl NotificationReply {
    pub fn from_packet(packet: &NetworkPacket) -> Option<Self> {
        Some(Self {
            id: packet.get_string("id")?,
            text: packet.get_string("text")?,
        })
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NotificationAction {
    pub id: String,
    pub action: String,
}

impl NotificationAction {
    pub fn from_packet(packet: &NetworkPacket) -> Option<Self> {
        Some(Self {
            id: packet.get_string("id")?,
            action: packet.get_string("action")?,
        })
    }
}

#[derive(Default, Debug)]
pub struct NotificationsPlugin;

impl Plugin for NotificationsPlugin {
    fn key(&self) -> &'static str {
        "NotificationsPlugin"
    }
    fn display_name(&self) -> &'static str {
        "Notifications"
    }
    fn description(&self) -> &'static str {
        "Send notifications and handle remote replies and actions"
    }
    fn supported_packet_types(&self) -> &'static [&'static str] {
        &[
            NOTIFICATION_REQUEST,
            NOTIFICATION_REPLY,
            NOTIFICATION_ACTION,
        ]
    }
    fn outgoing_packet_types(&self) -> &'static [&'static str] {
        &["kdeconnect.notification"]
    }

    fn on_packet(&self, ctx: &PluginContext, packet: &NetworkPacket) -> Result<bool> {
        match packet.ty.as_str() {
            NOTIFICATION_REQUEST => {
                // The request body may be empty. The host enumerates the
                // current notifications and sends `kdeconnect.notification`
                // packets using `notification_packet`.
                ctx.platform.on_notification(&ctx.device_id, packet);
                Ok(true)
            }
            NOTIFICATION_REPLY => {
                if NotificationReply::from_packet(packet).is_none() {
                    return Ok(false);
                }
                ctx.platform.on_notification(&ctx.device_id, packet);
                Ok(true)
            }
            NOTIFICATION_ACTION => {
                if NotificationAction::from_packet(packet).is_none() {
                    return Ok(false);
                }
                ctx.platform.on_notification(&ctx.device_id, packet);
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notification_constructor_roundtrips() {
        let packet = notification_packet("42", "org.example.app", "Hello", "World");
        let back = NetworkPacket::deserialize(&packet.serialize()).unwrap();
        assert_eq!(back.ty, "kdeconnect.notification");
        assert_eq!(back.get_str("id"), Some("42"));
        assert_eq!(back.get_str("app"), Some("org.example.app"));
        assert_eq!(back.get_str("title"), Some("Hello"));
        assert_eq!(back.get_str("text"), Some("World"));
    }

    #[test]
    fn reply_and_action_constructors_roundtrip() {
        let reply = reply_packet("42", "OK");
        let back = NetworkPacket::deserialize(&reply.serialize()).unwrap();
        assert_eq!(back.ty, NOTIFICATION_REPLY);
        assert_eq!(
            NotificationReply::from_packet(&back),
            Some(NotificationReply {
                id: "42".into(),
                text: "OK".into()
            })
        );

        let action = action_packet("42", "reply");
        let back = NetworkPacket::deserialize(&action.serialize()).unwrap();
        assert_eq!(back.ty, NOTIFICATION_ACTION);
        assert_eq!(
            NotificationAction::from_packet(&back),
            Some(NotificationAction {
                id: "42".into(),
                action: "reply".into()
            })
        );
    }

    #[test]
    fn incomplete_reply_or_action_is_rejected() {
        let mut packet = NetworkPacket::new(NOTIFICATION_REPLY);
        packet.set_str("id", "42");
        assert_eq!(NotificationReply::from_packet(&packet), None);

        let mut packet = NetworkPacket::new(NOTIFICATION_ACTION);
        packet.set_str("action", "dismiss");
        assert_eq!(NotificationAction::from_packet(&packet), None);
    }

    #[test]
    fn empty_request_has_no_required_fields() {
        let request = NetworkPacket::new(NOTIFICATION_REQUEST);
        let back = NetworkPacket::deserialize(&request.serialize()).unwrap();
        assert!(back.body.is_empty());
    }
}
