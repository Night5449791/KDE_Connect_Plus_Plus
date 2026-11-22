//! Persistence for identity, trusted devices and per-device plugin switches.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use kc_protocol::{cert_der_from_pem, cert_pem_from_der, Identity};
use serde::{Deserialize, Serialize};

use crate::Result;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IdentityMaterial {
    pub device_id: String,
    pub cert_pem: String,
    pub private_key_der_b64: String,
}

impl IdentityMaterial {
    pub fn from_identity(identity: &Identity) -> Self {
        use base64::Engine;
        Self {
            device_id: identity.device_id.clone(),
            cert_pem: identity.cert_pem.clone(),
            private_key_der_b64: base64::engine::general_purpose::STANDARD
                .encode(&identity.private_key_der),
        }
    }

    pub fn to_identity(&self) -> Result<Identity> {
        use base64::Engine;
        let key = base64::engine::general_purpose::STANDARD
            .decode(&self.private_key_der_b64)
            .map_err(|e| crate::CoreError::InvalidState(format!("bad private key: {e}")))?;
        Ok(Identity::from_parts(
            self.device_id.clone(),
            &self.cert_pem,
            key,
        )?)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct StoredDevice {
    pub name: String,
    pub device_type: String,
    pub cert_pem: String,
    pub plugin_enabled: HashMap<String, bool>,
}

/// Storage abstraction; headless daemon uses [`FileStore`], tests use [`MemoryStore`].
pub trait Store: Send + Sync {
    fn load_identity(&self) -> Result<Option<Identity>>;
    fn save_identity(&self, identity: &Identity) -> Result<()>;

    fn trusted_devices(&self) -> Vec<String>;
    fn is_trusted(&self, device_id: &str) -> bool;
    fn stored_device(&self, device_id: &str) -> Option<StoredDevice>;
    fn trust_device(
        &self,
        device_id: &str,
        name: &str,
        device_type: &str,
        cert_der: &[u8],
    ) -> Result<()>;
    fn remove_trusted_device(&self, device_id: &str) -> Result<()>;

    fn plugin_enabled(&self, device_id: &str, plugin_key: &str, default: bool) -> bool;
    fn set_plugin_enabled(&self, device_id: &str, plugin_key: &str, enabled: bool) -> Result<()>;
}

#[derive(Default)]
struct MemoryData {
    identity: Option<IdentityMaterial>,
    devices: HashMap<String, StoredDevice>,
}

/// In-memory store with interior mutability.
#[derive(Default)]
pub struct MemoryStore {
    data: Mutex<MemoryData>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Store for MemoryStore {
    fn load_identity(&self) -> Result<Option<Identity>> {
        self.data
            .lock()
            .unwrap()
            .identity
            .as_ref()
            .map(|m| m.to_identity())
            .transpose()
    }

    fn save_identity(&self, identity: &Identity) -> Result<()> {
        self.data.lock().unwrap().identity = Some(IdentityMaterial::from_identity(identity));
        Ok(())
    }

    fn trusted_devices(&self) -> Vec<String> {
        self.data.lock().unwrap().devices.keys().cloned().collect()
    }

    fn is_trusted(&self, device_id: &str) -> bool {
        self.data.lock().unwrap().devices.contains_key(device_id)
    }

    fn stored_device(&self, device_id: &str) -> Option<StoredDevice> {
        self.data.lock().unwrap().devices.get(device_id).cloned()
    }

    fn trust_device(
        &self,
        device_id: &str,
        name: &str,
        device_type: &str,
        cert_der: &[u8],
    ) -> Result<()> {
        let cert_pem = cert_pem_from_der(cert_der)?;
        let mut data = self.data.lock().unwrap();
        let entry = data.devices.entry(device_id.to_owned()).or_default();
        entry.name = name.to_owned();
        entry.device_type = device_type.to_owned();
        entry.cert_pem = cert_pem;
        Ok(())
    }

    fn remove_trusted_device(&self, device_id: &str) -> Result<()> {
        self.data.lock().unwrap().devices.remove(device_id);
        Ok(())
    }

    fn plugin_enabled(&self, device_id: &str, plugin_key: &str, default: bool) -> bool {
        self.data
            .lock()
            .unwrap()
            .devices
            .get(device_id)
            .and_then(|d| d.plugin_enabled.get(plugin_key).copied())
            .unwrap_or(default)
    }

    fn set_plugin_enabled(&self, device_id: &str, plugin_key: &str, enabled: bool) -> Result<()> {
        self.data
            .lock()
            .unwrap()
            .devices
            .entry(device_id.to_owned())
            .or_default()
            .plugin_enabled
            .insert(plugin_key.to_owned(), enabled);
        Ok(())
    }
}

#[derive(Default, Serialize, Deserialize)]
struct FileData {
    identity: Option<IdentityMaterial>,
    devices: HashMap<String, StoredDevice>,
}

/// JSON-file-backed store used by the headless daemon.
pub struct FileStore {
    path: PathBuf,
    data: Mutex<FileData>,
}

impl FileStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let data = if path.exists() {
            let text = std::fs::read_to_string(&path)?;
            serde_json::from_str(&text).unwrap_or_default()
        } else {
            FileData::default()
        };
        Ok(Self {
            path,
            data: Mutex::new(data),
        })
    }

    fn flush(&self, data: &FileData) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(data)?;
        std::fs::write(&self.path, text)?;
        Ok(())
    }
}

