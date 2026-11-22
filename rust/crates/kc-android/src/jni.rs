//! Exported JNI functions. The Kotlin declarations live in
//! `android-shell/app/src/main/java/org/kdeconnect/rust/KcNative.kt`.
//!
//! All functions are called on Java threads. We never store a `JNIEnv` in the
//! Rust state: [`crate::bridge`] re-attaches via the cached `JavaVM`, and the
//! exported functions only use the `JNIEnv` passed in by the VM for the
//! duration of the call. All background logic (including packet handling)
//! lives in `kc-core` / `kc-plugins` and the Rust supervisor.

#![allow(non_snake_case)]

use std::path::Path;
use std::sync::Arc;

use jni::objects::{JClass, JString};
use jni::sys::{jboolean, jstring};
use jni::JNIEnv;
use kc_core::{DeviceManager, FileStore, LinkEvent, LinkProvider, Platform, Store};
use kc_protocol::identity::generate_device_id;
use kc_protocol::{generate_identity, DeviceInfo};
use tokio::sync::mpsc;
use tracing::{debug, warn};

use crate::bridge;
use crate::echo::ClipboardEcho;
use crate::platform::AndroidPlatform;
use crate::snapshot::snapshots_to_json;
use crate::state::{current, rebroadcast_identity, state_slot, AndroidState};
use crate::supervisor;
use kc_transport::LanLinkProvider;

const STORE_FILE: &str = "kc-store.json";

fn init_tracing() {
    // rustls needs one process-wide CryptoProvider; the whole workspace uses ring.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .try_init();
}

fn read_jstring(env: &mut JNIEnv, value: &JString) -> Option<String> {
    env.get_string(value)
        .ok()
        .map(|s| s.to_string_lossy().into_owned())
}

/// `devices()` must always return a JSON array, including before `init` or on
/// any internal failure — never a null `jstring`.
fn devices_json(state: Option<Arc<AndroidState>>) -> String {
    let Some(state) = state else {
        return "[]".to_owned();
    };
    state
        .with_runtime(|runtime| {
            runtime.block_on(async {
                let mut snapshots = Vec::new();
                for device in state.manager.devices().await {
                    snapshots.push(device.snapshot().await);
                }
                snapshots_to_json(&snapshots)
            })
        })
        .unwrap_or_else(|| "[]".to_owned())
}

