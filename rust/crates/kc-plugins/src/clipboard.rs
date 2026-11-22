//! `ClipboardPlugin`: clipboard synchronization with a paired device.
//!
//! A short queue of recently received content hashes is kept so that text
//! coming from the remote side is not echoed straight back when the host
//! reads its clipboard.

use std::collections::hash_map::DefaultHasher;
use std::collections::VecDeque;
use std::hash::{Hash, Hasher};
use std::sync::Mutex;

use kc_core::{Platform, Plugin, PluginContext, Result};
use kc_protocol::NetworkPacket;

pub const PACKET_TYPE_CLIPBOARD: &str = "kdeconnect.clipboard";
pub const PACKET_TYPE_CLIPBOARD_CONNECT: &str = "kdeconnect.clipboard.connect";
/// Number of recent content hashes retained for echo suppression.
const RECENT_CAPACITY: usize = 8;

#[derive(Default, Debug)]
pub struct ClipboardPlugin {
    recent: Mutex<VecDeque<u64>>,
    last_timestamp: Mutex<Option<i64>>,
}

fn hash_text(text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

impl ClipboardPlugin {
    /// Whether `text` was seen recently (used for echo suppression).
    pub fn is_recent(&self, text: &str) -> bool {
        let hash = hash_text(text);
        self.recent
            .lock()
            .map(|queue| queue.contains(&hash))
            .unwrap_or(false)
    }

    /// Add `text` to the recent-content queue, evicting the oldest entry.
    pub fn remember(&self, text: &str) {
        let hash = hash_text(text);
        if let Ok(mut queue) = self.recent.lock() {
            queue.retain(|&h| h != hash);
            queue.push_back(hash);
            while queue.len() > RECENT_CAPACITY {
                queue.pop_front();
            }
        }
    }

    /// Accept `timestamp` unless it is a known, older timestamp.
    /// `0` means "unknown" and is always accepted.
    pub fn accept_timestamp(&self, timestamp: i64) -> bool {
        let mut last = self
            .last_timestamp
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if timestamp != 0 {
            if let Some(previous) = *last {
                if timestamp < previous {
                    return false;
                }
            }
            *last = Some(timestamp);
        }
        true
    }

    fn deliver(&self, content: &str, device_id: &str, platform: &dyn Platform) {
        self.remember(content);
        platform.on_clipboard(device_id, content);
    }

    pub(crate) fn handle(
        &self,
        packet: &NetworkPacket,
        device_id: &str,
        platform: &dyn Platform,
    ) -> bool {
        match packet.ty.as_str() {
            PACKET_TYPE_CLIPBOARD => {
                let Some(content) = packet.get_str("content") else {
                    return false;
                };
                self.deliver(content, device_id, platform);
                true
            }
            PACKET_TYPE_CLIPBOARD_CONNECT => {
                let timestamp = packet.get_i64("timestamp").unwrap_or(0);
                if !self.accept_timestamp(timestamp) {
                    return false;
                }
                let Some(content) = packet.get_str("content") else {
                    return false;
                };
                self.deliver(content, device_id, platform);
                true
            }
            _ => false,
        }
    }
}

impl Plugin for ClipboardPlugin {
    fn key(&self) -> &'static str {
        "ClipboardPlugin"
    }
    fn display_name(&self) -> &'static str {
        "Clipboard"
    }
    fn description(&self) -> &'static str {
        "Synchronize the clipboard with a paired device"
    }
    fn supported_packet_types(&self) -> &'static [&'static str] {
        &[PACKET_TYPE_CLIPBOARD, PACKET_TYPE_CLIPBOARD_CONNECT]
    }
    fn outgoing_packet_types(&self) -> &'static [&'static str] {
        &[PACKET_TYPE_CLIPBOARD, PACKET_TYPE_CLIPBOARD_CONNECT]
    }

    fn on_packet(&self, ctx: &PluginContext, packet: &NetworkPacket) -> Result<bool> {
        Ok(self.handle(packet, &ctx.device_id, ctx.platform.as_ref()))
    }
}

