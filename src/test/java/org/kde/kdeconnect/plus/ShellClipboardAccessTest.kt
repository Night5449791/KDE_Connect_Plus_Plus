package org.kde.kdeconnect.plus

import org.junit.Assert.assertEquals
import org.junit.Test

class ShellClipboardAccessTest {

    @Test
    fun planArgsMatchesAndroid12And13Signatures() {
        // getPrimaryClip(String pkg, int userId)
        assertEquals(
            listOf("callingPackage", "userId"),
            ShellClipboardAccess.planArgs("SI"),
        )
        // setPrimaryClip(ClipData clip, String callingPackage, int userId)
        assertEquals(
            listOf("clip", "callingPackage", "userId"),
            ShellClipboardAccess.planArgs("CSI"),
        )
    }

    @Test
    fun planArgsMatchesAndroid14And15Signatures() {
        // getPrimaryClip(String pkg, String attributionTag, int userId, int deviceId)
        assertEquals(
            listOf("callingPackage", "attributionTag", "userId", "deviceId"),
            ShellClipboardAccess.planArgs("SSII"),
        )
        // setPrimaryClip(ClipData, String, String attributionTag, int, int deviceId)
        assertEquals(
            listOf("clip", "callingPackage", "attributionTag", "userId", "deviceId"),
            ShellClipboardAccess.planArgs("CSSII"),
        )
    }

    @Test
    fun planArgsHandlesVendorHybridSignature() {
        // Some ROMs add attributionTag but not deviceId.
        assertEquals(
            listOf("callingPackage", "attributionTag", "userId"),
            ShellClipboardAccess.planArgs("SSI"),
        )
    }

    @Test
    fun planArgsHandlesVivoSignatureWithBoolean() {
        // vivo appends a boolean parameter; it must be filled with false, never null.
        assertEquals(
            listOf("clip", "callingPackage", "attributionTag", "userId", "deviceId", "boolean"),
            ShellClipboardAccess.planArgs("CSSIIB"),
        )
    }
}
