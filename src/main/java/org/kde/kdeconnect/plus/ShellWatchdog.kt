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
 *
 * Power: the old loop ran `dumpsys activity services` every 20s (4320x/day); each call forces
 * system_server to dump/format all services (50-300ms CPU). The loop now uses a near-free `pidof`
 * as the primary liveness probe at [WATCHDOG_INTERVAL_SECONDS], with a low-frequency `dumpsys`
 * fallback only as a safety net. All revive commands are merged into a single shell invocation.
 */
internal object ShellWatchdog {

    private const val TAG = "KdeConnectPlus/Watchdog"
    private const val MARKER = "kc-watchdog-loop"
    private const val PID_FILE = "/data/local/tmp/kc_watchdog.pid"

    /**
     * 轮询间隔（秒）。旧值 20s -> 4320 次/天且每次都是重量级 dumpsys；
     * 现在 60s -> 1440 次/天（1/3），且主检测换成微秒级的 pidof，耗电大幅下降。
     * 60s 是"被 force-stop 后尽快复活"与"省电"之间的折中：最坏复活延迟约一个间隔。
     */
    private const val WATCHDOG_INTERVAL_SECONDS = 60L

    /**
     * pidof 万一因 comm 截断/厂商定制一直落空时，才走重量级 `dumpsys activity services` 兜底，
     * 而且不是每轮都走：每 [FALLBACK_DUMPSYS_EVERY] 轮一次 = 60s * 2 = 120s。
     */
    private const val FALLBACK_DUMPSYS_EVERY = 2

    /** Start (or re-attach to) the detached watchdog. Called from the UserService process. */
    fun start() {
        ensureRunning()
    }

    /** Launch the detached watchdog script if it is not already running. */
    private fun ensureRunning() {
        // PID 文件存活检查保持不变：低频，仅 UserService 创建/重连时调用。
        // 用 kill -0 <pid> 精确判活（pgrep -f 会匹配到执行检查的 shell 自身）。
        val alive = exec("test -f $PID_FILE && kill -0 \$(cat $PID_FILE) 2>/dev/null && echo yes")
        if (alive == "yes") {
            Log.i(TAG, "watchdog already running (pid file present)")
            return
        }
        val script = buildDetachedScript()
        // `nohup ... &` detaches the loop from this process and from the Shizuku UserService.
        val result = exec("nohup sh -c '$script' >/dev/null 2>&1 &")
        Log.i(TAG, "started detached watchdog: result=$result")
    }

    /**
     * 生成独立 shell 进程里跑的循环脚本。
     *
     * 存活检测为什么用 pidof 而不是 dumpsys activity services：
     *  - pidof 只读 /proc/<pid>/comm，几乎零开销，不触发 system_server 全量 dump；
     *  - dumpsys activity services 会让 system_server 遍历并格式化所有服务，单次 50-300ms CPU。
     * Android 的 comm 被内核截断到 15 字节：org.kde.kdeconnect_tp 在 comm 里表现为
     * org.kde.kdeconn。因此先查完整包名，未命中再用前 15 字符补查一次，任一命中即视为 app 存活。
     * BackgroundService 运行在主进程内，force-stop 会杀掉整个进程，所以"主进程在 == app 存活"
     * 对本项目的主要场景（被杀后复活）是成立的；不额外用 `cmd activity` 查 service，省掉又一次
     * 进程创建与解析开销。仅当 pidof 两段都落空且到达兜底周期时，才用 dumpsys 做一次精确确认。
     *
     * 复活路径的 7 条命令放在一个 shell 函数里连续执行，整段脚本由一次 `sh -c` 承载，
     * 不再为每条命令各起一个 ProcessBuilder/sh 进程。
     */
    private fun buildDetachedScript(): String = buildString {
        append("echo \$\$ > $PID_FILE; ")
        append("PKG=$packageName; SVC=$serviceComponent; ")
        append("INTERVAL=$WATCHDOG_INTERVAL_SECONDS; FALLBACK_EVERY=$FALLBACK_DUMPSYS_EVERY; ")
        append("n=0; ")
        // 单次 sh 调用内合并所有 bypass + 拉起命令（原实现每条命令 fork 一个 sh）。
        append("revive() { ")
        append("cmd package unstop --user 0 \$PKG >/dev/null 2>&1; ")
        append("cmd appops set \$PKG RUN_ANY_IN_BACKGROUND allow >/dev/null 2>&1; ")
        append("cmd appops set \$PKG RUN_IN_BACKGROUND allow >/dev/null 2>&1; ")
        append("cmd appops set \$PKG AUTO_REVOKE_PERMISSIONS_IF_UNUSED ignore >/dev/null 2>&1; ")
        append("am set-standby-bucket \$PKG active >/dev/null 2>&1; ")
        append("dumpsys deviceidle whitelist +\$PKG >/dev/null 2>&1; ")
        append("am start-foreground-service --user 0 -n \$SVC >/dev/null 2>&1; ")
        append("}; ")
        append("while true; do ")
        append("n=\$((n+1)); ")
        append("alive=\$(pidof -s \$PKG 2>/dev/null); ")
        append("[ -z \"\$alive\" ] && alive=\$(pidof -s \${PKG:0:15} 2>/dev/null); ")
        append("if [ -z \"\$alive\" ] && [ \$((n % FALLBACK_EVERY)) -eq 0 ]; then ")
        append("if ! dumpsys activity services \$PKG 2>/dev/null | grep -q org.kde.kdeconnect.BackgroundService; then revive; fi; ")
        append("fi; ")
        append("sleep \$INTERVAL; ")
        append("done # $MARKER")
    }

    /** Force an immediate check+relaunch from the shell (used by boot receiver via AIDL). */
    fun requestRelaunch() {
        // bypass + unstop + 拉起服务合并为一次 exec；随后确保独立看门狗仍在运行。
        val result = exec(relaunchCommand())
        Log.i(TAG, "immediate relaunch result=$result")
        ensureRunning()
    }

    /** Refresh the shell-side ROM bypasses (合并为一次 exec)。 */
    fun refreshBypasses() {
        val result = exec(bypassCommands() + "echo ok")
        Log.i(TAG, "refreshed shell-side bypasses: $result")
    }

    /** ROM bypass 命令串（结尾带 "; "，便于拼接）。 */
    private fun bypassCommands(): String = buildString {
        append("cmd appops set $packageName RUN_ANY_IN_BACKGROUND allow >/dev/null 2>&1; ")
        append("cmd appops set $packageName RUN_IN_BACKGROUND allow >/dev/null 2>&1; ")
        append("cmd appops set $packageName AUTO_REVOKE_PERMISSIONS_IF_UNUSED ignore >/dev/null 2>&1; ")
        append("am set-standby-bucket $packageName active >/dev/null 2>&1; ")
        append("dumpsys deviceidle whitelist +$packageName >/dev/null 2>&1; ")
    }

    private fun relaunchCommand(): String =
        "cmd package unstop --user 0 $packageName >/dev/null 2>&1; " +
            bypassCommands() +
            "am start-foreground-service --user 0 -n $serviceComponent >/dev/null 2>&1; echo ok"

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
