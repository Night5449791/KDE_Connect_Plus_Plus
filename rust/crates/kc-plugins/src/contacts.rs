//! Contacts plugin for KDE Connect.
//!
//! Answers the two upstream contacts queries. There is no dedicated
//! `Platform` callback for the address book, so the host populates the
//! plugin's in-memory cache via [`ContactsPlugin::set_contacts`] and
//! [`ContactsPlugin::set_vcard`]; an empty cache yields empty responses.

use std::collections::BTreeMap;
use std::sync::Mutex;

use kc_core::{Plugin, PluginContext, Result};
use kc_protocol::NetworkPacket;
use serde::{Deserialize, Serialize};
use serde_json::Value;

const TYPE_REQUEST_UIDS: &str = "kdeconnect.contacts.request_all_uids_timestamps";
const TYPE_RESPONSE_UIDS: &str = "kdeconnect.contacts.response_uids_timestamps";
const TYPE_REQUEST_VCARDS: &str = "kdeconnect.contacts.request_vcards_by_uid";
const TYPE_RESPONSE_VCARDS: &str = "kdeconnect.contacts.response_vcards";

/// A contact identifier and the last time its vCard changed.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContactStamp {
    pub uid: String,
    pub timestamp: i64,
}

#[derive(Default, Debug)]
pub struct ContactsPlugin {
    stamps: Mutex<Vec<ContactStamp>>,
    vcards: Mutex<BTreeMap<String, String>>,
}

impl ContactsPlugin {
    /// Replace the uid/timestamp snapshot returned to the peer.
    pub fn set_contacts(&self, stamps: Vec<ContactStamp>) {
        *self.stamps.lock().unwrap_or_else(|e| e.into_inner()) = stamps;
    }

