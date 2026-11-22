//! SMS plugin for KDE Connect.
//!
//! Reading the device SMS database is an Android API concern, so every
//! incoming request is handed to the host through [`kc_core::Platform::on_sms`]. The
//! free functions in this module are the wire-format counterparts used when
//! this device wants to request messages or send a response back.

use kc_core::{Plugin, PluginContext, Result};
use kc_protocol::NetworkPacket;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const TYPE_SMS_REQUEST: &str = "kdeconnect.sms.request";
pub const TYPE_REQUEST_CONVERSATIONS: &str = "kdeconnect.sms.request_conversations";
pub const TYPE_REQUEST_CONVERSATION: &str = "kdeconnect.sms.request_conversation";
pub const TYPE_REQUEST_ATTACHMENT: &str = "kdeconnect.sms.request_attachment";
pub const TYPE_SMS_MESSAGES: &str = "kdeconnect.sms.messages";
pub const TYPE_ATTACHMENT_FILE: &str = "kdeconnect.sms.attachment_file";

/// One SMS/MMS message as exposed by the Android SMS content provider.
///
/// The Rust field names intentionally differ from the wire names where the
/// latter are not valid Rust identifiers; serde keeps the upstream spelling.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SmsMessage {
    pub id: i64,
    pub address: String,
    pub body: String,
    pub date: i64,
    #[serde(rename = "type")]
    pub r#type: i32,
    #[serde(rename = "threadID")]
    pub thread_id: i64,
    pub read: bool,
    #[serde(rename = "subscriptionId")]
    pub subscription_id: i32,
}

#[derive(Default, Debug)]
pub struct SMSPlugin;

/// Ask the peer for the list of conversations (threads).
pub fn request_conversations() -> NetworkPacket {
    NetworkPacket::new(TYPE_REQUEST_CONVERSATIONS)
}

/// Ask the peer for one specific conversation.
pub fn request_conversation(id: i64) -> NetworkPacket {
    let mut packet = NetworkPacket::new(TYPE_REQUEST_CONVERSATION);
    packet.set_i64("conversationID", id);
    packet
}

/// Ask the peer for the messages of a thread. Upstream uses
/// `kdeconnect.sms.request` with an optional `conversationID`; omitting the
/// id requests all messages.
pub fn request_messages(thread_id: i64) -> NetworkPacket {
    let mut packet = NetworkPacket::new(TYPE_SMS_REQUEST);
    packet.set_i64("conversationID", thread_id);
    packet
}

/// Build a `kdeconnect.sms.messages` packet from an already-serialized JSON
/// array of message objects.
pub fn messages_packet(messages_json: Value) -> NetworkPacket {
    let mut packet = NetworkPacket::new(TYPE_SMS_MESSAGES);
    packet.set("messages", messages_json);
    packet
}

/// Typed convenience wrapper around [`messages_packet`].
pub fn messages_packet_from(messages: &[SmsMessage]) -> Result<NetworkPacket> {
    Ok(messages_packet(serde_json::to_value(messages)?))
}

/// Like [`messages_packet`] but adds the optional context fields upstream uses
/// for thread-scoped replies and forwarded/replied messages.
pub fn messages_packet_with_context(
    messages_json: Value,
    conversation_id: Option<i64>,
    original_message: Option<&str>,
) -> NetworkPacket {
    let mut packet = messages_packet(messages_json);
    if let Some(id) = conversation_id {
        packet.set_i64("conversationID", id);
    }
    if let Some(original) = original_message {
        packet.set_str("originalMessage", original);
    }
    packet
}

/// Body of a `kdeconnect.sms.attachment_file` packet. The transport attaches
/// the actual attachment bytes as the packet payload.
pub fn attachment_file_packet(filename: &str, mime_type: &str, unique_id: &str) -> NetworkPacket {
    let mut packet = NetworkPacket::new(TYPE_ATTACHMENT_FILE);
    packet.set_str("filename", filename);
    packet.set_str("mimeType", mime_type);
    packet.set_str("uniqueID", unique_id);
    packet
}

pub fn parse_conversation_id(packet: &NetworkPacket) -> Option<i64> {
    packet.get_i64("conversationID")
}

pub fn parse_attachment_request(packet: &NetworkPacket) -> Option<(String, String)> {
    Some((packet.get_string("partID")?, packet.get_string("uniqueID")?))
}

