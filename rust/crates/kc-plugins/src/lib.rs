//! All 24 plugins from KDE Connect Android 1.37.3, implemented against the
//! `kc_core::Plugin` trait. Each module owns one plugin and its packet types.

use std::sync::Arc;

use kc_core::{Plugin, PluginRegistry};

pub mod battery;
pub mod clipboard;
pub mod connectivityreport;
pub mod contacts;
pub mod digitizer;
pub mod findmyphone;
pub mod findremotedevice;
pub mod inputdevicesreceiver;
pub mod mousepad;
pub mod mousereceiver;
pub mod mpris;
pub mod mprisreceiver;
pub mod notifications;
pub mod ping;
pub mod presenter;
pub mod receivenotifications;
pub mod remotekeyboard;
pub mod remotekeyboardime;
pub mod runcommand;
pub mod sftp;
pub mod share;
pub mod sms;
pub mod systemvolume;
pub mod telephony;

/// Build the default registry, mirroring `PluginFactory.initPluginInfo()`.
pub fn default_registry() -> Arc<PluginRegistry> {
    let mut registry = PluginRegistry::new();
    macro_rules! reg {
        ($ty:ty) => {
            registry.register(Arc::new(<$ty>::default()) as Arc<dyn Plugin>);
        };
    }
    reg!(battery::BatteryPlugin);
    reg!(clipboard::ClipboardPlugin);
    reg!(connectivityreport::ConnectivityReportPlugin);
    reg!(contacts::ContactsPlugin);
    reg!(digitizer::DigitizerPlugin);
    reg!(findmyphone::FindMyPhonePlugin);
    reg!(findremotedevice::FindRemoteDevicePlugin);
    reg!(inputdevicesreceiver::InputDevicesReceiver);
    reg!(mousereceiver::MouseReceiverPlugin);
    reg!(mousepad::MousePadPlugin);
    reg!(mpris::MprisPlugin);
    reg!(mprisreceiver::MprisReceiverPlugin);
    reg!(notifications::NotificationsPlugin);
    reg!(ping::PingPlugin);
    reg!(presenter::PresenterPlugin);
    reg!(receivenotifications::ReceiveNotificationsPlugin);
    reg!(remotekeyboard::RemoteKeyboardPlugin);
    reg!(remotekeyboardime::RemoteKeyboardIMEPlugin);
    reg!(runcommand::RunCommandPlugin);
    reg!(sftp::SftpPlugin);
    reg!(share::SharePlugin);
    reg!(sms::SMSPlugin);
    reg!(systemvolume::SystemVolumePlugin);
    reg!(telephony::TelephonyPlugin);
    Arc::new(registry)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_contains_all_upstream_plugins() {
        let registry = default_registry();
        assert_eq!(registry.len(), 24);
        let (incoming, outgoing) = registry.capabilities();
        assert!(incoming.contains("kdeconnect.ping"));
        assert!(outgoing.contains("kdeconnect.battery"));
        assert!(!registry.handlers_for("kdeconnect.ping").is_empty());
    }
}
