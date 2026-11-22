//! One remote device: links, pairing state, plugin routing.

use std::collections::BTreeSet;
use std::sync::Arc;

use kc_protocol::{
    cert_der_from_pem, parse_certificate_der, DeviceInfo, NetworkPacket, PairState, PairingHandler,
};
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, warn};

use crate::manager::{DeviceEvent, DeviceShared};
use crate::{platform::Platform, plugin::PluginContext, CoreError, Link, Result};

#[derive(Clone, Debug)]
pub struct DeviceSnapshot {
    pub device_id: String,
    pub name: String,
    pub device_type: String,
    pub protocol_version: u32,
    pub paired: bool,
    pub reachable: bool,
    pub link_count: usize,
    pub loaded_plugins: Vec<String>,
}

pub struct Device {
    info: RwLock<DeviceInfo>,
    pairing: Mutex<PairingHandler>,
    links: Mutex<Vec<Arc<dyn Link>>>,
    plugin_keys: Mutex<Vec<&'static str>>,
    peer_cert_der: Vec<u8>,
    shared: Arc<DeviceShared>,
}

impl std::fmt::Debug for Device {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Device")
            .field(
                "device_id",
                &self.info.try_read().map(|i| i.device_id.clone()),
            )
            .finish_non_exhaustive()
    }
}

impl Device {
    pub fn new(
        info: DeviceInfo,
        my_public_key: Vec<u8>,
        peer_public_key: Vec<u8>,
        peer_cert_der: Vec<u8>,
        paired: bool,
        shared: Arc<DeviceShared>,
    ) -> Self {
        let pairing = if paired {
            PairingHandler::paired(my_public_key, peer_public_key)
        } else {
            let mut handler = PairingHandler::new(my_public_key);
            handler.set_peer_public_key(peer_public_key);
            handler
        };
        Self {
            info: RwLock::new(info),
            pairing: Mutex::new(pairing),
            links: Mutex::new(Vec::new()),
            plugin_keys: Mutex::new(Vec::new()),
            peer_cert_der,
            shared,
        }
    }

    pub fn device_id(&self) -> String {
        self.info
            .try_read()
            .map(|i| i.device_id.clone())
            .unwrap_or_default()
    }

    pub async fn info(&self) -> DeviceInfo {
        self.info.read().await.clone()
    }

    pub async fn snapshot(&self) -> DeviceSnapshot {
        let info = self.info.read().await;
        let keys = self.plugin_keys.lock().await;
        let paired = self.pairing.lock().await.is_paired();
        // Do not lock `links` twice in one struct literal: temporaries live
        // until the end of the enclosing statement and would deadlock.
        let link_count = self.links.lock().await.len();
        DeviceSnapshot {
            device_id: info.device_id.clone(),
            name: info.device_name.clone(),
            device_type: info.device_type.as_str().to_owned(),
            protocol_version: info.protocol_version,
            paired,
            reachable: link_count > 0,
            link_count,
            loaded_plugins: keys.iter().map(|k| (*k).to_owned()).collect(),
        }
    }

    pub async fn is_paired(&self) -> bool {
        self.pairing.lock().await.is_paired()
    }

    pub async fn pairing_state(&self) -> PairState {
        self.pairing.lock().await.state()
    }

    pub async fn is_reachable(&self) -> bool {
        !self.links.lock().await.is_empty()
    }

    /// Update the advertised capabilities and reload the enabled plugin set.
    pub async fn update_info(&self, info: DeviceInfo) {
        *self.info.write().await = info;
        self.reload_plugins().await;
    }

    pub async fn add_link(&self, link: Arc<dyn Link>) {
        let device_id = link.peer_device_id().to_owned();
        {
            let mut links = self.links.lock().await;
            if links.iter().any(|l| l.id() == link.id()) {
                return;
            }
            links.push(link);
        }
        self.reload_plugins().await;
        let _ = self
            .shared
            .event_tx
            .send(DeviceEvent::Connected { device_id });
    }

