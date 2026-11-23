package org.kde.kdeconnect.plus

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class ShellClipboardCommandsTest {
    @Test
    fun parseGetPreservesMultilineClipboardText() {
        assertEquals("line one\nline two", ShellClipboardCommands.parseGet("line one\nline two\n"))
    }

    @Test
    fun parseGetRejectsMissingShellCommandResponse() {
        val output = "No shell command implementation.\n"

        assertTrue(ShellClipboardCommands.isCommandError(output))
        assertNull(ShellClipboardCommands.parseGet(output))
    }

    @Test
    fun isCommandErrorDoesNotRejectClipboardText() {
        assertFalse(ShellClipboardCommands.isCommandError("clipboard text"))
    }
}