    pub fn contacts(&self) -> Vec<ContactStamp> {
        self.stamps
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Register the vCard for one contact uid.
    pub fn set_vcard(&self, uid: impl Into<String>, vcard: impl Into<String>) {
        self.vcards
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(uid.into(), vcard.into());
    }

    pub fn vcard(&self, uid: &str) -> Option<String> {
        self.vcards
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(uid)
            .cloned()
    }
}

/// Ask the peer for all contact uids and their modification timestamps.
pub fn request_all_uids_timestamps() -> NetworkPacket {
    NetworkPacket::new(TYPE_REQUEST_UIDS)
}

/// Ask the peer for the vCards of the given uids.
pub fn request_vcards_by_uid(uids: &[String]) -> NetworkPacket {
    let mut packet = NetworkPacket::new(TYPE_REQUEST_VCARDS);
    packet.set("uids", uids.to_vec());
    packet
}

/// Response body: `uids` is a JSON array of `{"uid": .., "timestamp": ..}`.
pub fn response_uids_timestamps(stamps: &[ContactStamp]) -> NetworkPacket {
    let entries: Vec<Value> = stamps
        .iter()
        .map(|stamp| {
            serde_json::json!({
                "uid": stamp.uid,
                "timestamp": stamp.timestamp,
            })
        })
        .collect();
    let mut packet = NetworkPacket::new(TYPE_RESPONSE_UIDS);
    packet.set("uids", entries);
    packet
}

/// Response body: `uids` and `vcards` are parallel String arrays.
pub fn response_vcards(uids: &[String], vcards: &[String]) -> NetworkPacket {
    let mut packet = NetworkPacket::new(TYPE_RESPONSE_VCARDS);
    packet.set("uids", uids.to_vec());
    packet.set("vcards", vcards.to_vec());
    packet
}

pub fn parse_uids(packet: &NetworkPacket) -> Vec<String> {
    packet.get_string_list("uids").unwrap_or_default()
}

fn send(ctx: &PluginContext, packet: NetworkPacket) -> Result<()> {
    ctx.sender
        .send(kc_core::OutgoingPacket {
            device_id: ctx.device_id.clone(),
            packet,
        })
        .map_err(|_| kc_core::CoreError::Transport("outgoing channel closed".to_owned()))
}

impl Plugin for ContactsPlugin {
    fn key(&self) -> &'static str {
        "ContactsPlugin"
    }
    fn display_name(&self) -> &'static str {
        "ContactsPlugin"
    }
    fn description(&self) -> &'static str {
        "KDE Connect plugin"
    }
    fn supported_packet_types(&self) -> &'static [&'static str] {
        &[
            "kdeconnect.contacts.request_all_uids_timestamps",
            "kdeconnect.contacts.request_vcards_by_uid",
        ]
    }
    fn outgoing_packet_types(&self) -> &'static [&'static str] {
        &[
            "kdeconnect.contacts.response_uids_timestamps",
            "kdeconnect.contacts.response_vcards",
        ]
    }

    fn on_packet(&self, ctx: &PluginContext, packet: &NetworkPacket) -> Result<bool> {
        match packet.ty.as_str() {
            TYPE_REQUEST_UIDS => {
                send(ctx, response_uids_timestamps(&self.contacts()))?;
                Ok(true)
            }
            TYPE_REQUEST_VCARDS => {
                let uids = parse_uids(packet);
                let vcards: Vec<String> = uids
                    .iter()
                    .map(|uid| self.vcard(uid).unwrap_or_default())
                    .collect();
                send(ctx, response_vcards(&uids, &vcards))?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stamp(uid: &str, timestamp: i64) -> ContactStamp {
        ContactStamp {
            uid: uid.to_owned(),
            timestamp,
        }
    }

    #[test]
    fn contact_stamp_serde_roundtrip() {
        let value = serde_json::to_value(stamp("a", 123)).unwrap();
        assert_eq!(value["uid"], "a");
        assert_eq!(value["timestamp"], 123);
        let back: ContactStamp = serde_json::from_value(value).unwrap();
        assert_eq!(back.uid, "a");
        assert_eq!(back.timestamp, 123);
    }

    #[test]
    fn requests_set_expected_types_and_body() {
        let uids = vec!["a".to_owned(), "b".to_owned()];
        let request = request_vcards_by_uid(&uids);
        assert_eq!(request.ty, TYPE_REQUEST_VCARDS);
        assert_eq!(request.get_string_list("uids"), Some(uids));
        assert_eq!(request_all_uids_timestamps().ty, TYPE_REQUEST_UIDS);
    }

    #[test]
    fn response_uids_timestamps_body_shape() {
        let packet = response_uids_timestamps(&[stamp("a", 1), stamp("b", 2)]);
        assert_eq!(packet.ty, TYPE_RESPONSE_UIDS);
        let back = NetworkPacket::deserialize(&packet.serialize()).unwrap();
        let entries = back.get_array("uids").unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0]["uid"], "a");
        assert_eq!(entries[1]["timestamp"], 2);
    }

    #[test]
    fn response_vcards_keeps_uid_and_vcard_order_aligned() {
        let uids = vec!["a".to_owned(), "b".to_owned()];
        let cards = vec!["BEGIN:VCARD:a".to_owned(), "BEGIN:VCARD:b".to_owned()];
        let packet = response_vcards(&uids, &cards);
        assert_eq!(packet.ty, TYPE_RESPONSE_VCARDS);
        assert_eq!(
            packet.get_string_list("uids"),
            Some(vec!["a".to_owned(), "b".to_owned()])
        );
        assert_eq!(
            packet.get_string_list("vcards"),
            Some(vec!["BEGIN:VCARD:a".to_owned(), "BEGIN:VCARD:b".to_owned()])
        );
    }

    #[test]
    fn cache_and_parse_helpers() {
        let plugin = ContactsPlugin::default();
        plugin.set_contacts(vec![stamp("a", 9)]);
        plugin.set_vcard("a", "vcard-a");
        assert_eq!(plugin.contacts()[0].timestamp, 9);
        assert_eq!(plugin.vcard("a").as_deref(), Some("vcard-a"));
        assert_eq!(plugin.vcard("missing"), None);

        let mut request = NetworkPacket::new(TYPE_REQUEST_VCARDS);
        request.set("uids", vec!["a".to_owned(), "b".to_owned()]);
        assert_eq!(parse_uids(&request), vec!["a".to_owned(), "b".to_owned()]);
    }
}
