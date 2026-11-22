//! JNI bridge for the thin Android shell (`android-shell/`).
//!
//! Built as a `cdylib` named `libkc_android.so` and loaded by `KcNative.kt`.
//! Rust owns *all* background logic; Kotlin only renders UI and performs
//! stateless Android system API calls. The crate owns:
//! * the cached `JavaVM` and safe callbacks into `KcBridge` ([`bridge`]),
//! * the JNI exports and process-wide state ([`jni`], [`state`]),
//! * the `kc_core::Platform` callbacks into Kotlin ([`platform`]),
//! * clipboard echo suppression ([`echo`]) and the background supervisor
//!   ([`supervisor`]) that pushes device snapshots and self-heals,
//! * the serde DTO used by `devices()` ([`snapshot`]).

mod bridge;
mod echo;
mod jni;
mod platform;
mod snapshot;
mod state;
mod supervisor;
