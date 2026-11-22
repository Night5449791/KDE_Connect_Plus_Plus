//! Safe bridge to Kotlin's stateless `org.kdeconnect.rust.KcBridge` object.
//!
//! `KcNative.init` caches the `JavaVM` once ([`install_vm`]). Every callback
//! attaches the *calling* thread, finds the class and invokes a `@JvmStatic`
//! method. A `JNIEnv` or `jobject` is never stored across threads or calls,
//! which is the one hard rule of JNI. If the VM is not installed yet, the
//! class cannot be found, or Kotlin throws, the call is a silent no-op: a
//! partially initialised shell must never crash the Rust background runtime.

use std::sync::OnceLock;

use jni::objects::{JClass, JString, JValue};
use jni::sys::jint;
use jni::{JNIEnv, JavaVM};
use tracing::warn;

const CLASS: &str = "org/kdeconnect/rust/KcBridge";

const SIG_VOID: &str = "()V";
const SIG_STRING_VOID: &str = "(Ljava/lang/String;)V";
const SIG_TWO_STRINGS_VOID: &str = "(Ljava/lang/String;Ljava/lang/String;)V";
const SIG_INT_STRING_VOID: &str = "(ILjava/lang/String;)V";
const SIG_STRING_RETURN: &str = "()Ljava/lang/String;";
const SIG_INT_RETURN: &str = "()I";
const SIG_STRING_BOOL: &str = "(Ljava/lang/String;)Z";

static VM: OnceLock<JavaVM> = OnceLock::new();

/// Cache the VM handed to `KcNative.init`. Idempotent; called again on a
/// repeated `init` is harmless.
pub fn install_vm(vm: JavaVM) {
    let _ = VM.set(vm);
}

fn vm() -> Option<&'static JavaVM> {
    VM.get()
}

/// Attach the current thread and run `f` against the freshly resolved class.
/// Any pending Kotlin exception is described and cleared so it cannot unwind
/// into Rust or poison a later callback. `onLog` is deliberately not used here
/// to report bridge failures (that would recurse).
fn with_bridge<T, F>(f: F) -> Option<T>
where
    F: for<'local> FnOnce(&mut JNIEnv<'local>, JClass<'local>) -> Option<T>,
{
    let vm = vm()?;
    let mut guard = vm.attach_current_thread().ok()?;
    let env: &mut JNIEnv<'_> = &mut guard;
    let class = match env.find_class(CLASS) {
        Ok(class) => class,
        Err(err) => {
            warn!(%err, "KcBridge unavailable; dropping callback");
            return None;
        }
    };
    let result = f(&mut *env, class);
    if matches!(env.exception_check(), Ok(true)) {
        if let Err(err) = env.exception_describe() {
            warn!(%err, "cannot describe KcBridge exception");
        }
        let _ = env.exception_clear();
        return None;
    }
    result
}

fn call_void(method: &str) -> bool {
    with_bridge(|env, class| {
        env.call_static_method(class, method, SIG_VOID, &[]).ok()?;
        Some(())
    })
    .is_some()
}

fn call_string_void(method: &str, arg: &str) -> bool {
    with_bridge(|env, class| {
        let value = env.new_string(arg).ok()?;
        env.call_static_method(class, method, SIG_STRING_VOID, &[JValue::Object(&value)])
            .ok()?;
        Some(())
    })
    .is_some()
}

fn call_two_strings_void(method: &str, first: &str, second: &str) -> bool {
    with_bridge(|env, class| {
        let a = env.new_string(first).ok()?;
        let b = env.new_string(second).ok()?;
        env.call_static_method(
            class,
            method,
            SIG_TWO_STRINGS_VOID,
            &[JValue::Object(&a), JValue::Object(&b)],
        )
        .ok()?;
        Some(())
    })
    .is_some()
}

