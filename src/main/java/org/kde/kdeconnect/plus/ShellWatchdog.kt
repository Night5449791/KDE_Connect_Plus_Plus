// SPDX-License-Identifier: GPL-2.0-only OR GPL-3.0-only OR LicenseRef-KDE-Accepted-GPL
package org.kde.kdeconnect.plus

import android.util.Log
import org.kde.kdeconnect_tp.BuildConfig

/**
 * Keeps the app's BackgroundService alive using a standalone shell-identity watchdog process.
 *
 * Why not a timer inside the Shizuku UserService: when the app process is force-stopped / frozen
 * (vivo OriginOS and friends), Shizuku cancels the UserService's binder work, so an in-process
 * timer stops exactly when it is needed most. A completely detached `nohup sh` process, started
 * with shell (uid 2000) privileges, keeps running across force-stop/reboot-freeze and can relaunch
 * the app's foreground service via `am start-foreground-service` (shell is exempt from the
 * background-FGS restriction).
 */
internal object ShellWatchdog {

    private const val TAG = "KdeConnectPlus/Watchdog"
    private const val MARKER = "kc-watchdog-loop"
    private const val PID_FILE = "/data/local/tmp/kc_watchdog.pid"

    private val packageName: String = BuildConfig.APPLICATION_ID
    private val serviceComponent: String = "$packageName/org.kde.kdeconnect.BackgroundService"

    /** Start (or re-attach to) the detached watchdog. Called from the UserService process. */
    fun start() {
        ensureRunning()
    }

    /** Launch the detached watchdog script if it is not already running. */
    private fun ensureRunning() {
        // The script records its own PID; we use `kill -0 <pid>` to check liveness precisely
        // (pgrep -f would also match the very shell command that performs the check).
        val alive = exec("test -f $PID_FILE && kill -0 \$(cat $PID_FILE) 2>/dev/null && echo yes")
        if (alive == "yes") {
            Log.i(TAG, "watchdog already running (pid file present)")
            return
        }
        val script = buildString {
            append("echo \$\$ > $PID_FILE; ")
            append("PKG=$packageName; SVC=$serviceComponent; ")
            append("while true; do ")
            append("if ! dumpsys activity services \$PKG 2>/dev/null | grep -q 'org.kde.kdeconnect.BackgroundService'; then ")
            append("cmd package unstop --user 0 \$PKG >/dev/null 2>&1; ")
            append("cmd appops set \$PKG RUN_ANY_IN_BACKGROUND allow >/dev/null 2>&1; ")
            append("am set-standby-bucket \$PKG active >/dev/null 2>&1; ")
            append("dumpsys deviceidle whitelist +\$PKG >/dev/null 2>&1; ")
            append("am start-foreground-service --user 0 -n \$SVC >/dev/null 2>&1; ")
            append("fi; sleep 20; done # $MARKER")
        }
        // `nohup ... &` detaches the loop from this process and from the Shizuku UserService.
        val result = exec("nohup sh -c '$script' >/dev/null 2>&1 &")
        Log.i(TAG, "started detached watchdog: result=$result")
    }

    /** Force an immediate check+relaunch from the shell (used by boot receiver via AIDL). */
    fun requestRelaunch() {
        // Apply bypasses and start the service immediately, in addition to the periodic loop.
        exec("cmd package unstop --user 0 $packageName")
        refreshBypasses()
        val result = exec("am start-foreground-service --user 0 -n $serviceComponent")
        Log.i(TAG, "immediate relaunch result=$result")
        ensureRunning()
    }

    /** Refresh the shell-side ROM bypasses. */
    fun refreshBypasses() {
        exec("cmd appops set $packageName RUN_ANY_IN_BACKGROUND allow")
        exec("cmd appops set $packageName RUN_IN_BACKGROUND allow")
        exec("cmd appops set $packageName AUTO_REVOKE_PERMISSIONS_IF_UNUSED ignore")
        exec("am set-standby-bucket $packageName active")
        exec("dumpsys deviceidle whitelist +$packageName")
    }

    private fun exec(command: String): String? = runCatching {
        val process = ProcessBuilder("/system/bin/sh", "-c", command)
            .redirectErrorStream(true)
            .start()
        // Wait FIRST with a timeout: reading stdout before waitFor() can block forever when a
        // started service inherits stdout, which would hang the shell thread.
        val finished = process.waitFor(COMMAND_TIMEOUT_S, java.util.concurrent.TimeUnit.SECONDS)
        if (!finished) process.destroyForcibly()
        val available = process.inputStream.available()
        val output = if (available > 0) {
            ByteArray(available).let { buf ->
                val n = process.inputStream.read(buf)
                if (n > 0) String(buf, 0, n) else ""
            }
        } else ""
        output.trim().ifEmpty { null }
    }.onFailure { Log.w(TAG, "exec failed: $command", it) }.getOrNull()

    private const val COMMAND_TIMEOUT_S = 10L
}
