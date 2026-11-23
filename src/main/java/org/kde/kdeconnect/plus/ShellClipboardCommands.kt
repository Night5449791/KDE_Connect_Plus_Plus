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
     * Recognises `cmd clipboard` failure text. Some ROMs print "No shell command implementation."
     * with exit code 0 (and stderr merged into stdout), so the exit code alone is not enough.
     * Only the first line is inspected: multi-line clipboard text must not be mistaken for an
     * error because a later line happens to start with "Error".
     */
    fun isCommandError(output: String?): Boolean {
        val firstLine = output?.trimStart()?.lineSequence()?.firstOrNull() ?: return false
        if (firstLine.isEmpty()) return false
        return firstLine.startsWith("Exception", ignoreCase = true) ||
            firstLine.startsWith("Error", ignoreCase = true) ||
            firstLine.startsWith("No shell command implementation", ignoreCase = true) ||
            firstLine.startsWith("Unknown command", ignoreCase = true) ||
            firstLine.contains("not a valid command", ignoreCase = true)
    }

    /**
     * `cmd clipboard get-primary-clip` prints the clip and trims the trailing newline. Multi-line
     * content comes back as several lines, so only the final newline is dropped, not internal
     * whitespace. Anything that looks like the command's own error output is "no content".
     */
    fun parseGet(output: String?): String? {
        val trimmedEnd = output?.trimEnd() ?: return null
        if (trimmedEnd.isEmpty() || isCommandError(trimmedEnd)) return null
        return trimmedEnd
    }
}
