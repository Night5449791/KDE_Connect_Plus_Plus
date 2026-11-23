// SPDX-License-Identifier: GPL-2.0-only OR GPL-3.0-only OR LicenseRef-KDE-Accepted-GPL
package org.kde.kdeconnect.plus

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.os.Binder
import android.os.IBinder
import android.os.IInterface
import android.os.Parcel
import android.util.Log
import java.lang.reflect.InvocationHandler
import java.lang.reflect.Method
import java.lang.reflect.Proxy

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

    /** AOSP `IOnPrimaryClipChangedListener.dispatchPrimaryClipChanged()` is transaction 1. */
    private const val TRANSACTION_DISPATCH_PRIMARY_CLIP_CHANGED = 1

    @Volatile private var service: Any? = null
    @Volatile private var getMethod: Method? = null
    @Volatile private var setMethod: Method? = null
    @Volatile private var addListenerMethod: Method? = null
    @Volatile private var removeListenerMethod: Method? = null
    private var attempted = false

    /** Cached arg templates: one signature -> one immutable array, slots patched per call. */
    private class ArgTemplate(
        val args: Array<Any?>,
        val clipSlots: IntArray,
        val listenerSlots: IntArray,
    )

    private val argTemplates = HashMap<Method, ArgTemplate>()

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
            val methods = svc.javaClass.methods
            getMethod = methods.firstOrNull { it.name == "getPrimaryClip" }
            setMethod = methods.firstOrNull { it.name == "setPrimaryClip" }
            fun listenerMethod(name: String): Method? =
                methods.firstOrNull {
                    it.name == name && it.parameterTypes.any { t ->
                        IInterface::class.java.isAssignableFrom(t)
                    }
                } ?: methods.firstOrNull { it.name == name }
            addListenerMethod = listenerMethod("addPrimaryClipChangedListener")
            removeListenerMethod = listenerMethod("removePrimaryClipChangedListener")
            Log.i(
                TAG,
                "IClipboard ready get(${getMethod?.parameterTypes?.joinToString { it.name }}) " +
                    "set(${setMethod?.parameterTypes?.joinToString { it.name }}) " +
                    "add(${addListenerMethod?.parameterTypes?.joinToString { it.name }}) " +
                    "remove(${removeListenerMethod?.parameterTypes?.joinToString { it.name }})",
            )
            true
        } catch (t: Throwable) {
            Log.w(TAG, "IClipboard reflection unavailable, using public ClipboardManager", t)
            service = null
            false
        }
    }

    /**
     * Creates an `android.content.IOnPrimaryClipChangedListener` proxy backed by a real Binder.
     * AIDL marshals the listener with `listener.asBinder()`, so a plain dynamic proxy is not
     * enough: the proxy supplies the interface type and the Binder receives the one-way
     * transaction from system_server. All reflection is contained in runCatching; null means
     * "event mode unavailable" and the caller must fall back to polling.
     */
    fun createPrimaryClipChangedListener(onChanged: () -> Unit): IInterface? = runCatching {
        val listenerClass = Class.forName("android.content.IOnPrimaryClipChangedListener")
        require(listenerClass.isInterface) { "IOnPrimaryClipChangedListener is not an interface" }
        val binder = object : Binder() {
            override fun onTransact(code: Int, data: Parcel, reply: Parcel?, flags: Int): Boolean {
                // AOSP assigns code 1 to dispatchPrimaryClipChanged. Vendors may prepend methods,
                // so accept the whole low AIDL range; standard binder pseudo-codes are far above.
                if (code in TRANSACTION_DISPATCH_PRIMARY_CLIP_CHANGED..100) {
                    onChanged()
                    return true
                }
                return super.onTransact(code, data, reply, flags)
            }
        }
        val handler = InvocationHandler { proxy, method, args ->
            when (method.name) {
                "asBinder" -> binder
                // AOSP names it dispatchPrimaryClipChanged; accept the older/alias name too.
                "dispatchPrimaryClipChanged", "onPrimaryClipChanged" -> {
                    onChanged()
                    null
                }
                "hashCode" -> System.identityHashCode(proxy)
                "equals" -> proxy === args?.firstOrNull()
                "toString" -> "KdeConnectPlus.PrimaryClipChangedListener"
                else -> null
            }
        }
        Proxy.newProxyInstance(listenerClass.classLoader, arrayOf(listenerClass), handler) as IInterface
    }.onFailure { Log.w(TAG, "cannot create primary-clip listener proxy", it) }.getOrNull()

    /** Registers the listener on `IClipboard`; false means the caller should keep polling. */
    fun registerPrimaryClipChangedListener(listener: IInterface): Boolean {
        if (!ensure()) return false
        val method = addListenerMethod ?: return false
        return runCatching {
            method.invoke(service, *buildArgs(method, null, listener))
            Log.i(TAG, "primary-clip listener registered via ${method.name}")
            true
        }.onFailure { Log.w(TAG, "addPrimaryClipChangedListener failed", it) }
            .getOrDefault(false)
    }

    fun unregisterPrimaryClipChangedListener(listener: IInterface) {
        if (!ensure()) return
        val method = removeListenerMethod ?: return
        runCatching {
            method.invoke(service, *buildArgs(method, null, listener))
            Log.i(TAG, "primary-clip listener unregistered")
        }.onFailure { Log.w(TAG, "removePrimaryClipChangedListener failed", it) }
    }

    /** Returns the current primary clip as text, or null when empty / not readable. */
    fun read(context: Context?): String? {
        if (!ensure()) return readPublic(context)
        val method = getMethod ?: return readPublic(context)
        return try {
            val clip = method.invoke(service, *buildArgs(method, null, null)) as? ClipData
            // A null clip is a legitimate result (empty clipboard, locked device, no focus).
            // Do NOT fall back to the public ClipboardManager here: it would use the app's op
            // package name and throw "calling package does not match caller's uid". Fallback is
            // only for reflection/invocation failures.
            if (clip == null) return null
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
            method.invoke(service, *buildArgs(method, ClipData.newPlainText(null, text), null))
            true
        } catch (t: Throwable) {
            Log.w(TAG, "IClipboard setPrimaryClip failed, using public ClipboardManager", t)
            writePublic(context, text)
        }
    }

    /**
     * Build one template per signature (first call inspects the parameter types), then reuse it:
     * the read path allocates nothing at all, write/listener calls only copy the small template
     * and replace the ClipData / listener slots. Rules mirror [planArgs]:
     *   - first IInterface-typed parameter -> the listener binder; other interfaces -> null
     *   - ClipData -> the clip to write (read has none)
     *   - first String -> calling package "com.android.shell"; later Strings -> null attribution tags
     *   - first int -> user id; later ints -> device id / defaults (all 0)
     *   - boolean and any other primitive -> false / 0 boxed value
     *   - any other reference type -> null
     */
    @Synchronized
    private fun templateFor(method: Method): ArgTemplate = argTemplates.getOrPut(method) {
        val types = method.parameterTypes
        val args = arrayOfNulls<Any?>(types.size)
        val clipSlots = ArrayList<Int>()
        val listenerSlots = ArrayList<Int>()
        var strings = 0
        var ints = 0
        types.forEachIndexed { index, type ->
            when {
                type == ClipData::class.java -> {
                    clipSlots += index
                    args[index] = null
                }
                type.isInterface && IInterface::class.java.isAssignableFrom(type) -> {
                    listenerSlots += index
                    args[index] = null
                }
                type == String::class.java -> args[index] = if (strings++ == 0) SHELL_PACKAGE else null
                type == Integer.TYPE -> args[index] = if (ints++ == 0) USER_ID else DEVICE_ID_DEFAULT
                else -> args[index] = primitiveDefault(type)
            }
        }
        ArgTemplate(args, clipSlots.toIntArray(), listenerSlots.toIntArray())
    }

    private fun buildArgs(method: Method, clip: ClipData?, listener: IInterface?): Array<Any?> {
        val template = templateFor(method)
        if (template.clipSlots.isEmpty() && template.listenerSlots.isEmpty()) {
            return template.args
        }
        val args = template.args.copyOf()
        template.clipSlots.forEach { args[it] = clip }
        template.listenerSlots.forEach { args[it] = listener }
        return args
    }

    /** Non-null boxed default for any primitive parameter. Exposed for tests. */
    internal fun primitiveDefault(type: Class<*>): Any? = when (type) {
        java.lang.Boolean.TYPE -> false
        java.lang.Long.TYPE -> 0L
        java.lang.Float.TYPE -> 0f
        java.lang.Double.TYPE -> 0.0
        java.lang.Short.TYPE -> 0.toShort()
        java.lang.Byte.TYPE -> 0.toByte()
        java.lang.Character.TYPE -> '\u0000'
        else -> null // reference types (attributionTag and unknown objects) stay null
    }

    /** Roles for a signature string (C=ClipData, S=String, I=int, B=boolean, ?=other). Exposed for tests. */
    internal fun planArgs(signature: String): List<String> {
        var strings = 0
        var ints = 0
        return signature.map { c ->
            when (c) {
                'C' -> "clip"
                'S' -> if (strings++ == 0) "callingPackage" else "attributionTag"
                'I' -> if (ints++ == 0) "userId" else "deviceId"
                'B' -> "boolean"
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