impl Plugin for SMSPlugin {
    fn key(&self) -> &'static str {
        "SMSPlugin"
    }
    fn display_name(&self) -> &'static str {
        "SMSPlugin"
    }
    fn description(&self) -> &'static str {
        "KDE Connect plugin"
    }
    fn supported_packet_types(&self) -> &'static [&'static str] {
        &[
            "kdeconnect.sms.request",
            "kdeconnect.sms.request_conversations",
            "kdeconnect.sms.request_conversation",
            "kdeconnect.sms.request_attachment",
        ]
    }
    fn outgoing_packet_types(&self) -> &'static [&'static str] {
        &["kdeconnect.sms.messages", "kdeconnect.sms.attachment_file"]
    }

    fn on_packet(&self, ctx: &PluginContext, packet: &NetworkPacket) -> Result<bool> {
        if !matches!(
            packet.ty.as_str(),
            TYPE_SMS_REQUEST
                | TYPE_REQUEST_CONVERSATIONS
                | TYPE_REQUEST_CONVERSATION
                | TYPE_REQUEST_ATTACHMENT
        ) {
            return Ok(false);
        }
        // The host owns the SMS content provider and decides which response
        // packet (if any) to send; see the constructors above.
        ctx.platform.on_sms(&ctx.device_id, packet);
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> SmsMessage {
        SmsMessage {
            id: 7,
            address: "+123456".to_owned(),
            body: "hello".to_owned(),
            date: 1_700_000_000,
            r#type: 1,
            thread_id: 42,
            read: true,
            subscription_id: 2,
        }
    }

    #[test]
    fn sms_message_uses_upstream_field_names() {
        let value = serde_json::to_value(sample()).unwrap();
        assert_eq!(value["type"], 1);
        assert_eq!(value["threadID"], 42);
        assert_eq!(value["subscriptionId"], 2);
        let back: SmsMessage = serde_json::from_value(value).unwrap();
        assert_eq!(back.id, 7);
        assert_eq!(back.thread_id, 42);
        assert_eq!(back.address, "+123456");
        assert!(back.read);
    }

    #[test]
    fn request_constructors_set_expected_body() {
        assert_eq!(request_conversations().ty, TYPE_REQUEST_CONVERSATIONS);
        assert!(request_conversations().body.is_empty());

        let conversation = request_conversation(99);
        assert_eq!(conversation.ty, TYPE_REQUEST_CONVERSATION);
        assert_eq!(conversation.get_i64("conversationID"), Some(99));

        let messages = request_messages(42);
        assert_eq!(messages.ty, TYPE_SMS_REQUEST);
        assert_eq!(messages.get_i64("conversationID"), Some(42));
    }

    #[test]
    fn messages_packet_roundtrips_over_the_wire() {
        let packet = messages_packet_from(&[sample()]).unwrap();
        assert_eq!(packet.ty, TYPE_SMS_MESSAGES);
        let back = NetworkPacket::deserialize(&packet.serialize()).unwrap();
        let messages = back.get_array("messages").unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["address"], "+123456");
        assert_eq!(messages[0]["threadID"], 42);
    }

    #[test]
    fn messages_packet_context_fields_are_optional() {
        let bare = messages_packet_with_context(serde_json::json!([]), None, None);
        assert!(bare.get("conversationID").is_none());
        assert!(bare.get("originalMessage").is_none());

        let scoped =
            messages_packet_with_context(serde_json::json!([]), Some(42), Some("forwarded text"));
        assert_eq!(scoped.get_i64("conversationID"), Some(42));
        assert_eq!(scoped.get_str("originalMessage"), Some("forwarded text"));
    }

    #[test]
    fn attachment_and_parse_helpers() {
        let packet = attachment_file_packet("pic.jpg", "image/jpeg", "u-1");
        assert_eq!(packet.ty, TYPE_ATTACHMENT_FILE);
        assert_eq!(packet.get_str("filename"), Some("pic.jpg"));
        assert_eq!(packet.get_str("mimeType"), Some("image/jpeg"));
        assert_eq!(packet.get_str("uniqueID"), Some("u-1"));

        let mut request = NetworkPacket::new(TYPE_REQUEST_ATTACHMENT);
        request.set_str("partID", "p-1");
        request.set_str("uniqueID", "u-1");
        assert_eq!(
            parse_attachment_request(&request),
            Some(("p-1".to_owned(), "u-1".to_owned()))
        );
    }
}
