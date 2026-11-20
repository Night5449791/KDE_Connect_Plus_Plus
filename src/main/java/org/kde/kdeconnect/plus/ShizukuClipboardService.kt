// SPDX-License-Identifier: GPL-2.0-only OR GPL-3.0-only OR LicenseRef-KDE-Accepted-GPL
package org.kde.kdeconnect.plus

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.os.Build
import android.os.Handler
import android.os.HandlerThread
import android.os.IBinder
import android.os.IInterface
import android.os.PowerManager
import android.util.Log
import java.util.concurrent.atomic.AtomicBoolean

/**
 * Change detector for potentially large clipboard strings. Length + cached hashCode gate the
 * expensive `equals`, so a 100KB clip is not compared character by character on every poll.
 */
internal class LastSeenText {
    @Volatile private var text: String? = null
    @Volatile private var length = -1
    @Volatile private var hash = 0

    /** Returns the text when it differs from the last value, null when it is unchanged. */
    fun onRead(current: String): String? {
        val previous = text
        if (previous != null && current.length == length && current.hashCode() == hash && current == previous) {
            return null
        }
        text = current
        length = current.length
        hash = current.hashCode()
        return current
    }

    fun set(value: String?) {
        text = value
        length = value?.length ?: -1
        hash = value?.hashCode() ?: 0
    }
}

/**
 * Shizuku UserService used to reach the clipboard with the shell identity.
 *
 * Shizuku starts a UserService as shell (uid 2000, adb / wireless-debug) or as root (uid 0,
 * root / Sui). Both are usable here:
 *  - shell: attach to the clipboard directly, we already are the shell user.
 *  - root: never touch the clipboard from uid 0 (Android 12+ wipes it), shell out to
 *    `cmd clipboard` instead, which runs with the shell identity.
 *
 * Watching strategy (cheapest first):
 *  1. In shell mode register an AOSP `IOnPrimaryClipChangedListener` binder on `IClipboard`.
 *     A callback reads the clip once and fans it out to our watchers; no polling at all.
 *  2. If that reflection fails (vendor ROM) fall back to polling with exponential backoff,
 *     starting at 700ms (direct) / 1500ms (shell command) and settling at 10s.
 *  3. Screen off or an unusable shell-command backend stops the loop entirely.
 */
class ShizukuClipboardService : IClipboardService.Stub() {

    private val watchers = LinkedHashSet<IClipboardWatcher>()
    private var handlerThread: HandlerThread? = null
    private var handler: Handler? = null
    private var backend: ClipboardBackend? = null

    /** Proxy of `android.content.IOnPrimaryClipChangedListener` when event mode is active. */
    @Volatile private var systemListener: IInterface? = null
    @Volatile private var eventDriven = false
    @Volatile private var pollingStopped = false
    @Volatile private var screenReceiver: BroadcastReceiver? = null

    /** Base interval, decided once in startWatching() from the backend type. */
    @Volatile private var pollIntervalMs = POLL_INTERVAL_MS
    @Volatile private var quietTicks = 0

    @Volatile private var appContext: Context? = null

    override fun getPrimaryClipText(): String? = backendOrNull()?.read()