fn call_int_string_void(method: &str, level: jint, message: &str) -> bool {
    with_bridge(|env, class| {
        let text = env.new_string(message).ok()?;
        env.call_static_method(
            class,
            method,
            SIG_INT_STRING_VOID,
            &[JValue::Int(level), JValue::Object(&text)],
        )
        .ok()?;
        Some(())
    })
    .is_some()
}

fn call_string(method: &str) -> Option<String> {
    with_bridge(|env, class| {
        let value = env
            .call_static_method(class, method, SIG_STRING_RETURN, &[])
            .ok()?;
        let obj = value.l().ok()?;
        if obj.is_null() {
            return None;
        }
        let java_string = JString::from(obj);
        let text = env.get_string(&java_string).ok()?;
        Some(text.to_string_lossy().into_owned())
    })
}

fn call_int(method: &str) -> Option<i32> {
    with_bridge(|env, class| {
        env.call_static_method(class, method, SIG_INT_RETURN, &[])
            .ok()?
            .i()
            .ok()
    })
}

fn call_string_bool(method: &str, arg: &str) -> Option<bool> {
    with_bridge(|env, class| {
        let value = env.new_string(arg).ok()?;
        env.call_static_method(class, method, SIG_STRING_BOOL, &[JValue::Object(&value)])
            .ok()?
            .z()
            .ok()
    })
}

// --- Device state / events -------------------------------------------------

/// `void KcBridge.onDevicesChanged(String json)`
pub fn on_devices_changed(json: &str) {
    call_string_void("onDevicesChanged", json);
}

/// `void KcBridge.onEvent(String json)` — pairing, errors, plugin events.
pub fn on_event(json: &str) {
    call_string_void("onEvent", json);
}

/// `void KcBridge.onClipboardChanged(String text)` — remote clipboard content.
pub fn on_clipboard_changed(text: &str) {
    call_string_void("onClipboardChanged", text);
}

/// `void KcBridge.onLog(int level, String message)` — optional tracing bridge.
/// Level convention: 0=debug, 1=info, 2=warn, 3=error.
pub fn on_log(level: jint, message: &str) {
    call_int_string_void("onLog", level, message);
}

/// `void KcBridge.notify(String title, String message)` — stateless toast /
/// notification adapter used for ping and remote notifications.
pub fn notify(title: &str, message: &str) {
    call_two_strings_void("notify", title, message);
}

/// `void KcBridge.requestServiceRestart()` — self-healing escape hatch.
pub fn request_service_restart() {
    call_void("requestServiceRestart");
}

// --- Stateless system API adapters (Rust pulls these) ----------------------

/// `String KcBridge.deviceName()`
pub fn device_name() -> Option<String> {
    call_string("deviceName")
}

/// `int KcBridge.deviceType()` — 0 phone, 1 tablet, 2 desktop, 3 laptop, 4 tv.
pub fn device_type() -> Option<i32> {
    call_int("deviceType")
}

/// `String KcBridge.clipboardText()` — may be null/empty.
pub fn clipboard_text() -> Option<String> {
    call_string("clipboardText")
}

/// `boolean KcBridge.setClipboardText(String text)`
pub fn set_clipboard_text(text: &str) -> Option<bool> {
    call_string_bool("setClipboardText", text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bridge_without_vm_is_a_noop() {
        // No `JavaVM` is installed in a host-side unit test. Every helper must
        // return quietly instead of panicking.
        assert!(!call_void("requestServiceRestart"));
        assert!(!call_string_void("onEvent", "{}"));
        assert!(!call_two_strings_void("notify", "a", "b"));
        assert!(!call_int_string_void("onLog", 2, "m"));
        assert!(call_string("deviceName").is_none());
        assert!(call_int("deviceType").is_none());
        assert!(call_string_bool("setClipboardText", "x").is_none());

        on_devices_changed("[]");
        on_event("{}");
        on_clipboard_changed("x");
        on_log(2, "m");
        notify("t", "m");
        request_service_restart();
    }
}
