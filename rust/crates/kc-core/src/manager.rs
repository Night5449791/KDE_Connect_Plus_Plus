//! Device list, link events and packet ownership.

use std::collections::HashMap;
use std::sync::Arc;

use kc_protocol::{
    cert_der_from_pem, parse_certificate_der, DeviceInfo, DeviceType, Identity, NetworkPacket,
};
use tokio::sync::{broadcast, mpsc, Mutex, RwLock};
use tracing::{debug, warn};

use crate::device::Device;
use crate::{
    platform::Platform, plugin::OutgoingPacket, store::Store, CoreError, LinkEvent, PluginRegistry,
    Result,
};

#[derive(Clone, Debug)]
pub enum DeviceEvent {
    Discovered {
        device_id: String,
        name: String,
        paired: bool,
    },
    PairingRequested {
        device_id: String,
        verification_key: String,
    },
    Paired {
        device_id: String,
    },
    Unpaired {
        device_id: String,
    },
    Connected {
        device_id: String,
    },
    Disconnected {
        device_id: String,
    },
}

/// State shared between every `Device` instance and the manager.
pub struct DeviceShared {
    pub identity: Arc<Identity>,
    pub identity_public_key: Vec<u8>,
    pub store: Arc<dyn Store>,
    pub platform: Arc<dyn Platform>,
    pub registry: Arc<PluginRegistry>,
    pub outgoing_tx: mpsc::UnboundedSender<OutgoingPacket>,
    pub event_tx: broadcast::Sender<DeviceEvent>,
}

pub struct DeviceManager {
    devices: RwLock<HashMap<String, Arc<Device>>>,
    shared: Arc<DeviceShared>,
    outgoing_rx: Mutex<Option<mpsc::UnboundedReceiver<OutgoingPacket>>>,
}

impl DeviceManager {
    pub fn new(
        identity: Identity,
        store: Arc<dyn Store>,
        platform: Arc<dyn Platform>,
        registry: Arc<PluginRegistry>,
    ) -> Self {
        let (event_tx, _) = broadcast::channel(256);
        let (outgoing_tx, outgoing_rx) = mpsc::unbounded_channel();
        let identity_public_key = identity.public_key_der.clone();
        let identity = Arc::new(identity);
        Self {
            devices: RwLock::new(HashMap::new()),
            shared: Arc::new(DeviceShared {
                identity,
                identity_public_key,
                store,
                platform,
                registry,
                outgoing_tx,
                event_tx,
            }),
            outgoing_rx: Mutex::new(Some(outgoing_rx)),
        }
    }

    /// Take the channel carrying packets produced by plugins.
    pub async fn take_outgoing_receiver(&self) -> Option<mpsc::UnboundedReceiver<OutgoingPacket>> {
        self.outgoing_rx.lock().await.take()
    }

    pub fn subscribe(&self) -> broadcast::Receiver<DeviceEvent> {
        self.shared.event_tx.subscribe()
    }

    pub async fn devices(&self) -> Vec<Arc<Device>> {
        self.devices.read().await.values().cloned().collect()
    }

    pub async fn get(&self, device_id: &str) -> Option<Arc<Device>> {
        self.devices.read().await.get(device_id).cloned()
    }

    pub async fn load_remembered_devices(&self) -> Result<()> {
        for id in self.shared.store.trusted_devices() {
            if self.get(&id).await.is_some() {
                continue;
            }
            let Some(stored) = self.shared.store.stored_device(&id) else {
                continue;
            };
            let der = match cert_der_from_pem(&stored.cert_pem) {
                Ok(d) => d,
                Err(e) => {
                    warn!(%id, error = %e, "skipping device with invalid certificate");
                    continue;
                }
            };
            let parsed = parse_certificate_der(&der)?;
            let mut info = DeviceInfo::new(
                id.clone(),
                stored.name.clone(),
                DeviceType::from_str(&stored.device_type),
            );
            let (incoming, outgoing) = self.shared.registry.capabilities();
            info.incoming_capabilities = incoming;
            info.outgoing_capabilities = outgoing;
            let device = Device::new(
                info,
                self.shared.identity_public_key.clone(),
                parsed.spki_der.clone(),
                der,
                true,
                self.shared.clone(),
            );
            device.reload_plugins().await;
            self.devices
                .write()
                .await
                .insert(id.clone(), Arc::new(device));
        }
        Ok(())
    }