    override fun setPrimaryClipText(text: String): Boolean {
        val current = backendOrNull() ?: return false
        val written = current.write(text)
        if (written) {
            // write() already refreshed lastSeen. Notify once explicitly as well: some ROMs never
            // deliver the system event, and the plugin-side echo marker (hash queue) is consumed
            // only when a watcher callback arrives. pollChanged() dedupes the genuine event.
            notifyWatchers(text)
        }
        return written
    }

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
        Log.i(TAG, "clipboard watcher unregistered (empty=$empty)")
        // Last watcher gone: no reason to keep a binder/process alive for nobody.
        if (empty) stopWatching()
    }

    override fun requestRelaunch() = ShellWatchdog.requestRelaunch()

    override fun refreshBypasses() = ShellWatchdog.refreshBypasses()

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
        val current = backendOrNull() ?: return
        // Fixed once per session; never re-evaluate the backend type inside the loop.
        pollIntervalMs = if (current is DirectClipboardBackend) POLL_INTERVAL_MS else SHELL_POLL_INTERVAL_MS
        quietTicks = 0
        eventDriven = false
        pollingStopped = false
        val thread = HandlerThread("plus-clipboard-watch").also { it.start() }
        handlerThread = thread
        handler = Handler(thread.looper)

        if (tryStartEventDriven(current)) {
            Log.i(TAG, "event-driven clipboard active; polling not started")
            return
        }
        Log.i(TAG, "event registration unavailable, using backoff polling (base=${pollIntervalMs}ms)")
        registerScreenReceiver()
        handler?.post(watchLoop)
    }

    @Synchronized
    private fun stopWatching() {
        eventDriven = false
        pollingStopped = true
        systemListener?.let { listener ->
            runCatching { ShellClipboardAccess.unregisterPrimaryClipChangedListener(listener) }
                .onFailure { Log.w(TAG, "cannot unregister system clipboard listener", it) }
        }
        systemListener = null
        screenReceiver?.let { receiver ->
            runCatching { context()?.unregisterReceiver(receiver) }
                .onFailure { Log.w(TAG, "cannot unregister screen receiver", it) }
        }
        screenReceiver = null
        handler?.removeCallbacksAndMessages(null)
        handlerThread?.quitSafely()
        handlerThread = null
        handler = null
        backend?.close()
        backend = null
        quietTicks = 0
    }

    /** Registers the AOSP primary-clip listener. Returns false to request polling fallback. */
    private fun tryStartEventDriven(current: ClipboardBackend): Boolean {
        // Only the shell process may touch IClipboard; uid 0 must stay on `cmd clipboard`.
        if (current !is DirectClipboardBackend) return false
        val listener = ShellClipboardAccess.createPrimaryClipChangedListener {
            onSystemClipboardChanged()
        } ?: return false
        val ok = runCatching { ShellClipboardAccess.registerPrimaryClipChangedListener(listener) }
            .onFailure { Log.w(TAG, "system listener registration threw", it) }
            .getOrDefault(false)
        if (ok) {
            systemListener = listener
            eventDriven = true
        } else {
            runCatching { ShellClipboardAccess.unregisterPrimaryClipChangedListener(listener) }
        }
        return ok
    }

    /** Invoked on a binder thread; hop onto the watch thread so reads stay serialised. */
    private fun onSystemClipboardChanged() {
        if (!eventDriven) return
        val target = handler
        if (target == null) {
            readAndNotifyFromEvent()
        } else {
            target.post { readAndNotifyFromEvent() }
        }
    }

    private fun readAndNotifyFromEvent() {
        if (!eventDriven) return
        val changed = runCatching { backendOrNull()?.pollChanged() }
            .onFailure { Log.w(TAG, "system-event clipboard read failed", it) }
            .getOrNull()
        if (changed != null) {
            Log.d(TAG, "clipboard change from system event (chars=${changed.length})")
            quietTicks = 0
            notifyWatchers(changed)
        }
    }

    private fun notifyWatchers(text: String) {
        val snapshot = synchronized(watchers) { watchers.toList() }
        for (watcher in snapshot) {
            val alive = runCatching { watcher.onClipboardChanged(text) }.isSuccess
            if (!alive) synchronized(watchers) { watchers.remove(watcher) }
        }
    }

    private val watchLoop = object : Runnable {
        override fun run() {
            if (eventDriven || pollingStopped) return
            val current = backendOrNull() ?: return
            if (current.unavailable) {
                // e.g. `cmd clipboard` is not implemented on this ROM: stop forking processes.
                Log.w(TAG, "clipboard backend unavailable, stopping polling")
                pollingStopped = true
                return
            }
            if (screenReceiver != null && !isScreenInteractive()) {
                // Wait for ACTION_SCREEN_ON to re-post; nothing to do while the screen is dark.
                Log.d(TAG, "screen off, pausing clipboard polling")
                return
            }
            val changed = runCatching { current.pollChanged() }
                .onFailure { Log.w(TAG, "poll read failed", it) }
                .getOrNull()
            if (changed != null) {
                Log.d(TAG, "clipboard change detected (chars=${changed.length})")
                quietTicks = 0
                notifyWatchers(changed)
            } else {
                quietTicks++
            }
            handler?.postDelayed(this, backoffDelay(quietTicks, pollIntervalMs))
        }
    }

    /** 700/1500ms while active, then 2s -> 5s -> 10s after long quiet streaks. */
    private fun backoffDelay(quiet: Int, base: Long): Long = when {
        quiet < QUIET_TICKS_TO_2S -> base
        quiet < QUIET_TICKS_TO_5S -> 2_000L
        quiet < QUIET_TICKS_TO_10S -> 5_000L
        else -> 10_000L
    }

    private fun context(): Context? {
        appContext?.let { return it }
        val ctx = runCatching {
            Class.forName("android.app.ActivityThread")
                .getMethod("currentApplication").invoke(null) as? Context
        }.getOrNull()
        appContext = ctx
        return ctx
    }

    private fun isScreenInteractive(): Boolean = runCatching {
        (context()?.getSystemService(Context.POWER_SERVICE) as? PowerManager)?.isInteractive ?: true
    }.getOrDefault(true)

    private fun registerScreenReceiver() {
        val ctx = context() ?: return
        if (screenReceiver != null) return
        val receiver = object : BroadcastReceiver() {
            override fun onReceive(context: Context, intent: Intent) {
                if (intent.action != Intent.ACTION_SCREEN_ON || pollingStopped || eventDriven) return
                Log.d(TAG, "screen on, resuming clipboard polling")
                handler?.removeCallbacks(watchLoop)
                quietTicks = 0
                handler?.post(watchLoop)
            }
        }
        runCatching {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                ctx.registerReceiver(
                    receiver, IntentFilter(Intent.ACTION_SCREEN_ON), Context.RECEIVER_NOT_EXPORTED
                )
            } else {
                ctx.registerReceiver(receiver, IntentFilter(Intent.ACTION_SCREEN_ON))
            }
            screenReceiver = receiver
        }.onFailure { Log.w(TAG, "cannot register ACTION_SCREEN_ON receiver", it) }
    }

    companion object {
        private const val TAG = "KdeConnectPlus/ClipSvc"
        /** Direct clipboard reads are cheap; shell reads spawn a process so poll them slower. */
        private const val POLL_INTERVAL_MS = 700L
        private const val SHELL_POLL_INTERVAL_MS = 1500L
        private const val QUIET_TICKS_TO_2S = 20
        private const val QUIET_TICKS_TO_5S = 40
        private const val QUIET_TICKS_TO_10S = 60

        /**
         * Entry point Shizuku calls to instantiate the service: a public static no-arg method
         * returning an IBinder.
         */
        @JvmStatic
        fun create(): IBinder {
            Log.i(TAG, "creating clipboard user service, uid=${android.os.Process.myUid()}")
            return ShizukuClipboardService().also { ShellWatchdog.start() }
        }
    }
}

