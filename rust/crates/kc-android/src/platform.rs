//! `kc_core::Platform` implementation backed entirely by the stateless
//! `KcBridge` Kotlin object. Every callback is forwarded through [`bridge`],
//! which attaches the current thread to the JVM and never retains a `JNIEnv`.
//!
//! * getters pull `deviceName()`, `deviceType()`, `clipboardText()`,
//!   `setClipboardText()` from Kotlin;
//! * notifications/toasts go to `notify()`;
//! * clipboard writes go to `onClipboardChanged()`;
//! * pairing uses the JSON event `{"kind":"pairing_request",...}`;
//! * all other plugin callbacks use `onEvent()` with a `kind` field.

use std::sync::{Arc, Mutex};

use kc_core::{BatteryStatus, Platform};
use kc_protocol::{DeviceType, NetworkPacket};
use serde_json::{json, Value};

use crate::bridge;
use crate::echo::ClipboardEcho;

/// Map Kotlin's `deviceType()` (0=phone, 1=tablet, 2=desktop, 3=laptop, 4=tv)
/// to the protocol enum. Unknown values fall back to `Phone`.
pub fn device_type_from_code(code: i32) -> DeviceType {
    match code {
        0 => DeviceType::Phone,
        1 => DeviceType::Tablet,
        2 => DeviceType::Desktop,
        3 => DeviceType::Laptop,
        4 => DeviceType::Tv,
        _ => DeviceType::Phone,
    }
}

/// Build the common event envelope, merging optional extra fields.
fn event_json(kind: &str, device_id: &str, extra: Value) -> String {
    let mut value = json!({ "kind": kind, "deviceId": device_id });
    if let (Some(object), Some(extra_object)) = (value.as_object_mut(), extra.as_object()) {
        for (key, item) in extra_object {
            object.insert(key.clone(), item.clone());
        }
    }
    value.to_string()
}

/// Packet-based plugin callbacks keep the packet type and body for Kotlin to
/// display; Kotlin never parses KDE Connect packets itself, it only shows the
/// JSON payload provided by Rust.
fn packet_event_json(kind: &str, device_id: &str, packet: &NetworkPacket) -> String {
    event_json(
        kind,
        device_id,
        json!({ "packetType": packet.ty, "body": packet.body.clone() }),
    )
}

pub struct AndroidPlatform {
    /// Last value successfully read from Kotlin, used before the JVM is ready.
    device_name: Mutex<String>,
    echo: Arc<ClipboardEcho>,
}

impl AndroidPlatform {
    pub fn new(device_name: String, echo: Arc<ClipboardEcho>) -> Self {
        Self {
            device_name: Mutex::new(device_name),
            echo,
        }
    }
}

impl Platform for AndroidPlatform {
    fn device_name(&self) -> String {
        match bridge::device_name() {
            Some(name) if !name.is_empty() => {
                if let Ok(mut cache) = self.device_name.lock() {
                    *cache = name.clone();
                }
                name
            }
            _ => self
                .device_name
                .lock()
                .map(|guard| guard.clone())
                .unwrap_or_else(|_| "Android".to_owned()),
        }
    }

    fn device_type(&self) -> DeviceType {
        bridge::device_type()
            .map(device_type_from_code)
            .unwrap_or(DeviceType::Phone)
    }

    fn clipboard_get(&self) -> Option<String> {
        let text = bridge::clipboard_text()?;
        // Never echo text we just sent back to the remote device.
        if text.is_empty() || self.echo.is_recent(&text) {
            None
        } else {
            Some(text)
        }
    }

    fn clipboard_set(&self, text: &str) -> bool {
        bridge::set_clipboard_text(text).unwrap_or(false)
    }

    fn on_ping(&self, device_id: &str, message: Option<&str>) {
        let body = message.unwrap_or("");
        let text = if body.is_empty() {
            format!("Ping from {device_id}")
        } else {
            format!("Ping from {device_id}: {body}")
        };
        bridge::notify("KDE Connect", &text);
    }

    fn on_battery(&self, device_id: &str, status: &BatteryStatus) {
        bridge::on_event(&event_json(
            "battery",
            device_id,
            json!({
                "currentCharge": status.current_charge,
                "isCharging": status.is_charging,
                "thresholdEvent": status.threshold_event,
            }),
        ));
    }

    fn on_connectivity(&self, device_id: &str, packet: &NetworkPacket) {
        bridge::on_event(&packet_event_json("connectivity", device_id, packet));
    }

