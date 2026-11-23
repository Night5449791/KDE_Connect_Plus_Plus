// SPDX-License-Identifier: GPL-2.0-only OR GPL-3.0-only OR LicenseRef-KDE-Accepted-GPL
package org.kde.kdeconnect.plus

import android.content.Context
import java.io.BufferedReader
import java.io.InputStreamReader
import java.io.PrintStream

/**
 * Entry point of the shell-identity helper process. `app_process` starts it as uid 0 (from a
 * root/Sui Shizuku), so the first thing it does is drop to the shell uid and re-exec itself,
 * which guarantees the process attaches to the primary clip with the shell identity.
 *
 * Protocol (one line per message, UTF-8):
 *   child -> parent : "READY" once the clipboard context is usable
 *   parent -> child : "READ"                  -> "OK <escaped>" | "NONE"
 *                     "WRITE <escaped>"       -> "OK" | "ERR"
 *                     "POLL"                  -> "OK <escaped>" | "NONE"
 *                     "EXIT"                  -> process exits
 */
object ShellClipboardMain {

    const val READY = "READY"
    const val OK = "OK"
    const val ERR = "ERR"
    const val NONE = "NONE"
    const val CMD_READ = "READ"
    const val CMD_WRITE = "WRITE"
    const val CMD_POLL = "POLL"
    const val CMD_EXIT = "EXIT"

    private const val SHELL_UID = 2000
    private const val RE_EXEC_MARKER = "KDE_CONNECT_PLUS_CLIPBOARD_CHILD"

    @JvmStatic
    fun main(args: Array<String>) {
        if (System.getenv(RE_EXEC_MARKER) == null) {
            if (android.os.Process.myUid() == SHELL_UID || !reExecAsShell()) {
                // Already shell, or the re-exec failed: continue with whatever identity we have.
                if (!runLoop()) System.exit(1)
            }
            return
        }
        if (!runLoop()) System.exit(1)
    }

    /**
     * Replaces this process image with one that runs under uid 2000. Uses `app_process` through
     * `sh -c "exec ..."` because a root process can only change its uid at exec time.
     */
    private fun reExecAsShell(): Boolean {
        val javaHome = System.getProperty("java.home") ?: return false
        val classpath = System.getProperty("java.class.path") ?: return false
        val runtime = javaHome.removeSuffix("/jre") + "/lib/" + archRuntime()
        return runCatching {
            val cmd = buildString {
                append("exec ")
                append("/system/bin/app_process")
                append(' ')
                append(shellQuote(runtimeDir(javaHome)))
                append(" /system/bin --nice-name=kdeconnect-plus-clipboard ")
                append(shellQuote(classpath))
                append(' ')
                append(ShellClipboardMain::class.java.name)
            }
            val pb = ProcessBuilder("/system/bin/sh", "-c", cmd)
            pb.environment()[RE_EXEC_MARKER] = "1"
            pb.environment()["CLASSPATH"] = classpath
            pb.inheritIO()
            val p = pb.start()
            // sh replaces itself with app_process, so waiting here blocks until the child exits.
            p.waitFor()
            true
        }.also { System.err.println("re-exec as shell: $it ($runtime)") }
            .getOrDefault(false)
    }

    private fun runtimeDir(javaHome: String): String = javaHome.removeSuffix("/jre") + "/lib"

    private fun archRuntime(): String =
        if (System.getProperty("os.arch")?.contains("64") == true) "aarch64/server/libjvm.so"
        else "arm/server/libjvm.so"

    private fun shellQuote(value: String): String = "'" + value.replace("'", "'\\''") + "'"

    private fun runLoop(): Boolean {
        // An app_process helper may have no Application. The binder path passes the calling
        // package explicitly and works without one; the context is only used for content:// URIs
        // and as the public-API fallback.
        val context: Context? = currentApplication()
        val out = PrintStream(System.out, true, "UTF-8")
        val input = BufferedReader(InputStreamReader(System.`in`, Charsets.UTF_8))
        out.println(READY)
        var lastSeen = readClipboard(context)
        while (true) {
            val line = input.readLine() ?: break
            val space = line.indexOf(' ')
            val cmd = if (space < 0) line else line.substring(0, space)
            val arg = if (space < 0) null else line.substring(space + 1)
            when (cmd) {
                CMD_READ -> {
                    val text = readClipboard(context)
                    out.println(if (text == null) NONE else "$OK ${encodePayload(text)}")
                }
                CMD_WRITE -> {
                    val text = decodePayload(arg ?: "")
                    val written = ShellClipboardAccess.write(context, text)
                    if (written) lastSeen = text
                    out.println(if (written) OK else ERR)
                }
                CMD_POLL -> {
                    val text = readClipboard(context)
                    if (text != null && text != lastSeen) {
                        lastSeen = text
                        out.println("$OK ${encodePayload(text)}")
                    } else {
                        out.println(NONE)
                    }
                }
                CMD_EXIT -> break
                else -> out.println(ERR)
            }
        }
        return true
    }

    private fun readClipboard(context: Context?): String? =
        runCatching { ShellClipboardAccess.read(context) }.getOrNull()

    private fun currentApplication(): Context? = runCatching {
        val at = Class.forName("android.app.ActivityThread")
        at.getMethod("currentApplication").invoke(null) as? Context
    }.getOrNull()

    fun encodePayload(text: String): String =
        text.replace("\\", "\\\\").replace("\n", "\\n")

    fun decodePayload(payload: String): String {
        if (payload.isEmpty()) return ""
        val sb = StringBuilder(payload.length)
        var i = 0
        while (i < payload.length) {
            val c = payload[i]
            if (c == '\\' && i + 1 < payload.length) {
                when (payload[i + 1]) {
                    'n' -> sb.append('\n')
                    '\\' -> sb.append('\\')
                    else -> sb.append(payload[i + 1])
                }
                i += 2
            } else {
                sb.append(c)
                i++
            }
        }
        return sb.toString()
    }
}