fn build_state(data_dir: &str) -> std::result::Result<Arc<AndroidState>, String> {
    init_tracing();

    // Echo suppression is shared between the JNI sender and the Platform
    // receiver; Kotlin only performs the stateless clipboard API call.
    let echo = Arc::new(ClipboardEcho::default());
    let platform = Arc::new(AndroidPlatform::new("Android".to_owned(), echo.clone()));
    let device_name = platform.device_name();

    let store: Arc<dyn Store> = Arc::new(
        FileStore::open(Path::new(data_dir).join(STORE_FILE))
            .map_err(|e| format!("cannot open store: {e}"))?,
    );
    let identity = match store.load_identity() {
        Ok(Some(identity)) => identity,
        Ok(None) => {
            let identity = generate_identity(&generate_device_id())
                .map_err(|e| format!("cannot generate identity: {e}"))?;
            store
                .save_identity(&identity)
                .map_err(|e| format!("cannot save identity: {e}"))?;
            identity
        }
        Err(e) => return Err(format!("cannot load identity: {e}")),
    };

    let registry = kc_plugins::default_registry();
    let mut my_info = DeviceInfo::new(
        identity.device_id.clone(),
        device_name.clone(),
        platform.device_type(),
    );
    let (incoming, outgoing) = registry.capabilities();
    my_info.incoming_capabilities = incoming;
    my_info.outgoing_capabilities = outgoing;

    let manager = Arc::new(DeviceManager::new(
        identity.clone(),
        store.clone(),
        platform,
        registry,
    ));
    // Subscribe before the provider starts so no DeviceEvent is missed.
    let events_rx = manager.subscribe();

    let provider = Arc::new(
        LanLinkProvider::new(identity, my_info.clone(), store, true)
            .map_err(|e| format!("cannot start LAN provider: {e}"))?,
    );

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("kc-android-rust")
        .enable_all()
        .build()
        .map_err(|e| format!("cannot build tokio runtime: {e}"))?;

    let (link_tx, mut link_rx) = mpsc::unbounded_channel::<LinkEvent>();
    {
        let provider = provider.clone();
        runtime.spawn(async move {
            if let Err(e) = provider.start(link_tx).await {
                warn!(%e, "LAN provider stopped with an error");
            }
        });
    }

    // Every LinkEvent — including Packet — goes through the manager, which
    // dispatches to `Device::on_packet` and then to the plugin registry. There
    // is deliberately no second packet pump here.
    {
        let manager_for_events = manager.clone();
        runtime.spawn(async move {
            while let Some(event) = link_rx.recv().await {
                if let Err(e) = manager_for_events.handle_link_event(event).await {
                    warn!(%e, "failed to handle link event");
                }
            }
        });
    }

    if let Some(mut outgoing_rx) = runtime.block_on(manager.take_outgoing_receiver()) {
        let sender = manager.clone();
        runtime.spawn(async move {
            while let Some(packet) = outgoing_rx.recv().await {
                if let Err(e) = sender.send_packet(&packet.device_id, packet.packet).await {
                    debug!(%e, device = %packet.device_id, "outgoing packet dropped");
                }
            }
        });
    }

    if let Err(e) = runtime.block_on(manager.load_remembered_devices()) {
        warn!(%e, "failed to load remembered devices");
    }

    let state = Arc::new(AndroidState::new(manager, runtime, provider, my_info, echo));
    {
        let mut slot = state_slot()
            .lock()
            .map_err(|_| "state lock poisoned".to_owned())?;
        if let Some(previous) = slot.take() {
            previous.stop();
        }
        *slot = Some(state.clone());
    }

    // Rust owns the event loop: watch DeviceEvents, push device snapshots and
    // run periodic rediscovery/self-healing inside the same tokio runtime.
    let supervisor_state = state.clone();
    if state
        .with_runtime(|runtime| {
            runtime.spawn(supervisor::run(supervisor_state, events_rx));
        })
        .is_none()
    {
        return Err("cannot spawn background supervisor".to_owned());
    }

    // Push the initial device list once at init; Kotlin never polls devices().
    bridge::on_devices_changed(&devices_json(Some(state.clone())));
    Ok(state)
}

/// `boolean KcNative.init(String dataDir)`
#[no_mangle]
pub extern "system" fn Java_org_kdeconnect_rust_KcNative_init<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    data_dir: JString<'local>,
) -> jboolean {
    init_tracing();
    let Some(dir) = read_jstring(&mut env, &data_dir) else {
        bridge::on_log(1, "KcNative.init: invalid dataDir");
        return 0;
    };
    let vm = match env.get_java_vm() {
        Ok(vm) => vm,
        Err(e) => {
            warn!(%e, "cannot get JavaVM");
            return 0;
        }
    };
    // Cache the VM before anything else so early Platform callbacks are safe.
    bridge::install_vm(vm);
    match build_state(&dir) {
        Ok(_) => 1,
        Err(e) => {
            warn!(%e, "KcNative.init failed");
            bridge::on_log(1, &format!("KcNative.init failed: {e}"));
            0
        }
    }
}

/// `String KcNative.devices()` — always a JSON array, never null.
#[no_mangle]
pub extern "system" fn Java_org_kdeconnect_rust_KcNative_devices<'local>(
    env: JNIEnv<'local>,
    _class: JClass<'local>,
) -> jstring {
    let json = devices_json(current());
    match env.new_string(json) {
        Ok(text) => text.into_raw(),
        Err(e) => {
            warn!(%e, "cannot build devices JSON; returning empty array");
            env.new_string("[]")
                .map(|text| text.into_raw())
                .unwrap_or(std::ptr::null_mut())
        }
    }
}

fn device_id_arg(env: &mut JNIEnv, id: &JString) -> Option<String> {
    read_jstring(env, id)
}

/// `boolean KcNative.pair(String deviceId)`
#[no_mangle]
pub extern "system" fn Java_org_kdeconnect_rust_KcNative_pair<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    device_id: JString<'local>,
) -> jboolean {
    let Some(id) = device_id_arg(&mut env, &device_id) else {
        return 0;
    };
    match current() {
        Some(state) => state
            .with_runtime(|runtime| runtime.block_on(state.manager.request_pairing(&id)))
            .and_then(|result| result.ok())
            .is_some() as jboolean,
        None => 0,
    }
}

