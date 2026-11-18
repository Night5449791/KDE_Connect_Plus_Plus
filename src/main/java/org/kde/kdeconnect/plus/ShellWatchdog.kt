// SPDX-License-Identifier: GPL-2.0-only OR GPL-3.0-only OR LicenseRef-KDE-Accepted-GPL
package org.kde.kdeconnect.plus

import android.util.Log
import org.kde.kdeconnect_tp.BuildConfig
import java.util.concurrent.Executors
import java.util.concurrent.ScheduledExecutorService
import java.util.concurrent.TimeUnit

/**
 * Keeps the app's BackgroundService alive from a shell-identity (uid 2000) process.
 *
 * The app process and its foreground service can be killed by the system or by vendor ROMs (vivo
 * OriginOS and friends). A normal app process cannot reliably restart its own foreground service
 * from the background on Android 12+; this watchdog runs inside the Shizuku UserService (already
 * uid 2000), which is exempt from the background-FGS restriction. It periodically checks whether
 * BackgroundService is running and, if not, clears the stopped state and starts the service via
 * `am start-foreground-service` with the shell identity.
 */
internal class ShellWatchdog {

    private val executor: ScheduledExecutorService =
        Executors.newSingleThreadScheduledExecutor { r ->
            Thread(r, "plus-watchdog").apply { isDaemon = true }
        }

    private val packageName: String = BuildConfig.APPLICATION_ID
    private val serviceComponent: String = "$packageName/org.kde.kdeconnect.BackgroundService"
    @Volatile private var enabled = false

    /** Start the periodic check. Called from the UserService process only. */
    fun start() {
        if (enabled) return
        enabled = true
        Log.i(TAG, "starting watchdog for $packageName (uid=${android.os.Process.myUid()})")
        // Apply bypasses once on startup, then check service liveness periodically.
        refreshBypasses()
        executor.scheduleWithFixedDelay({ safeCheck() }, INITIAL_DELAY_S, CHECK_INTERVAL_S, TimeUnit.SECONDS)
    }

    fun stop() {
        enabled = false
        executor.shutdownNow()
    }

    /** Public entry point used by the AIDL requestRelaunch()/refreshBypasses(). */
    fun requestRelaunch() {
        executor.execute { safeCheck() }
    }

    fun refreshBypasses() {
        executor.execute { applyBypasses() }
    }

    private fun safeCheck() {
        runCatching {
            if (isServiceRunning()) {
                Log.d(TAG, "BackgroundService alive")
            } else {
                Log.i(TAG, "BackgroundService not running, relaunching")
                relaunch()
            }
        }.onFailure { Log.w(TAG, "watchdog check failed", it) }
    }

    /** Use `dumpsys activity services` rather than ActivityManager.getRunningServices (limited). */
    private fun isServiceRunning(): Boolean {
        val out = exec("dumpsys activity services $packageName") ?: return false
        return out.contains("ServiceRecord{") &&
            out.contains("org.kde.kdeconnect.BackgroundService")
    }

    private fun relaunch() {
        // 1) Clear the stopped state (vivo force-stop / never-opened app). Android 13+.
        exec("cmd package unstop --user 0 $packageName")
        // 2) Apply the same bypasses again in case the ROM reset them.
        applyBypasses()
        // 3) Shell identity is exempt from background-FGS start restrictions, but the service
        //    must be exported and must call startForeground() within a few seconds.
        val result = exec("am start-foreground-service --user 0 -n $serviceComponent")
        Log.i(TAG, "relaunch result: $result")
    }

    private fun applyBypasses() {
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
        val output = process.inputStream.bufferedReader().use { it.readText() }
        process.waitFor(COMMAND_TIMEOUT_S, TimeUnit.SECONDS)
        output.trim().ifEmpty { null }
    }.onFailure { Log.w(TAG, "exec failed: $command", it) }.getOrNull()

    companion object {
        private const val TAG = "KdeConnectPlus/Watchdog"
        private const val INITIAL_DELAY_S = 10L
        private const val CHECK_INTERVAL_S = 20L
        private const val COMMAND_TIMEOUT_S = 10L
    }
}
