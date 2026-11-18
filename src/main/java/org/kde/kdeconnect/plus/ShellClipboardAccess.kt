// SPDX-License-Identifier: GPL-2.0-only OR GPL-3.0-only OR LicenseRef-KDE-Accepted-GPL
package org.kde.kdeconnect.plus

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.os.IBinder
import android.util.Log
import java.lang.reflect.Method

/**
 * Clipboard access through the raw [IClipboard] binder, passing "com.android.shell" explicitly
 * as the calling package.
 *
 * ClipboardManager always passes `mContext.getOpPackageName()` to the binder. That is the app
 * package (org.kde.kdeconnect_tp) even when the Context came from
 * `createPackageContext("com.android.shell")`, which only changes `getPackageName()`.
 * ClipboardService validates that the calling package belongs to `Binder.getCallingUid()`
 * (2000 here), so the app package is rejected. With uid 2000, "com.android.shell" is an AOSP
 * special case that passes `mAppOps.checkPackage` and `addActiveOwnerLocked`, and is exempt from
 * the foreground-focus requirement.
 *
 * The IClipboard method signatures changed in Android 14 (API 34): `attributionTag` and
 * `deviceId` were added. Some vendor ROMs ship hybrid signatures, so arguments are chosen by
 * parameter type instead of by SDK level. If any reflection step is unavailable (hidden API
 * restrictions), we fall back to the public ClipboardManager rather than break worse than before.
 */
internal object ShellClipboardAccess {

    private const val TAG = "KdeConnectPlus/ClipAccess"
    private const val SHELL_PACKAGE = "com.android.shell"
    private const val USER_ID = 0
    private const val DEVICE_ID_DEFAULT = 0

    @Volatile private var service: Any? = null
    @Volatile private var getMethod: Method? = null
    @Volatile private var setMethod: Method? = null
    private var attempted = false

    /** Resolve the clipboard binder once; safe to call from any thread. */
    @Synchronized
    private fun ensure(): Boolean {
        if (attempted) return service != null
        attempted = true
        return try {
            val binder = Class.forName("android.os.ServiceManager")
                .getMethod("getService", String::class.java)
                .invoke(null, "clipboard") as? IBinder
                ?: throw IllegalStateException("clipboard binder is null")
            val stub = Class.forName("android.content.IClipboard\$Stub")
            val svc = stub.getMethod("asInterface", IBinder::class.java)
                .invoke(null, binder)
                ?: throw IllegalStateException("asInterface returned null")
            service = svc
            getMethod = svc.javaClass.methods.firstOrNull { it.name == "getPrimaryClip" }
            setMethod = svc.javaClass.methods.firstOrNull { it.name == "setPrimaryClip" }
            Log.i(
                TAG,
                "IClipboard ready (get=${getMethod?.parameterTypes?.size} params, " +
                    "set=${setMethod?.parameterTypes?.size} params)",
            )
            true
        } catch (t: Throwable) {
            Log.w(TAG, "IClipboard reflection unavailable, using public ClipboardManager", t)
            service = null
            false
        }
    }

    /** Returns the current primary clip as text, or null when empty / not readable. */
    fun read(context: Context?): String? {
        if (!ensure()) return readPublic(context)
        val method = getMethod ?: return readPublic(context)
        return try {
            val clip = method.invoke(service, *buildArgs(method, null)) as? ClipData
                ?: return readPublic(context)
            if (clip.itemCount <= 0) return null
            val item = clip.getItemAt(0)
            // Plain text needs no Context; only content:// URIs need coerceToText.
            item.text?.toString() ?: context?.let { item.coerceToText(it).toString() }
        } catch (t: Throwable) {
            Log.w(TAG, "IClipboard getPrimaryClip failed, using public ClipboardManager", t)
            readPublic(context)
        }
    }

    /** Writes plain text, returning true on success. */
    fun write(context: Context?, text: String): Boolean {
        if (!ensure()) return writePublic(context, text)
        val method = setMethod ?: return writePublic(context, text)
        return try {
            method.invoke(service, *buildArgs(method, ClipData.newPlainText(null, text)))
            true
        } catch (t: Throwable) {
            Log.w(TAG, "IClipboard setPrimaryClip failed, using public ClipboardManager", t)
            writePublic(context, text)
        }
    }

    /** Map parameter types to their roles: C=ClipData, S=String, I=int. */
    private fun signatureOf(method: Method): String =
        method.parameterTypes.joinToString("") { type ->
            when (type) {
                ClipData::class.java -> "C"
                String::class.java -> "S"
                Integer.TYPE -> "I"
                else -> "?"
            }
        }

    private fun buildArgs(method: Method, clip: ClipData?): Array<Any?> =
        planArgs(signatureOf(method)).map { role ->
            when (role) {
                "clip" -> clip
                "callingPackage" -> SHELL_PACKAGE
                "userId" -> USER_ID
                "deviceId" -> DEVICE_ID_DEFAULT
                else -> null // attributionTag and unknown parameters stay null
            }
        }.toTypedArray()

    /**
     * Decide each argument's role from the signature alone. The first String is the calling
     * package, a second String is the attribution tag; the first int is the user id, the second
     * is the device id. Exposed for unit tests.
     */
    internal fun planArgs(signature: String): List<String> {
        var strings = 0
        var ints = 0
        return signature.map { c ->
            when (c) {
                'C' -> "clip"
                'S' -> if (strings++ == 0) "callingPackage" else "attributionTag"
                'I' -> if (ints++ == 0) "userId" else "deviceId"
                else -> "unknown"
            }
        }
    }

    private fun readPublic(context: Context?): String? {
        val cm = context?.getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager ?: return null
        val clip = cm.primaryClip ?: return null
        if (clip.itemCount <= 0) return null
        val item = clip.getItemAt(0)
        return item.text?.toString() ?: item.coerceToText(context).toString()
    }

    private fun writePublic(context: Context?, text: String): Boolean {
        val cm = context?.getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager ?: return false
        return runCatching {
            cm.setPrimaryClip(ClipData.newPlainText(null, text))
            true
        }.getOrDefault(false)
    }
}
