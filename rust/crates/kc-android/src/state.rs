//! Process-wide bridge state. JNI calls enter on arbitrary Java threads, so
//! the only shared mutable datum is a `OnceLock<Mutex<Option<Arc<...>>>>`.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use kc_core::{DeviceManager, LinkProvider};
use kc_protocol::DeviceInfo;
use tokio::net::UdpSocket;
use tokio::runtime::Runtime;

use crate::echo::ClipboardEcho;
use kc_transport::LanLinkProvider;

pub struct AndroidState {
    pub manager: Arc<DeviceManager>,
    /// `Option` so `stop()` can take ownership and call `shutdown_timeout`,
    /// which consumes the runtime.
    runtime: Mutex<Option<Runtime>>,
    pub provider: Arc<LanLinkProvider>,
    /// Our advertised identity, kept for the Rust-side rediscovery helper.
    pub my_info: DeviceInfo,
    /// Shared with `AndroidPlatform` for clipboard echo suppression.
    pub echo: Arc<ClipboardEcho>,
}

impl AndroidState {
    pub fn new(
        manager: Arc<DeviceManager>,
        runtime: Runtime,
        provider: Arc<LanLinkProvider>,
        my_info: DeviceInfo,
        echo: Arc<ClipboardEcho>,
    ) -> Self {
        Self {
            manager,
            runtime: Mutex::new(Some(runtime)),
            provider,
            my_info,
            echo,
        }
    }

    /// Borrow the runtime for JNI calls. `block_on`/`spawn` only need `&self`.
    pub fn with_runtime<T>(&self, f: impl FnOnce(&Runtime) -> T) -> Option<T> {
        let guard: MutexGuard<'_, Option<Runtime>> = self.runtime.lock().ok()?;
        Some(f(guard.as_ref()?))
    }

    /// Called from JNI `stop`, never from inside the tokio runtime.
    pub fn stop(&self) {
        if let Ok(mut guard) = self.runtime.lock() {
            if let Some(runtime) = guard.take() {
                runtime.block_on(self.provider.stop());
                runtime.shutdown_timeout(Duration::from_secs(1));
            }
        }
    }
}

static STATE: OnceLock<Mutex<Option<Arc<AndroidState>>>> = OnceLock::new();

pub fn state_slot() -> &'static Mutex<Option<Arc<AndroidState>>> {
    STATE.get_or_init(|| Mutex::new(None))
}

pub fn current() -> Option<Arc<AndroidState>> {
    state_slot().lock().ok()?.clone()
}

/// Rust-side rediscovery helper used by both `KcNative.refresh()` and the
/// periodic supervisor task.
///
/// `LanLinkProvider` already runs its own identity broadcast every 5 seconds
/// and exposes no public "broadcast now" method. Rather than modify the shared
/// transport crate, we send one extra UDP identity packet ourselves using its
/// public `identity_with_tcp_port` + `UDP_PORT` helpers and the provider's
/// advertised `tcp_port()`. Peers receiving it open a TCP link back to us,
/// which is exactly the effect of an upstream `onNetworkChange()` refresh.
/// No Kotlin polling or state is involved.
pub async fn rebroadcast_identity(state: &AndroidState) -> std::result::Result<(), String> {
    let port = state.provider.tcp_port();
    if port == 0 {
        return Err("LAN provider has no TCP listener yet".to_owned());
    }
    let wire = kc_transport::identity_with_tcp_port(&state.my_info, port).serialize();

    let socket = UdpSocket::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)))
        .await
        .map_err(|e| format!("cannot bind rediscovery socket: {e}"))?;
    socket
        .set_broadcast(true)
        .map_err(|e| format!("cannot enable broadcast: {e}"))?;
    let target = SocketAddr::from((Ipv4Addr::BROADCAST, kc_transport::UDP_PORT));
    socket
        .send_to(wire.as_bytes(), target)
        .await
        .map_err(|e| format!("rediscovery broadcast failed: {e}"))?;
    Ok(())
}