impl Store for FileStore {
    fn load_identity(&self) -> Result<Option<Identity>> {
        self.data
            .lock()
            .unwrap()
            .identity
            .as_ref()
            .map(|m| m.to_identity())
            .transpose()
    }

    fn save_identity(&self, identity: &Identity) -> Result<()> {
        let mut data = self.data.lock().unwrap();
        data.identity = Some(IdentityMaterial::from_identity(identity));
        self.flush(&data)
    }

    fn trusted_devices(&self) -> Vec<String> {
        self.data.lock().unwrap().devices.keys().cloned().collect()
    }

    fn is_trusted(&self, device_id: &str) -> bool {
        self.data.lock().unwrap().devices.contains_key(device_id)
    }

    fn stored_device(&self, device_id: &str) -> Option<StoredDevice> {
        self.data.lock().unwrap().devices.get(device_id).cloned()
    }

    fn trust_device(
        &self,
        device_id: &str,
        name: &str,
        device_type: &str,
        cert_der: &[u8],
    ) -> Result<()> {
        let cert_pem = cert_pem_from_der(cert_der)?;
        let mut data = self.data.lock().unwrap();
        let entry = data.devices.entry(device_id.to_owned()).or_default();
        entry.name = name.to_owned();
        entry.device_type = device_type.to_owned();
        entry.cert_pem = cert_pem;
        self.flush(&data)
    }

    fn remove_trusted_device(&self, device_id: &str) -> Result<()> {
        let mut data = self.data.lock().unwrap();
        data.devices.remove(device_id);
        self.flush(&data)
    }

    fn plugin_enabled(&self, device_id: &str, plugin_key: &str, default: bool) -> bool {
        self.data
            .lock()
            .unwrap()
            .devices
            .get(device_id)
            .and_then(|d| d.plugin_enabled.get(plugin_key).copied())
            .unwrap_or(default)
    }

    fn set_plugin_enabled(&self, device_id: &str, plugin_key: &str, enabled: bool) -> Result<()> {
        let mut data = self.data.lock().unwrap();
        data.devices
            .entry(device_id.to_owned())
            .or_default()
            .plugin_enabled
            .insert(plugin_key.to_owned(), enabled);
        self.flush(&data)
    }
}

/// Validate that a stored certificate PEM is still parseable.
pub fn validate_stored_certificate(cert_pem: &str) -> bool {
    cert_der_from_pem(cert_pem)
        .map(|der| kc_protocol::parse_certificate_der(&der).is_ok())
        .unwrap_or(false)
}
