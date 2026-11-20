// SPDX-License-Identifier: GPL-2.0-only OR GPL-3.0-only OR LicenseRef-KDE-Accepted-GPL
package org.kde.kdeconnect.plus

import android.content.ComponentName
import android.content.Context
import android.content.ServiceConnection
import android.content.pm.PackageManager
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.os.ParcelFileDescriptor
import android.os.RemoteException
import android.util.Log
import androidx.annotation.WorkerThread
import rikka.shizuku.Shizuku
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/**
 * App-side Shizuku bridge. Keeps the [ShizukuFileService] UserService bound and exposes plain
 * synchronous filesystem methods to the rest of the app. All real filesystem work happens inside
 * the shell/root process; this object only forwards calls over the binder.
 *
 * UserService startup is asynchronous, so every API waits up to [BIND_TIMEOUT_MS] for the binder
 * before returning a safe default. None of these methods change threads; callers should invoke
 * them off the UI thread (they are annotated [WorkerThread]).
 */
object ShizukuFileBridge {

    private const val TAG = "KdeConnectPlus/FileBridge"
    private const val REQUEST_CODE = 0x4b46 // "KF"
    private const val BIND_TIMEOUT_MS = 3000L

    @Volatile private var service: IShellFileService? = null
    @Volatile private var appContext: Context? = null
    @Volatile private var registered = false
    @Volatile private var bindInFlight = false
    @Volatile private var bindLatch: CountDownLatch? = null

    private val mainHandler = Handler(Looper.getMainLooper())

    private val binderReceivedListener = Shizuku.OnBinderReceivedListener {
        Log.i(TAG, "Shizuku binder received, (re)binding file user service")
        ensureBound()
    }

    private val binderDeadListener = Shizuku.OnBinderDeadListener {
        Log.w(TAG, "Shizuku binder died, dropping file service")
        dropService()
    }

    private val permissionResultListener =
        Shizuku.OnRequestPermissionResultListener { requestCode, grantResult ->
            if (requestCode == REQUEST_CODE) {
                if (grantResult == PackageManager.PERMISSION_GRANTED) {
                    ensureBound()
                } else {
                    Log.w(TAG, "Shizuku permission denied for file service")
                }
            }
        }

    private val connection = object : ServiceConnection {
        override fun onServiceConnected(name: ComponentName?, binder: IBinder?) {
            bindInFlight = false
            val svc = IShellFileService.Stub.asInterface(binder)
            if (svc == null) {
                Log.e(TAG, "connected but binder is null")
                bindLatch?.countDown()
                return
            }
            service = svc
            bindLatch?.countDown()
            Log.i(TAG, "file user service connected")
        }

        override fun onServiceDisconnected(name: ComponentName?) {
            Log.w(TAG, "file user service disconnected")
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

    /** True when Shizuku is running, permission is granted and the UserService is connected. */
    fun isAvailable(): Boolean = isShizukuAvailable() && hasPermission() && service != null

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

    @WorkerThread
    fun listDirectory(path: String): Array<String>? =
        call<Array<String>?>(null) { it.listDirectory(path) }

    @WorkerThread
    fun stat(path: String, followLinks: Boolean): String? =
        call<String?>(null) { it.stat(path, followLinks) }

    @WorkerThread
    fun exists(path: String): Boolean = call(false) { it.exists(path) }

    @WorkerThread
    fun canonicalPath(path: String): String? = call<String?>(null) { it.canonicalPath(path) }

    @WorkerThread
    fun readLink(path: String): String? = call<String?>(null) { it.readLink(path) }

    @WorkerThread
    fun openFile(path: String, mode: String): ParcelFileDescriptor? =
        call<ParcelFileDescriptor?>(null) { it.openFile(path, mode) }

    @WorkerThread
    fun createDirectory(path: String): Boolean = call(false) { it.createDirectory(path) }

    @WorkerThread
    fun delete(path: String): Boolean = call(false) { it.delete(path) }

    @WorkerThread
    fun rename(from: String, to: String): Boolean = call(false) { it.rename(from, to) }

    @WorkerThread
    fun copyFile(from: String, to: String, overwrite: Boolean): Boolean =
        call(false) { it.copyFile(from, to, overwrite) }

    @WorkerThread
    fun setLastModified(path: String, time: Long): Boolean =
        call(false) { it.setLastModified(path, time) }

    @WorkerThread
    fun chmod(path: String, mode: Int): Boolean = call(false) { it.chmod(path, mode) }

    @WorkerThread
    fun createSymlink(target: String, link: String): Boolean =
        call(false) { it.createSymlink(target, link) }

    @WorkerThread
    fun usableSpace(path: String): Long = call(0L) { it.usableSpace(path) }

    @WorkerThread
    fun totalSpace(path: String): Long = call(0L) { it.totalSpace(path) }

    /**
     * Forward one binder call. Waits for the service to connect, then runs [block] with the
     * interface. Any failure (including [RemoteException]) is logged and mapped to [default] so a
     * dead/remote service never crashes the caller.
     */
    private fun <T> call(default: T, block: (IShellFileService) -> T): T {
        val svc = awaitService() ?: return default
        return runCatching { block(svc) }
            .onFailure { t ->
                if (t is RemoteException) {
                    Log.w(TAG, "remote file service call failed", t)
                } else {
                    Log.w(TAG, "file service call failed", t)
                }
            }
            .getOrDefault(default)
    }

    /** Return the connected service, triggering a bind and waiting up to 3 s if necessary. */
    private fun awaitService(): IShellFileService? {
        service?.let { return it }
        ensureBound()
        val latch = bindLatch
        if (latch != null && latch.count > 0L && service == null) {
            runCatching {
                if (!latch.await(BIND_TIMEOUT_MS, TimeUnit.MILLISECONDS)) {
                    Log.w(TAG, "timed out waiting for file service binder")
                }
            }.onFailure { Log.w(TAG, "interrupted waiting for file service binder", it) }
        }
        return service
    }

    private fun isShizukuAvailable(): Boolean = runCatching {
        Shizuku.pingBinder()
    }.getOrDefault(false)

    private fun hasPermission(): Boolean = runCatching {
        Shizuku.checkSelfPermission() == PackageManager.PERMISSION_GRANTED
    }.getOrDefault(false)

    @Synchronized
    private fun ensureBound() {
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
        bindLatch = CountDownLatch(1)
        val args = Shizuku.UserServiceArgs(
            ComponentName(context.packageName, ShizukuFileService::class.java.name)
        )
            .daemon(true)
            .processNameSuffix("files")
            .debuggable(false)
            // Bump when the AIDL or the service implementation changes.
            .version(1)
        runCatching {
            Shizuku.bindUserService(args, connection)
            Log.i(TAG, "bindUserService requested for file service")
        }.onFailure {
            bindInFlight = false
            bindLatch?.countDown()
            Log.e(TAG, "bindUserService failed for file service", it)
        }
    }

    @Synchronized
    private fun dropService() {
        service = null
        bindInFlight = false
        bindLatch?.countDown()
    }
}