    pub async fn remove_link(&self, link_id: u64) {
        let device_id = self.device_id();
        let mut links = self.links.lock().await;
        links.retain(|l| l.id() != link_id);
        let reachable = !links.is_empty();
        drop(links);
        if !reachable {
            let _ = self
                .shared
                .event_tx
                .send(DeviceEvent::Disconnected { device_id });
        }
    }

    /// Send a packet on the best available link.
    pub async fn send_packet(&self, packet: NetworkPacket) -> Result<()> {
        let links = self.links.lock().await;
        let link = links
            .iter()
            .max_by_key(|l| l.priority())
            .ok_or_else(|| CoreError::InvalidState("device is not reachable".to_owned()))?;
        link.send(&packet).await
    }

    /// Recompute which plugins can be used with this peer's capabilities.
    pub async fn reload_plugins(&self) {
        let info = self.info.read().await;
        let incoming: BTreeSet<String> = info.outgoing_capabilities.iter().cloned().collect();
        let outgoing: BTreeSet<String> = info.incoming_capabilities.iter().cloned().collect();
        drop(info);

        let mut keys = Vec::new();
        for plugin in self.shared.registry.all() {
            let can_receive = plugin
                .supported_packet_types()
                .iter()
                .any(|t| outgoing.contains(*t));
            let can_send = plugin
                .outgoing_packet_types()
                .iter()
                .any(|t| incoming.contains(*t));
            if !can_receive && !can_send {
                continue;
            }
            let device_id = self.device_id();
            if !self.shared.store.plugin_enabled(
                &device_id,
                plugin.key(),
                plugin.enabled_by_default(),
            ) {
                continue;
            }
            keys.push(plugin.key());
        }
        let ctx = self.plugin_context().await;
        let mut current = self.plugin_keys.lock().await;
        for key in current.iter() {
            if !keys.contains(key) {
                if let Some(plugin) = self.shared.registry.get(key) {
                    plugin.stop(&ctx);
                }
            }
        }
        for key in &keys {
            if !current.contains(key) {
                if let Some(plugin) = self.shared.registry.get(key) {
                    if let Err(e) = plugin.start(&ctx) {
                        warn!(plugin = key, error = %e, "failed to start plugin");
                    }
                }
            }
        }
        *current = keys;
        debug!(device = %ctx.device_id, plugins = current.len(), "plugins reloaded");
    }

    async fn plugin_context(&self) -> PluginContext {
        PluginContext {
            device_id: self.device_id(),
            device_name: self.info.read().await.device_name.clone(),
            platform: self.shared.platform.clone(),
            sender: self.shared.outgoing_tx.clone(),
        }
    }

    /// Main packet entry point, called by the transport or by a link task.
    pub async fn on_packet(&self, packet: NetworkPacket) -> Result<()> {
        let device_id = self.device_id();
        if packet.ty == kc_protocol::PACKET_TYPE_PAIR {
            let event = self.pairing.lock().await.on_packet(&packet)?;
            match event {
                kc_protocol::PairingEvent::PairingRequested { verification_key } => {
                    self.shared
                        .platform
                        .on_pairing_request(&device_id, &verification_key);
                    let _ = self.shared.event_tx.send(DeviceEvent::PairingRequested {
                        device_id: device_id.clone(),
                        verification_key,
                    });
                }
                kc_protocol::PairingEvent::PairingAccepted => self.on_paired().await?,
                kc_protocol::PairingEvent::Unpaired => self.on_unpaired().await?,
                kc_protocol::PairingEvent::PairingCancelled => {
                    let _ = self.shared.event_tx.send(DeviceEvent::Unpaired {
                        device_id: device_id.clone(),
                    });
                }
                kc_protocol::PairingEvent::PairingFailed(msg) => {
                    warn!(%device_id, %msg, "pairing failed");
                }
            }
            return Ok(());
        }

        if !self.is_paired().await {
            // Upstream unpairs when an unpaired device sends any other packet.
            // We ignore it to avoid accidental state loss on flaky networks.
            debug!(%device_id, type = %packet.ty, "ignoring packet from unpaired device");
            return Ok(());
        }

        let ctx = self.plugin_context().await;
        let handlers: Vec<_> = {
            let keys = self.plugin_keys.lock().await;
            self.shared
                .registry
                .handlers_for(&packet.ty)
                .into_iter()
                .filter(|p| keys.contains(&p.key()))
                .collect()
        };
        if handlers.is_empty() {
            debug!(%device_id, type = %packet.ty, "no plugin handles packet");
            return Ok(());
        }
        for plugin in handlers {
            match plugin.on_packet(&ctx, &packet) {
                Ok(true) => break,
                Ok(false) => {}
                Err(e) => {
                    warn!(%device_id, plugin = plugin.key(), error = %e, "plugin packet handler failed")
                }
            }
        }
        Ok(())
    }

