//! Plugin trait and registry.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use kc_protocol::NetworkPacket;
use tokio::sync::mpsc;

use crate::{platform::Platform, Result};

/// A packet queued for delivery to one device.
#[derive(Debug, Clone)]
pub struct OutgoingPacket {
    pub device_id: String,
    pub packet: NetworkPacket,
}

/// Per-device, per-plugin runtime context.
#[derive(Clone)]
pub struct PluginContext {
    pub device_id: String,
    pub device_name: String,
    pub platform: Arc<dyn Platform>,
    pub sender: mpsc::UnboundedSender<OutgoingPacket>,
}

/// Rust counterpart of `org.kde.kdeconnect.plugins.Plugin`.
pub trait Plugin: Send + Sync {
    /// Class short name upstream, e.g. `BatteryPlugin`.
    fn key(&self) -> &'static str;
    fn display_name(&self) -> &'static str;
    fn description(&self) -> &'static str;

    /// Packet types this plugin can receive (`supportedPacketTypes`).
    fn supported_packet_types(&self) -> &'static [&'static str];
    /// Packet types this plugin can send (`outgoingPacketTypes`).
    fn outgoing_packet_types(&self) -> &'static [&'static str];

    fn enabled_by_default(&self) -> bool {
        true
    }
    fn listen_to_unpaired(&self) -> bool {
        false
    }

    fn start(&self, _ctx: &PluginContext) -> Result<()> {
        Ok(())
    }
    fn stop(&self, _ctx: &PluginContext) {}

    /// Return `Ok(true)` when the packet was consumed.
    fn on_packet(&self, _ctx: &PluginContext, _packet: &NetworkPacket) -> Result<bool> {
        Ok(false)
    }
}

/// Immutable registry of all compiled-in plugins.
#[derive(Default)]
pub struct PluginRegistry {
    plugins: BTreeMap<&'static str, Arc<dyn Plugin>>,
    by_incoming: BTreeMap<String, Vec<&'static str>>,
}

impl PluginRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, plugin: Arc<dyn Plugin>) {
        for ty in plugin.supported_packet_types() {
            self.by_incoming
                .entry((*ty).to_owned())
                .or_default()
                .push(plugin.key());
        }
        self.plugins.insert(plugin.key(), plugin);
    }

    pub fn get(&self, key: &str) -> Option<Arc<dyn Plugin>> {
        self.plugins.get(key).cloned()
    }

    pub fn all(&self) -> impl Iterator<Item = Arc<dyn Plugin>> + '_ {
        self.plugins.values().cloned()
    }

    pub fn len(&self) -> usize {
        self.plugins.len()
    }

    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }

    /// Plugins that can handle an incoming packet type.
    pub fn handlers_for(&self, packet_type: &str) -> Vec<Arc<dyn Plugin>> {
        self.by_incoming
            .get(packet_type)
            .map(|keys| {
                keys.iter()
                    .filter_map(|k| self.plugins.get(k).cloned())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Union of all incoming / outgoing capabilities, as advertised in the identity packet.
    pub fn capabilities(&self) -> (BTreeSet<String>, BTreeSet<String>) {
        let mut incoming = BTreeSet::new();
        let mut outgoing = BTreeSet::new();
        for plugin in self.plugins.values() {
            incoming.extend(
                plugin
                    .supported_packet_types()
                    .iter()
                    .map(|s| (*s).to_owned()),
            );
            outgoing.extend(
                plugin
                    .outgoing_packet_types()
                    .iter()
                    .map(|s| (*s).to_owned()),
            );
        }
        (incoming, outgoing)
    }
}