/** How this process reaches the clipboard. */
internal interface ClipboardBackend {
    fun read(): String?
    fun write(text: String): Boolean
    /** New clipboard text when it changed since the last poll, else null. */
    fun pollChanged(): String?
    /** True when the underlying command/backend cannot work at all; stops the poll loop. */
    val unavailable: Boolean get() = false
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
    // Keep the "com.android.shell" package context for coerceToText on content:// items and as
    // the public-API fallback. The binder path itself passes the calling package explicitly.
    private val context: Context? = currentApplication()?.let { applicationContext ->
        runCatching { applicationContext.createPackageContext("com.android.shell", 0) }
            .onFailure { Log.e("KdeConnectPlus/ClipSvc", "cannot create shell package context", it) }
            .getOrNull()
    }

    private val lastSeen = LastSeenText()
    private val readFailureLogged = AtomicBoolean(false)

    init {
        Log.i("KdeConnectPlus/ClipSvc", "direct backend context=${context?.packageName}")
        lastSeen.set(read())
    }

    override fun read(): String? = runCatching {
        ShellClipboardAccess.read(context)
    }.onFailure {
        if (readFailureLogged.compareAndSet(false, true)) {
            Log.w("KdeConnectPlus/ClipSvc", "direct clipboard read failed", it)
        }
    }.onSuccess { readFailureLogged.set(false) }
        .getOrNull()

    override fun write(text: String): Boolean = runCatching {
        val written = ShellClipboardAccess.write(context, text)
        if (written) lastSeen.set(text)
        written
    }.getOrDefault(false)

    override fun pollChanged(): String? {
        val current = read() ?: return null
        val changed = lastSeen.onRead(current) ?: return null
        Log.d("KdeConnectPlus/ClipSvc", "direct backend detected clipboard change (chars=${changed.length})")
        return changed
    }

    override fun close() { }

    private fun currentApplication(): Context? = runCatching {
        val at = Class.forName("android.app.ActivityThread")
        at.getMethod("currentApplication").invoke(null) as? Context
    }.getOrNull()
}

/**
 * Root/Sui mode. uid 0 must not attach to the clipboard, so every operation goes through
 * `cmd clipboard` executed with the shell identity. If that command is not implemented the
 * backend disables itself so the poll loop stops forking processes every 1.5s.
 */
internal class ShellCommandBackend : ClipboardBackend {
    private val lastSeen = LastSeenText()
    @Volatile private var disabled = false

    override val unavailable: Boolean get() = disabled

    override fun read(): String? =
        ShellClipboardCommands.parseGet(exec(ShellClipboardCommands.GET))

    override fun write(text: String): Boolean {
        val ok = exec(ShellClipboardCommands.set(text)) != null
        if (ok) lastSeen.set(text)
        return ok
    }

    override fun pollChanged(): String? {
        val current = read() ?: return null
        return lastSeen.onRead(current)
    }

    override fun close() { }

    /** Runs a shell command and returns its stdout, or null on any failure. */
    private fun exec(command: String): String? = runCatching {
        val process = ProcessBuilder("sh", "-c", command)
            .redirectErrorStream(true)
            .start()
        val out = process.inputStream.bufferedReader().use { it.readText() }
        val exitCode = process.waitFor()
        val commandError = ShellClipboardCommands.isCommandError(out)
        if (commandError) {
            disabled = true
            Log.w(
                "KdeConnectPlus/ClipSvc",
                "shell clipboard backend disabled (${out.trimStart().take(120)})",
            )
        }
        if (exitCode != 0 || commandError) null else out
    }.onFailure { Log.w("KdeConnectPlus/Shell", "command failed: $command", it) }
        .getOrNull()
}
