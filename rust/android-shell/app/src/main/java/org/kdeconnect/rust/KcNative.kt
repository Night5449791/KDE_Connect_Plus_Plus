package org.kdeconnect.rust

/**
 * Kotlin → Rust 的 JNI 声明。
 *
 * 契约见 `rust/docs/JNI_CONTRACT.md`，实现见 `crates/kc-android/src/jni.rs`。
 * 这里只声明函数，不持有任何状态；动态库在 [KcApp] 中加载。所有方法都可从任意线程调用。
 */
object KcNative {
    @JvmStatic external fun init(dataDir: String): Boolean

    /** 设备快照 JSON 数组；即使没有设备也返回 `[]`。 */
    @JvmStatic external fun devices(): String

    @JvmStatic external fun pair(deviceId: String): Boolean

    @JvmStatic external fun accept(deviceId: String): Boolean

    @JvmStatic external fun unpair(deviceId: String): Boolean

    @JvmStatic external fun ping(deviceId: String, message: String): Boolean

    @JvmStatic external fun setClipboard(deviceId: String, text: String): Boolean

    @JvmStatic external fun find(deviceId: String): Boolean

    @JvmStatic external fun refresh(): Boolean

    @JvmStatic external fun stop()
}
