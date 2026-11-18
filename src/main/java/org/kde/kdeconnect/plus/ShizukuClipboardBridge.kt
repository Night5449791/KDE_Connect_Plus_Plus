// SPDX-License-Identifier: GPL-2.0-only OR GPL-3.0-only OR LicenseRef-KDE-Accepted-GPL
package org.kde.kdeconnect.plus

import android.content.ComponentName
import android.content.Context
import android.content.ServiceConnection
import android.content.pm.PackageManager
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.util.Log
import org.kde.kdeconnect.BackgroundService
import rikka.shizuku.Shizuku
import java.util.concurrent.CopyOnWriteArraySet

/**
 * App-side Shizuku bridge. Keeps a UserService running as shell and exposes plain read / write /
 * observe methods to the clipboard plugin.
 *
 * Also owns the "auto relaunch" behaviour: every time Shizuku's binder comes back (Shizuku
 * restarted, system killed it, device rebooted) the UserService is re-bound and the local
 * clipboard listener restarted, so clipboard sync recovers without the user opening the app.
 */
object ShizukuClipboardBridge {

    private const val TAG = "KdeConnectPlus/Shizuku"
    private const val REQUEST_CODE = 0x4b44 // "KD"

    interface Listener {
        /** Called on a binder thread with text copied while we were able to watch the clipboard. */
        fun onShellClipboardChanged(text: String)
    }

    private val listeners = CopyOnWriteArraySet<Listener>()
    private val mainHandler = Handler(Looper.getMainLooper())

    @Volatile private var service: IClipboardService? = null
    @Volatile private var appContext: Context? = null
    @Volatile private var registered = false
    @Volatile private var bindInFlight = false

    private val watcher = object : IClipboardWatcher.Stub() {
        override fun onClipboardChanged(text: String) {
            Log.d(TAG, "clipboard watcher callback received (chars=${text.length})")
            for (listener in listeners) {
                runCatching { listener.onShellClipboardChanged(text) }
            }
        }
    }

    private val binderReceivedListener = Shizuku.OnBinderReceivedListener {
        Log.i(TAG, "Shizuku binder received, (re)binding user service")
        // kdeconnect+ auto relaunch: after Shizuku restarts, Android may have killed our
        // BackgroundService (or never restarted it), so bring the app back up before binding.
        relaunchBackgroundService()
        ensureBound()
    }

    private val binderDeadListener = Shizuku.OnBinderDeadListener {
        Log.w(TAG, "Shizuku binder died, dropping clipboard service")
        dropService()
    }

    /**
     * kdeconnect+ : Shizuku already runs as shell/root, so it can start our service even from a
     * cold process state. Falls back to a normal start when the shell path is unavailable.
     */
    private fun relaunchBackgroundService() {
        val context = appContext ?: return
        runCatching { BackgroundService.Start(context) }
            .onFailure { Log.w(TAG, "relaunch of BackgroundService failed", it) }
    }

    private val permissionResultListener =
        Shizuku.OnRequestPermissionResultListener { requestCode, grantResult ->
            if (requestCode == REQUEST_CODE) {
                if (grantResult == PackageManager.PERMISSION_GRANTED) {
                    ensureBound()
                } else {
                    Log.w(TAG, "Shizuku permission denied")
                }
            }
        }

    private val connection = object : ServiceConnection {
        override fun onServiceConnected(name: ComponentName?, binder: IBinder?) {
            bindInFlight = false
            val svc = IClipboardService.Stub.asInterface(binder)
            if (svc == null) {
                Log.e(TAG, "connected but binder is null")
                return
            }
            service = svc
            Log.i(TAG, "clipboard user service connected")
            runCatching { svc.registerWatcher(watcher) }
                .onFailure { Log.e(TAG, "registerWatcher failed", it) }
            // Push the current clip once so a freshly started app syncs the latest value.
            runCatching { svc.getPrimaryClipText() }.getOrNull()?.let { text ->
                for (listener in listeners) {
                    runCatching { listener.onShellClipboardChanged(text) }
                }
            }
        }

        override fun onServiceDisconnected(name: ComponentName?) {
            Log.w(TAG, "clipboard user service disconnected")
            service = null
            bindInFlight = false
        }
    }

    /** Registers Shizuku callbacks once; safe to call from Application.onCreate. */
    fun install(context: Context) {
        appContext = context.applicationContext
        if (registered) return
        registered = true
        Shizuku.addBinderReceivedListenerSticky(binderReceivedListener)
        Shizuku.addBinderDeadListener(binderDeadListener)
        Shizuku.addRequestPermissionResultListener(permissionResultListener)
        ensureBound()
    }

    fun addListener(listener: Listener) {
        listeners.add(listener)
        ensureBound()
    }

    fun removeListener(listener: Listener) {
        listeners.remove(listener)
    }

    /** True when Shizuku is running and we hold the clipboard UserService. */
    fun isReady(): Boolean = service != null

    /** True when Shizuku is installed and its binder is reachable. */
    fun isShizukuAvailable(): Boolean = runCatching {
        Shizuku.pingBinder()
    }.getOrDefault(false)

    fun hasPermission(): Boolean = runCatching {
        Shizuku.checkSelfPermission() == PackageManager.PERMISSION_GRANTED
    }.getOrDefault(false)

    /** Asks Shizuku for permission on the main thread; the result listener re-binds on grant. */
    fun requestPermission() {
        if (!isShizukuAvailable() || hasPermission()) {
            ensureBound()
            return
        }
        mainHandler.post {
            runCatching { Shizuku.requestPermission(REQUEST_CODE) }
                .onFailure { Log.e(TAG, "requestPermission failed", it) }
        }
    }

    fun readText(): String? = runCatching { service?.getPrimaryClipText() }.getOrNull()

    fun writeText(text: String): Boolean =
        runCatching { service?.setPrimaryClipText(text) }.getOrNull() ?: false

    @Synchronized
    fun ensureBound() {
        val context = appContext ?: return
        if (!isShizukuAvailable()) return
        if (!hasPermission()) return
        if (service != null) return
        if (bindInFlight) return
        // bindUserService needs Shizuku API 10 (Shizuku 12.x+).
        if (runCatching { Shizuku.getVersion() }.getOrDefault(0) < 10) {
            Log.w(TAG, "Shizuku too old for UserService, needs API 10")
            return
        }
        bindInFlight = true
        val args = Shizuku.UserServiceArgs(
            ComponentName(context.packageName, ShizukuClipboardService::class.java.name)
        )
            .daemon(true)
            .processNameSuffix("clipboard")
            .debuggable(false)
            // Bump when the AIDL or the service implementation changes.
            .version(1)
        runCatching {
            Shizuku.bindUserService(args, connection)
            Log.i(TAG, "bindUserService requested")
        }.onFailure {
            bindInFlight = false
            Log.e(TAG, "bindUserService failed", it)
        }
    }

    @Synchronized
    private fun dropService() {
        runCatching { service?.unregisterWatcher(watcher) }
        service = null
        bindInFlight = false
    }
}