    async fn on_paired(&self) -> Result<()> {
        let info = self.info.read().await;
        self.shared.store.trust_device(
            &info.device_id,
            &info.device_name,
            info.device_type.as_str(),
            &self.peer_cert_der,
        )?;
        drop(info);
        self.reload_plugins().await;
        let _ = self.shared.event_tx.send(DeviceEvent::Paired {
            device_id: self.device_id(),
        });
        Ok(())
    }

    async fn on_unpaired(&self) -> Result<()> {
        self.shared.store.remove_trusted_device(&self.device_id())?;
        *self.plugin_keys.lock().await = Vec::new();
        let _ = self.shared.event_tx.send(DeviceEvent::Unpaired {
            device_id: self.device_id(),
        });
        Ok(())
    }

    // ---- pairing API -----------------------------------------------------

    pub async fn request_pairing(&self) -> Result<()> {
        let packet = self.pairing.lock().await.request_pairing()?;
        self.send_packet(packet).await
    }

    pub async fn accept_pairing(&self) -> Result<()> {
        let result = {
            let mut pairing = self.pairing.lock().await;
            pairing.accept_pairing()?
        };
        let info = self.info.read().await.clone();
        let packet =
            NetworkPacket::new(kc_protocol::PACKET_TYPE_PAIR).tap(|p| p.set_bool("pair", true));
        let _ = result;
        self.send_packet(packet).await?;
        self.on_paired().await?;
        drop(info);
        Ok(())
    }

    pub async fn cancel_pairing(&self) -> Result<()> {
        let packet = self.pairing.lock().await.cancel_pairing();
        self.send_packet(packet).await
    }

    pub async fn unpair(&self) -> Result<()> {
        let packet = self.pairing.lock().await.cancel_pairing();
        let _ = self.send_packet(packet).await;
        self.on_unpaired().await
    }

    /// 8-character verification code shown during pairing.
    pub async fn verification_key(&self) -> Option<String> {
        let info = self.info.read().await;
        let stored = self.shared.store.stored_device(&info.device_id);
        drop(info);
        let stored = stored?;
        let der = cert_der_from_pem(&stored.cert_pem).ok()?;
        let parsed = parse_certificate_der(&der).ok()?;
        let my_key = self.shared.identity_public_key.clone();
        Some(kc_protocol::pairing::verification_key(
            &my_key,
            &parsed.spki_der,
            None,
        ))
    }
}

/// Tiny helper trait for fluent packet construction.
trait Tap {
    fn tap(self, f: impl FnOnce(&mut Self)) -> Self;
}
impl Tap for NetworkPacket {
    fn tap(mut self, f: impl FnOnce(&mut Self)) -> Self {
        f(&mut self);
        self
    }
}

#[allow(dead_code)]
fn _assert_platform(_: &dyn Platform) {}
