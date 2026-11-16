// SPDX-License-Identifier: GPL-2.0-only OR GPL-3.0-only OR LicenseRef-KDE-Accepted-GPL
package org.kde.kdeconnect.plus

/**
 * Shell-command clipboard helper.
 *
 * In root/Sui mode Shizuku gives us uid 0, and a uid-0 process must never attach to the primary
 * clip on Android 12+: doing so (even while impersonating the shell package) wipes the clipboard.
 * So instead of touching the clipboard directly we ask Shizuku to run `cmd clipboard` as the
 * shell user and parse its output. The commands never contain user data, which keeps this free
 * of quoting problems and of any need for a helper process of our own.
 */
internal object ShellClipboardCommands {

    const val GET = "cmd clipboard get-primary-clip"
    const val SET_PREFIX = "cmd clipboard set-primary-clip --user 0 --text "

    /** Quote a value as a single POSIX shell word. */
    fun quote(value: String): String = "'" + value.replace("'", "'\\''") + "'"

    fun set(text: String): String = SET_PREFIX + quote(text)

    /**
     * `cmd clipboard get-primary-clip` prints the clip on one line. Anything that looks like the
     * command's own error output is treated as "no content".
     */
    fun parseGet(output: String?): String? {
        val trimmed = output?.trim() ?: return null
        if (trimmed.isEmpty()) return null
        if (trimmed.startsWith("Exception") || trimmed.startsWith("Error")) return null
        return trimmed
    }
}
