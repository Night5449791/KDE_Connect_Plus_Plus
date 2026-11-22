//! Host callbacks. The daemon implements these for the terminal; the thin
//! Kotlin shell implements them via JNI for Android APIs.

use kc_protocol::{DeviceType, NetworkPacket};

#[derive(Clone, Debug, Default)]
pub struct BatteryStatus {
    pub current_charge: i32,
    pub is_charging: bool,
    pub threshold_event: i32,
}

/// Everything platform-specific that a plugin might need.
///
/// Default implementations are no-ops so headless/test builds need only
/// override the callbacks they care about.
pub trait Platform: Send + Sync + 'static {
    fn device_name(&self) -> String;
    fn device_type(&self) -> DeviceType;

    // clipboard
    fn clipboard_get(&self) -> Option<String> {
        None
    }
    fn clipboard_set(&self, _text: &str) -> bool {
        false
    }

    // simple notifications
    fn on_ping(&self, _device_id: &str, _message: Option<&str>) {}
    fn on_battery(&self, _device_id: &str, _status: &BatteryStatus) {}
    fn on_connectivity(&self, _device_id: &str, _packet: &NetworkPacket) {}

    // packets that need a UI / system service on the host
    fn on_clipboard(&self, _device_id: &str, _content: &str) {}
    fn on_notification(&self, _device_id: &str, _packet: &NetworkPacket) {}
    fn on_mpris(&self, _device_id: &str, _packet: &NetworkPacket) {}
    fn on_system_volume(&self, _device_id: &str, _packet: &NetworkPacket) {}
    fn on_telephony(&self, _device_id: &str, _packet: &NetworkPacket) {}
    fn on_sms(&self, _device_id: &str, _packet: &NetworkPacket) {}
    fn on_share(&self, _device_id: &str, _packet: &NetworkPacket) {}
    fn on_run_command(&self, _device_id: &str, _packet: &NetworkPacket) {}
    fn on_find_my_phone(&self, _device_id: &str) {}
    fn on_file_transfer(&self, _device_id: &str, _packet: &NetworkPacket) {}

    // pairing
    fn on_pairing_request(&self, _device_id: &str, _verification_key: &str) {}
}

/// Minimal platform used by tests and the CLI when no host is supplied.
pub struct NoopPlatform;

impl Platform for NoopPlatform {
    fn device_name(&self) -> String {
        "KDE Connect Rust".to_owned()
    }

    fn device_type(&self) -> DeviceType {
        DeviceType::Desktop
    }
}
