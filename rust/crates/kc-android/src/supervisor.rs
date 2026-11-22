//! Rust-owned background supervisor.
//!
//! This is the only place that decides *when* Kotlin is notified about device
//! state, and the only place that performs periodic self-healing. Kotlin never
//! polls `devices()`; it just receives `KcBridge.onDevicesChanged` pushes.

use std::sync::Arc;
use std::time::{Duration, Instant};

use kc_core::{DeviceEvent, DeviceManager};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::broadcast::Receiver;
use tracing::{debug, warn};

use crate::bridge;
use crate::snapshot::snapshots_to_json;
use crate::state::{rebroadcast_identity, AndroidState};

/// Explicit `refresh()` and the periodic helper both re-broadcast our identity
/// over UDP so peers re-advertise themselves and links are re-established.
const REDISCOVERY_INTERVAL: Duration = Duration::from_secs(30);
/// Cheap timer used to evaluate the self-healing heuristics.
const WATCHDOG_INTERVAL: Duration = Duration::from_secs(15);
/// Count only "signal" within this sliding window.
const FAILURE_WINDOW: Duration = Duration::from_secs(60);
/// This many disconnect/unpair events in the window is "clearly" unhealthy.
const FAILURE_THRESHOLD: usize = 3;
/// Never ask Kotlin to restart the service more often than this.
const RESTART_COOLDOWN: Duration = Duration::from_secs(300);
/// Consecutive failed re-broadcasts before asking for a restart.
const MAX_BROADCAST_FAILURES: u32 = 3;

/// Compute the full device snapshot JSON and push it to Kotlin in one call.
pub async fn push_devices(manager: &Arc<DeviceManager>) {
    let mut snapshots = Vec::new();
    for device in manager.devices().await {
        snapshots.push(device.snapshot().await);
    }
    bridge::on_devices_changed(&snapshots_to_json(&snapshots));
}

/// Request a foreground-service restart, but never in a loop: a cooldown and
/// the caller's "clearly needed" condition both have to be satisfied.
fn maybe_request_restart(last: &mut Option<Instant>, reason: &str) {
    let now = Instant::now();
    if let Some(previous) = *last {
        if now.duration_since(previous) < RESTART_COOLDOWN {
            debug!(reason, "service restart suppressed by cooldown");
            return;
        }
    }
    warn!(
        reason,
        "requesting Android service restart for self-healing"
    );
    bridge::on_log(2, reason);
    bridge::request_service_restart();
    *last = Some(now);
}

/// Run until the runtime is stopped (the manager's event channel closes).
pub async fn run(state: Arc<AndroidState>, mut events: Receiver<DeviceEvent>) {
    let mut rediscovery = tokio::time::interval(REDISCOVERY_INTERVAL);
    rediscovery.tick().await; // consume the immediate first tick
    let mut watchdog = tokio::time::interval(WATCHDOG_INTERVAL);
    watchdog.tick().await;

    let mut suspicious: Vec<Instant> = Vec::new();
    let mut broadcast_failures = 0u32;
    let mut last_restart: Option<Instant> = None;

    loop {
        tokio::select! {
            event = events.recv() => {
                match event {
                    Ok(event) => {
                        // Link loss / pairing removal are the only signals we
                        // treat as potentially requiring a service restart.
                        if matches!(event, DeviceEvent::Disconnected { .. } | DeviceEvent::Unpaired { .. }) {
                            suspicious.push(Instant::now());
                        }
                        push_devices(&state.manager).await;
                    }
                    Err(RecvError::Lagged(skipped)) => {
                        warn!(skipped, "device event receiver lagged");
                        push_devices(&state.manager).await;
                    }
                    Err(RecvError::Closed) => {
                        debug!("device event stream closed; supervisor exiting");
                        break;
                    }
                }
            }
            _ = rediscovery.tick() => {
                // No polling in Kotlin: the LAN provider already broadcasts
                // every 5s, and this explicit identity re-broadcast (also
                // exposed as KcNative.refresh()) makes discovery deterministic.
                match rebroadcast_identity(&state).await {
                    Ok(()) => broadcast_failures = 0,
                    Err(err) => {
                        broadcast_failures += 1;
                        warn!(%err, broadcast_failures, "rediscovery broadcast failed");
                        if broadcast_failures >= MAX_BROADCAST_FAILURES {
                            maybe_request_restart(
                                &mut last_restart,
                                "repeated LAN rediscovery broadcast failure",
                            );
                            broadcast_failures = 0;
                        }
                    }
                }
            }
            _ = watchdog.tick() => {
                let now = Instant::now();
                suspicious.retain(|at| now.duration_since(*at) <= FAILURE_WINDOW);
                if suspicious.len() >= FAILURE_THRESHOLD {
                    maybe_request_restart(&mut last_restart, "repeated link loss / pairing failure");
                    suspicious.clear();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_bounded() {
        // Guard against accidental "poll every millisecond" regressions.
        assert_eq!(REDISCOVERY_INTERVAL, Duration::from_secs(30));
        assert!(RESTART_COOLDOWN > FAILURE_WINDOW);
        const { assert!(FAILURE_THRESHOLD > 1) };
    }
}