    /// Consume a `LinkEvent` from a transport provider.
    pub async fn handle_link_event(&self, event: LinkEvent) -> Result<()> {
        match event {
            LinkEvent::Connected {
                link,
                info,
                cert_der,
            } => {
                let id = info.device_id.clone();
                let is_new = self.get(&id).await.is_none();
                let device = if is_new {
                    let paired = self.shared.store.is_trusted(&id);
                    let peer_key = if cert_der.is_empty() {
                        Vec::new()
                    } else {
                        parse_certificate_der(&cert_der)?.spki_der.clone()
                    };
                    let mut device_info = info.clone();
                    if device_info.incoming_capabilities.is_empty() {
                        let (incoming, outgoing) = self.shared.registry.capabilities();
                        device_info.incoming_capabilities = incoming;
                        device_info.outgoing_capabilities = outgoing;
                    }
                    let device = Arc::new(Device::new(
                        device_info,
                        self.shared.identity_public_key.clone(),
                        peer_key,
                        cert_der,
                        paired,
                        self.shared.clone(),
                    ));
                    self.devices
                        .write()
                        .await
                        .insert(id.clone(), device.clone());
                    device.reload_plugins().await;
                    let _ = self.shared.event_tx.send(DeviceEvent::Discovered {
                        device_id: id.clone(),
                        name: info.device_name.clone(),
                        paired,
                    });
                    device
                } else {
                    self.get(&id)
                        .await
                        .ok_or_else(|| CoreError::DeviceNotFound(id.clone()))?
                };
                if !is_new {
                    device.update_info(info).await;
                }
                device.add_link(link).await;
                debug!(%id, "link connected");
            }
            LinkEvent::Packet { device_id, packet } => {
                self.on_packet(&device_id, packet).await?;
            }
            LinkEvent::Disconnected { link_id, device_id } => {
                if let Some(device) = self.get(&device_id).await {
                    device.remove_link(link_id).await;
                }
            }
        }
        Ok(())
    }

    pub async fn on_packet(&self, device_id: &str, packet: NetworkPacket) -> Result<()> {
        let device = self
            .get(device_id)
            .await
            .ok_or_else(|| CoreError::DeviceNotFound(device_id.to_owned()))?;
        device.on_packet(packet).await
    }

    pub async fn send_packet(&self, device_id: &str, packet: NetworkPacket) -> Result<()> {
        let device = self
            .get(device_id)
            .await
            .ok_or_else(|| CoreError::DeviceNotFound(device_id.to_owned()))?;
        device.send_packet(packet).await
    }

    pub async fn request_pairing(&self, device_id: &str) -> Result<()> {
        self.get(device_id)
            .await
            .ok_or_else(|| CoreError::DeviceNotFound(device_id.to_owned()))?
            .request_pairing()
            .await
    }

    pub async fn accept_pairing(&self, device_id: &str) -> Result<()> {
        self.get(device_id)
            .await
            .ok_or_else(|| CoreError::DeviceNotFound(device_id.to_owned()))?
            .accept_pairing()
            .await
    }

    pub async fn cancel_pairing(&self, device_id: &str) -> Result<()> {
        self.get(device_id)
            .await
            .ok_or_else(|| CoreError::DeviceNotFound(device_id.to_owned()))?
            .cancel_pairing()
            .await
    }

    pub async fn unpair(&self, device_id: &str) -> Result<()> {
        self.get(device_id)
            .await
            .ok_or_else(|| CoreError::DeviceNotFound(device_id.to_owned()))?
            .unpair()
            .await
    }

    pub fn identity(&self) -> Arc<Identity> {
        self.shared.identity.clone()
    }

    pub fn platform(&self) -> Arc<dyn Platform> {
        self.shared.platform.clone()
    }

    pub fn store(&self) -> Arc<dyn Store> {
        self.shared.store.clone()
    }

    pub fn registry(&self) -> Arc<PluginRegistry> {
        self.shared.registry.clone()
    }
}
