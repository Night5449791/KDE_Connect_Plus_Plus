// SPDX-License-Identifier: GPL-2.0-only OR GPL-3.0-only OR LicenseRef-KDE-Accepted-GPL
package org.kde.kdeconnect.plus

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.os.Handler
import android.os.HandlerThread
import android.os.IBinder
import android.util.Log
import java.util.concurrent.atomic.AtomicBoolean

/**
 * Shizuku UserService used to reach the clipboard with the shell identity.
 *
 * Shizuku starts a UserService as shell (uid 2000, adb / wireless-debug) or as root (uid 0,
 * root / Sui). Both are usable here:
 *  - shell: attach to the clipboard directly, we already are the shell user.
 *  - root: never touch the clipboard from uid 0 (Android 12+ wipes it), shell out to
 *    `cmd clipboard` instead, which runs with the shell identity.
 */
class ShizukuClipboardService : IClipboardService.Stub() {

    private val watchers = LinkedHashSet<IClipboardWatcher>()
    private var handlerThread: HandlerThread? = null
    private var handler: Handler? = null
    private var backend: ClipboardBackend? = null

    override fun getPrimaryClipText(): String? = backendOrNull()?.read()

    override fun setPrimaryClipText(text: String): Boolean = backendOrNull()?.write(text) ?: false

    override fun registerWatcher(watcher: IClipboardWatcher) {
        val count = synchronized(watchers) {
            watchers.add(watcher)
            watchers.size
        }
        Log.i(TAG, "clipboard watcher registered (count=$count)")
        startWatching()
    }

    override fun unregisterWatcher(watcher: IClipboardWatcher) {
        val empty = synchronized(watchers) {
            watchers.remove(watcher)
            watchers.isEmpty()
        }
        if (empty) stopWatching()
    }

    @Synchronized
    private fun backendOrNull(): ClipboardBackend? {
        backend?.let { return it }
        val created = runCatching { ClipboardBackend.create() }
            .onFailure { Log.e(TAG, "cannot create clipboard backend", it) }
            .onSuccess { Log.i(TAG, "clipboard backend ready: ${it.javaClass.simpleName}") }
            .getOrNull()
        backend = created
        return created
    }

    @Synchronized
    private fun startWatching() {
        if (handler != null) return
        val thread = HandlerThread("plus-clipboard-watch").also { it.start() }
        handlerThread = thread
        handler = Handler(thread.looper)
        handler?.post(watchLoop)
    }

    @Synchronized
    private fun stopWatching() {
        handler?.removeCallbacksAndMessages(null)
        handlerThread?.quitSafely()
        handlerThread = null
        handler = null
        backend?.close()
        backend = null
    }

    private val watchLoop = object : Runnable {
        override fun run() {
            val current = backendOrNull()
            val changed = current?.pollChanged()
            if (changed != null) {
                Log.d(TAG, "clipboard change detected (chars=${changed.length})")
                val snapshot = synchronized(watchers) { watchers.toList() }
                for (watcher in snapshot) {
                    val alive = runCatching { watcher.onClipboardChanged(changed) }.isSuccess
                    if (!alive) synchronized(watchers) { watchers.remove(watcher) }
                }
            }
            val delay = if (current is DirectClipboardBackend) POLL_INTERVAL_MS else SHELL_POLL_INTERVAL_MS
            handler?.postDelayed(this, delay)
        }
    }

    companion object {
        private const val TAG = "KdeConnectPlus/ClipSvc"
        /** Direct clipboard reads are cheap; shell reads spawn a process so poll them slower. */
        private const val POLL_INTERVAL_MS = 700L
        private const val SHELL_POLL_INTERVAL_MS = 1500L

        /**
         * Entry point Shizuku calls to instantiate the service: a public static no-arg method
         * returning an IBinder.
         */
        @JvmStatic
        fun create(): IBinder {
            Log.i(TAG, "creating clipboard user service, uid=${android.os.Process.myUid()}")
            return ShizukuClipboardService()
        }
    }
}

/** How this process reaches the clipboard. */
internal interface ClipboardBackend {
    fun read(): String?
    fun write(text: String): Boolean
    /** New clipboard text when it changed since the last poll, else null. */
    fun pollChanged(): String?
    fun close()

    companion object {
        const val SHELL_UID = 2000

        fun create(): ClipboardBackend =
            if (android.os.Process.myUid() == SHELL_UID) {
                DirectClipboardBackend()
            } else {
                ShellCommandBackend()
            }
    }
}

/** Clipboard access from a process that already runs as the shell user. */
internal class DirectClipboardBackend : ClipboardBackend {
    private val context: Context? = currentApplication()?.let { applicationContext ->
        runCatching { applicationContext.createPackageContext("com.android.shell", 0) }
            .onFailure { Log.e("KdeConnectPlus/ClipSvc", "cannot create shell package context", it) }
            .getOrNull()
    }
    private val cm: ClipboardManager? =
        context?.getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager

    @Volatile private var lastSeen: String? = null
    private val readFailureLogged = AtomicBoolean(false)

    init {
        Log.i("KdeConnectPlus/ClipSvc", "direct backend context=${context?.packageName}, clipboardManager=${cm != null}")
        lastSeen = read()
    }

    override fun read(): String? = runCatching {
        val clip = cm?.primaryClip ?: return null
        if (clip.itemCount <= 0) return null
        clip.getItemAt(0).coerceToText(context).toString()
    }.onFailure {
        if (readFailureLogged.compareAndSet(false, true)) {
            Log.w("KdeConnectPlus/ClipSvc", "direct clipboard read failed", it)
        }
    }.onSuccess { readFailureLogged.set(false) }
        .getOrNull()

    override fun write(text: String): Boolean = runCatching {
        cm?.setPrimaryClip(ClipData.newPlainText(null, text))
        lastSeen = text
        true
    }.getOrDefault(false)

    override fun pollChanged(): String? {
        val current = read() ?: return null
        if (current == lastSeen) return null
        lastSeen = current
        Log.d("KdeConnectPlus/ClipSvc", "direct backend detected clipboard change (chars=${current.length})")
        return current
    }

    override fun close() { }

    private fun currentApplication(): Context? = runCatching {
        val at = Class.forName("android.app.ActivityThread")
        at.getMethod("currentApplication").invoke(null) as? Context
    }.getOrNull()
}

/**
 * Root/Sui mode. uid 0 must not attach to the clipboard, so every operation goes through
 * `cmd clipboard` executed with the shell identity.
 */
internal class ShellCommandBackend : ClipboardBackend {
    @Volatile private var lastSeen: String? = null

    override fun read(): String? =
        ShellClipboardCommands.parseGet(exec(ShellClipboardCommands.GET))

    override fun write(text: String): Boolean {
        val ok = exec(ShellClipboardCommands.set(text)) != null
        if (ok) lastSeen = text
        return ok
    }

    override fun pollChanged(): String? {
        val current = read() ?: return null
        if (current == lastSeen) return null
        lastSeen = current
        return current
    }

    override fun close() { }

    /** Runs a shell command and returns its stdout. */
    private fun exec(command: String): String? = runCatching {
        val process = ProcessBuilder("sh", "-c", command)
            .redirectErrorStream(true)
            .start()
        val out = process.inputStream.bufferedReader().use { it.readText() }
        val exitCode = process.waitFor()
        if (exitCode != 0 || ShellClipboardCommands.isCommandError(out)) null else out
    }.onFailure { Log.w("KdeConnectPlus/Shell", "command failed: $command", it) }
        .getOrNull()
}
