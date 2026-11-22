package org.kdeconnect.rust

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.content.res.Configuration
import android.os.Build
import android.util.Log

/**
 * Rust → Kotlin 的无状态桥接层。
 *
 * Rust 缓存 `JavaVM`，回调时 attach 当前线程并调用这里的 `@JvmStatic` 静态方法。
 * 本文件不包含任何 KDE Connect 逻辑：回调只转发给已注册的 UI listener，
 * 系统 API 方法只做一次调用并返回。不保存设备列表、配对状态或剪贴板历史。
 */
object KcBridge {
    /** UI 只需要关心的四个回调；实现方自行负责切回主线程。 */
    interface UiListener {
        fun onDevicesChanged(json: String)
        fun onEvent(json: String)
        fun onClipboardChanged(text: String)
        fun onLog(level: Int, message: String)
    }

    @Volatile
    private var uiListener: UiListener? = null

    @Volatile
    private var appContext: Context? = null

    @JvmStatic
    fun setUiListener(listener: UiListener) {
        uiListener = listener
    }

    @JvmStatic
    fun clearUiListener(listener: UiListener) {
        if (uiListener === listener) uiListener = null
    }

    /** 由前台服务在外壳启动时调用，仅为系统 API 适配器提供 Application context。 */
    @JvmStatic
    fun attach(context: Context) {
        appContext = context.applicationContext
    }

    // ---- Rust 主动推送的回调 ---------------------------------------------

    @JvmStatic
    fun onDevicesChanged(json: String) {
        uiListener?.onDevicesChanged(json)
    }

    @JvmStatic
    fun onEvent(json: String) {
        uiListener?.onEvent(json)
    }

    @JvmStatic
    fun onClipboardChanged(text: String) {
        // 远端剪贴板：一次系统写入，然后（若 UI 存在）给出提示。
        setClipboardText(text)
        uiListener?.onClipboardChanged(text)
    }

    @JvmStatic
    fun onLog(level: Int, message: String) {
        when (level) {
            1 -> Log.e(TAG, message)
            2 -> Log.w(TAG, message)
            3 -> Log.i(TAG, message)
            4 -> Log.d(TAG, message)
            else -> Log.v(TAG, message)
        }
        uiListener?.onLog(level, message)
    }

    // ---- 无状态 Android 系统 API 适配器（Rust 主动拉取）------------------

    @JvmStatic
    fun deviceName(): String {
        val raw = Build.MODEL ?: "Android"
        return raw.trim().replace('\n', ' ').replace('\r', ' ').take(64).ifEmpty { "Android" }
    }

    /** 0=phone, 1=tablet, 2=desktop, 3=laptop, 4=tv。Android 只能可靠推断 phone/tablet/tv。 */
    @JvmStatic
    fun deviceType(): Int {
        val context = appContext ?: return 0
        return try {
            val pm = context.packageManager
            when {
                pm.hasSystemFeature(PackageManager.FEATURE_TELEVISION) -> 4
                isLargeScreen(context) -> 1
                else -> 0
            }
        } catch (t: Throwable) {
            Log.w(TAG, "deviceType failed", t)
            0
        }
    }

    private fun isLargeScreen(context: Context): Boolean {
        val layout = context.resources.configuration.screenLayout and
            Configuration.SCREENLAYOUT_SIZE_MASK
        return layout >= Configuration.SCREENLAYOUT_SIZE_LARGE
    }

    @JvmStatic
    fun clipboardText(): String? {
        val context = appContext ?: return null
        return try {
            val manager = context.getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager
                ?: return null
            val clip = manager.primaryClip ?: return null
            if (clip.itemCount == 0) return null
            clip.getItemAt(0)?.text?.toString()
        } catch (t: Throwable) {
            Log.w(TAG, "clipboardText failed", t)
            null
        }
    }

    @JvmStatic
    fun setClipboardText(text: String): Boolean {
        val context = appContext ?: return false
        return try {
            val manager = context.getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager
                ?: return false
            manager.setPrimaryClip(ClipData.newPlainText("KDE Connect", text))
            true
        } catch (t: Throwable) {
            Log.w(TAG, "setClipboardText failed", t)
            false
        }
    }

    @JvmStatic
    fun notify(title: String, message: String) {
        val context = appContext ?: return
        try {
            val manager = context.getSystemService(Context.NOTIFICATION_SERVICE) as? NotificationManager
                ?: return
            manager.createNotificationChannel(
                NotificationChannel(NOTIFY_CHANNEL_ID, "KC Rust", NotificationManager.IMPORTANCE_DEFAULT),
            )
            val pending = PendingIntent.getActivity(
                context,
                0,
                Intent(context, MainActivity::class.java),
                PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
            )
            val notification: Notification = Notification.Builder(context, NOTIFY_CHANNEL_ID)
                .setContentTitle(title)
                .setContentText(message)
                .setSmallIcon(android.R.drawable.stat_notify_more)
                .setContentIntent(pending)
                .build()
            manager.notify(NOTIFY_ID, notification)
        } catch (t: Throwable) {
            Log.w(TAG, "notify failed", t)
        }
    }

    @JvmStatic
    fun requestServiceRestart() {
        val context = appContext ?: return
        val intent = Intent(context, KcService::class.java)
        try {
            context.stopService(intent)
            context.startForegroundService(intent)
        } catch (t: Throwable) {
            Log.w(TAG, "requestServiceRestart failed", t)
        }
    }

    private const val TAG = "KcBridge"
    private const val NOTIFY_CHANNEL_ID = "kc-rust-notify"
    private const val NOTIFY_ID = 2
}