    fn on_clipboard(&self, _device_id: &str, content: &str) {
        // A packet carrying text we just sent is an echo; drop it. Otherwise
        // remember it so a later clipboard_read does not send it straight back.
        if self.echo.is_recent(content) {
            return;
        }
        self.echo.remember(content);
        bridge::on_clipboard_changed(content);
    }

    fn on_notification(&self, device_id: &str, packet: &NetworkPacket) {
        // Remote notification -> local notification/toast via stateless adapter.
        let title = packet.get_str("title").unwrap_or("KDE Connect");
        let body = packet
            .get_str("body")
            .or_else(|| packet.get_str("text"))
            .unwrap_or("");
        let message = if body.is_empty() {
            format!("Notification from {device_id}")
        } else {
            format!("{device_id}: {body}")
        };
        bridge::notify(title, &message);
    }

    fn on_mpris(&self, device_id: &str, packet: &NetworkPacket) {
        bridge::on_event(&packet_event_json("mpris", device_id, packet));
    }

    fn on_system_volume(&self, device_id: &str, packet: &NetworkPacket) {
        bridge::on_event(&packet_event_json("system_volume", device_id, packet));
    }

    fn on_telephony(&self, device_id: &str, packet: &NetworkPacket) {
        bridge::on_event(&packet_event_json("telephony", device_id, packet));
    }

    fn on_sms(&self, device_id: &str, packet: &NetworkPacket) {
        bridge::on_event(&packet_event_json("sms", device_id, packet));
    }

    fn on_share(&self, device_id: &str, packet: &NetworkPacket) {
        bridge::on_event(&packet_event_json("share", device_id, packet));
    }

    fn on_run_command(&self, device_id: &str, packet: &NetworkPacket) {
        bridge::on_event(&packet_event_json("run_command", device_id, packet));
    }

    fn on_find_my_phone(&self, device_id: &str) {
        bridge::on_event(&event_json("find_my_phone", device_id, json!({})));
    }

    fn on_file_transfer(&self, device_id: &str, packet: &NetworkPacket) {
        bridge::on_event(&packet_event_json("file_transfer", device_id, packet));
    }

    fn on_pairing_request(&self, device_id: &str, verification_key: &str) {
        bridge::on_event(&event_json(
            "pairing_request",
            device_id,
            json!({ "verificationKey": verification_key }),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kotlin_device_type_codes_map_to_protocol_enum() {
        assert_eq!(device_type_from_code(0), DeviceType::Phone);
        assert_eq!(device_type_from_code(1), DeviceType::Tablet);
        assert_eq!(device_type_from_code(2), DeviceType::Desktop);
        assert_eq!(device_type_from_code(3), DeviceType::Laptop);
        assert_eq!(device_type_from_code(4), DeviceType::Tv);
        assert_eq!(device_type_from_code(99), DeviceType::Phone);
    }

    #[test]
    fn pairing_event_matches_contract_shape() {
        let json = event_json(
            "pairing_request",
            "device-42",
            json!({ "verificationKey": "AB12CD34" }),
        );
        let value: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["kind"], "pairing_request");
        assert_eq!(value["deviceId"], "device-42");
        assert_eq!(value["verificationKey"], "AB12CD34");
    }

    #[test]
    fn every_plugin_event_has_a_kind_field() {
        let packet = NetworkPacket::new("kdeconnect.telephony");
        for kind in [
            "connectivity",
            "mpris",
            "system_volume",
            "telephony",
            "sms",
            "share",
            "run_command",
            "file_transfer",
            "find_my_phone",
        ] {
            let json = if kind == "find_my_phone" {
                event_json(kind, "d", json!({}))
            } else {
                packet_event_json(kind, "d", &packet)
            };
            let value: Value = serde_json::from_str(&json).unwrap();
            assert_eq!(value["kind"], kind);
            assert_eq!(value["deviceId"], "d");
        }
    }

    #[test]
    fn platform_without_vm_keeps_fallbacks_and_suppresses_echo() {
        let platform =
            AndroidPlatform::new("Fallback".to_owned(), Arc::new(ClipboardEcho::default()));
        assert_eq!(platform.device_name(), "Fallback");
        assert_eq!(platform.device_type(), DeviceType::Phone);
        assert_eq!(platform.clipboard_get(), None);
        assert!(!platform.clipboard_set("x"));

        // These calls must be silent no-ops with no JVM installed.
        platform.on_ping("d", Some("hi"));
        platform.on_pairing_request("d", "KEY");
        platform.on_find_my_phone("d");
        platform.on_battery("d", &BatteryStatus::default());
        let packet = NetworkPacket::new("kdeconnect.notification");
        platform.on_notification("d", &packet);
        platform.on_clipboard("d", "text");
    }
}
