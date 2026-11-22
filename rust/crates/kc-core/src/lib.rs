//! Core state machine for the Rust KDE Connect client.
//!
//! Crate layout:
//! * [`link`] — transport-agnostic `Link`/`LinkProvider` traits.
//! * [`platform`] — callbacks the host application implements (UI, Android APIs).
//! * [`plugin`] — plugin trait and per-device plugin context.
//! * [`store`] — persistence for identity, trusted devices and settings.
//! * [`device`] / [`manager`] — device state, pairing and plugin dispatch.

pub mod device;
pub mod link;
pub mod manager;
pub mod platform;
pub mod plugin;
pub mod store;

pub use device::{Device, DeviceSnapshot};
pub use link::{Link, LinkEvent, LinkProvider};
pub use manager::{DeviceEvent, DeviceManager};
pub use platform::{BatteryStatus, NoopPlatform, Platform};
pub use plugin::{OutgoingPacket, Plugin, PluginContext, PluginRegistry};
pub use store::{FileStore, IdentityMaterial, MemoryStore, Store};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("protocol error: {0}")]
    Protocol(#[from] kc_protocol::ProtocolError),
    #[error("plugin error: {0}")]
    Plugin(String),
    #[error("transport error: {0}")]
    Transport(String),
    #[error("device {0} not found")]
    DeviceNotFound(String),
    #[error("invalid state: {0}")]
    InvalidState(String),
    #[error("pairing error: {0}")]
    Pairing(#[from] kc_protocol::PairingError),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, CoreError>;