/// `boolean KcNative.accept(String deviceId)`
#[no_mangle]
pub extern "system" fn Java_org_kdeconnect_rust_KcNative_accept<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    device_id: JString<'local>,
) -> jboolean {
    let Some(id) = device_id_arg(&mut env, &device_id) else {
        return 0;
    };
    match current() {
        Some(state) => state
            .with_runtime(|runtime| runtime.block_on(state.manager.accept_pairing(&id)))
            .and_then(|result| result.ok())
            .is_some() as jboolean,
        None => 0,
    }
}

/// `boolean KcNative.unpair(String deviceId)`
#[no_mangle]
pub extern "system" fn Java_org_kdeconnect_rust_KcNative_unpair<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    device_id: JString<'local>,
) -> jboolean {
    let Some(id) = device_id_arg(&mut env, &device_id) else {
        return 0;
    };
    match current() {
        Some(state) => state
            .with_runtime(|runtime| runtime.block_on(state.manager.unpair(&id)))
            .and_then(|result| result.ok())
            .is_some() as jboolean,
        None => 0,
    }
}

/// `boolean KcNative.ping(String deviceId, String message)`
#[no_mangle]
pub extern "system" fn Java_org_kdeconnect_rust_KcNative_ping<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    device_id: JString<'local>,
    message: JString<'local>,
) -> jboolean {
    let Some(id) = device_id_arg(&mut env, &device_id) else {
        return 0;
    };
    let Some(text) = read_jstring(&mut env, &message) else {
        return 0;
    };
    let Some(state) = current() else {
        return 0;
    };
    let packet = kc_plugins::ping::ping_packet(Some(&text));
    state
        .with_runtime(|runtime| runtime.block_on(state.manager.send_packet(&id, packet)))
        .and_then(|result| result.ok())
        .is_some() as jboolean
}

/// `boolean KcNative.setClipboard(String deviceId, String text)`
#[no_mangle]
pub extern "system" fn Java_org_kdeconnect_rust_KcNative_setClipboard<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    device_id: JString<'local>,
    text: JString<'local>,
) -> jboolean {
    let Some(id) = device_id_arg(&mut env, &device_id) else {
        return 0;
    };
    let Some(content) = read_jstring(&mut env, &text) else {
        return 0;
    };
    let Some(state) = current() else {
        return 0;
    };
    // Rust-side echo suppression: remember what we send so a returning
    // clipboard packet or a later local read does not bounce it back.
    state.echo.remember(&content);
    let packet = kc_plugins::clipboard::clipboard_packet(&content);
    state
        .with_runtime(|runtime| runtime.block_on(state.manager.send_packet(&id, packet)))
        .and_then(|result| result.ok())
        .is_some() as jboolean
}

/// `boolean KcNative.find(String deviceId)` — send a find-my-phone request.
#[no_mangle]
pub extern "system" fn Java_org_kdeconnect_rust_KcNative_find<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    device_id: JString<'local>,
) -> jboolean {
    let Some(id) = device_id_arg(&mut env, &device_id) else {
        return 0;
    };
    let Some(state) = current() else {
        return 0;
    };
    let packet = kc_plugins::findmyphone::find_packet();
    state
        .with_runtime(|runtime| runtime.block_on(state.manager.send_packet(&id, packet)))
        .and_then(|result| result.ok())
        .is_some() as jboolean
}

/// `boolean KcNative.refresh()` — trigger an immediate Rust-side rediscovery.
#[no_mangle]
pub extern "system" fn Java_org_kdeconnect_rust_KcNative_refresh<'local>(
    _env: JNIEnv<'local>,
    _class: JClass<'local>,
) -> jboolean {
    match current() {
        Some(state) => state
            .with_runtime(|runtime| runtime.block_on(rebroadcast_identity(&state)))
            .and_then(|result| result.ok())
            .is_some() as jboolean,
        None => 0,
    }
}

/// `void KcNative.stop()`
#[no_mangle]
pub extern "system" fn Java_org_kdeconnect_rust_KcNative_stop<'local>(
    _env: JNIEnv<'local>,
    _class: JClass<'local>,
) {
    let previous = state_slot().lock().ok().and_then(|mut slot| slot.take());
    if let Some(state) = previous {
        state.stop();
    }
}