/// Build a plain `kdeconnect.clipboard` packet.
pub fn clipboard_packet(content: &str) -> NetworkPacket {
    let mut packet = NetworkPacket::new(PACKET_TYPE_CLIPBOARD);
    packet.set_str("content", content);
    packet
}

/// Build a `kdeconnect.clipboard.connect` packet. `timestamp` is epoch
/// milliseconds (0 means unknown).
pub fn clipboard_connect_packet(timestamp: i64, content: &str) -> NetworkPacket {
    let mut packet = NetworkPacket::new(PACKET_TYPE_CLIPBOARD_CONNECT);
    packet.set_i64("timestamp", timestamp);
    packet.set_str("content", content);
    packet
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    use kc_protocol::DeviceType;

    use super::*;

    #[derive(Default)]
    struct RecordingPlatform {
        clipboards: AtomicUsize,
        received: Mutex<Vec<String>>,
    }

    impl Platform for RecordingPlatform {
        fn device_name(&self) -> String {
            "test".to_owned()
        }
        fn device_type(&self) -> DeviceType {
            DeviceType::Phone
        }

        fn on_clipboard(&self, _device_id: &str, content: &str) {
            self.clipboards.fetch_add(1, Ordering::SeqCst);
            self.received.lock().unwrap().push(content.to_owned());
        }
    }

    #[test]
    fn clipboard_packets_roundtrip_through_wire() {
        let plain = clipboard_packet("hello");
        let back = NetworkPacket::deserialize(&plain.serialize()).unwrap();
        assert_eq!(back.ty, PACKET_TYPE_CLIPBOARD);
        assert_eq!(back.get_string("content").as_deref(), Some("hello"));

        let connect = clipboard_connect_packet(1_700_000_000_123, "world");
        let back = NetworkPacket::deserialize(&connect.serialize()).unwrap();
        assert_eq!(back.ty, PACKET_TYPE_CLIPBOARD_CONNECT);
        assert_eq!(back.get_i64("timestamp"), Some(1_700_000_000_123));
        assert_eq!(back.get_string("content").as_deref(), Some("world"));
    }

    #[test]
    fn recent_hash_queue_suppresses_echo() {
        let plugin = ClipboardPlugin::default();
        plugin.remember("secret");
        assert!(plugin.is_recent("secret"));
        assert!(!plugin.is_recent("other"));

        for i in 0..RECENT_CAPACITY + 2 {
            plugin.remember(&format!("item-{i}"));
        }
        assert!(!plugin.is_recent("secret"));
        assert!(plugin.is_recent(&format!("item-{}", RECENT_CAPACITY + 1)));
    }

    #[test]
    fn stale_timestamps_are_ignored_but_unknown_is_allowed() {
        let plugin = ClipboardPlugin::default();
        assert!(plugin.accept_timestamp(100));
        assert!(!plugin.accept_timestamp(50));
        assert!(plugin.accept_timestamp(0));
        assert!(plugin.accept_timestamp(150));
        assert!(!plugin.accept_timestamp(149));
    }

    #[test]
    fn handle_delivers_both_clipboard_formats() {
        let plugin = ClipboardPlugin::default();
        let platform = RecordingPlatform::default();

        assert!(plugin.handle(&clipboard_packet("a"), "dev", &platform));
        let fresh = clipboard_connect_packet(1_700_000_000_000, "b");
        assert!(plugin.handle(&fresh, "dev", &platform));
        let stale = clipboard_connect_packet(1_699_999_999_999, "c");
        assert!(!plugin.handle(&stale, "dev", &platform));

        assert_eq!(platform.clipboards.load(Ordering::SeqCst), 2);
        assert_eq!(*platform.received.lock().unwrap(), vec!["a", "b"]);
        assert!(plugin.is_recent("a"));
        assert!(plugin.is_recent("b"));
        assert!(!plugin.is_recent("c"));
    }
}
