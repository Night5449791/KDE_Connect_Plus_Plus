/*
 * SPDX-FileCopyrightText: 2014 Albert Vaca Cintora <albertvaka@gmail.com>
 *
 * SPDX-License-Identifier: GPL-2.0-only OR GPL-3.0-only OR LicenseRef-KDE-Accepted-GPL
 */
package org.kde.kdeconnect.plugins.clipboard

import android.Manifest
import android.app.Activity
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import android.util.Log
import android.widget.Toast
import androidx.annotation.VisibleForTesting
import androidx.core.content.ContextCompat
import androidx.core.content.getSystemService
import org.kde.kdeconnect.NetworkPacket
import org.kde.kdeconnect.plugins.Plugin
import org.kde.kdeconnect.plugins.PluginFactory.LoadablePlugin
import org.kde.kdeconnect.plugins.clipboard.ClipboardListener.ClipboardObserver
import org.kde.kdeconnect.plus.ShizukuClipboardBridge
import org.kde.kdeconnect.ui.PluginSettingsFragment
import org.kde.kdeconnect_tp.R

@LoadablePlugin
class ClipboardPlugin : Plugin() {
    override val displayName: String
        get() = context.resources.getString(R.string.pref_plugin_clipboard)

    override val description: String
        get() = context.resources.getString(R.string.pref_plugin_clipboard_desc)

    override val hasSettings: Boolean = true

    override val settingsScope = SettingsScope.PER_DEVICE

    override fun getSettingsFragment(activity: Activity): PluginSettingsFragment {
        // kdeconnect+ : use the subclass that appends the Shizuku section.
        return ClipboardSettingsFragment.newInstance(
            pluginKey, device.deviceId, R.xml.clipboardplugin_preferences
        )
    }

    override fun onPacketReceived(np: NetworkPacket): Boolean {
        val content = np.getString("content")
        when (np.type) {
            (PACKET_TYPE_CLIPBOARD) -> {
                // kdeconnect+ : remember the remote value's hash so the local observer does not
                // echo it straight back when the shell reports the write as a clipboard change.
                rememberRemoteClip(content)
                ClipboardListener.instance(context).setText(content)
                return true
            }
            (PACKET_TYPE_CLIPBOARD_CONNECT) -> {
                val packetTime = np.getLong("timestamp")
                // If the packetTime is 0, it means the timestamp is unknown (so do nothing).
                if (packetTime == 0L || packetTime < ClipboardListener.instance(context).updateTimestamp) {
                    return false
                }
                if ("content" in np) { // change clipboard if content is in NetworkPacket
                    rememberRemoteClip(content)
                    ClipboardListener.instance(context).setText(content)
                }
                return true
            }
            else -> throw UnsupportedOperationException("Unknown packet type: ${np.type}")
        }
    }

    /**
     * kdeconnect+ : values written locally because a remote device sent them. Only the text hash
     * is kept (never the text itself), capped at [MAX_ECHO_ENTRIES] and expiring after
     * [ECHO_TTL_MS], so a stale marker can never suppress a later legitimate copy forever.
     * Plugin instances are per device, which also excludes the sender in a multi-device setup:
     * only this device's instance carries its own source id in the queue.
     */
    private data class EchoEntry(val hash: Int, val createdAtMs: Long, val sourceDeviceId: String?)

    private val echoLock = Any()
    private val echoQueue = ArrayList<EchoEntry>()

    private fun myDeviceId(): String? = runCatching { device.deviceId }.getOrNull()

    /** Records a remote write before applying it locally. */
    private fun rememberRemoteClip(content: String) {
        val hash = content.hashCode()
        val now = System.currentTimeMillis()
        synchronized(echoLock) {
            echoQueue.removeAll { now - it.createdAtMs > ECHO_TTL_MS }
            echoQueue.removeAll { it.hash == hash }
            echoQueue.add(EchoEntry(hash, now, myDeviceId()))
            while (echoQueue.size > MAX_ECHO_ENTRIES) {
                echoQueue.removeAt(0)
            }
        }
    }

    /**
     * True when this content is the echo of a remote write: consumes the marker so a second,
     * genuine copy of the same text is reported normally.
     */
    private fun isEchoAndConsume(content: String): Boolean {
        val hash = content.hashCode()
        val now = System.currentTimeMillis()
        return synchronized(echoLock) {
            echoQueue.removeAll { now - it.createdAtMs > ECHO_TTL_MS }
            val index = echoQueue.indexOfFirst { it.hash == hash }
            if (index < 0) {
                false
            } else {
                val entry = echoQueue.removeAt(index)
                // Defensive: never exclude a device other than the original sender.
                entry.sourceDeviceId == null || entry.sourceDeviceId == myDeviceId()
            }
        }
    }

    private val observer: ClipboardObserver = object : ClipboardObserver {
        override fun clipboardChanged(content: String, contentType: ClipboardListener.ClipboardContentType) {
            if (contentType == ClipboardListener.ClipboardContentType.Password &&
                preferences.getBoolean(
                    context.getString(R.string.clipboard_preference_key_skip_sensitive),
                    false,
                )
            ) {
                return
            }
            // kdeconnect+ : do not bounce a remote write back to the sender. A hash match
            // consumes the marker; other device instances have no marker and still relay.
            if (isEchoAndConsume(content)) {
                return
            }
            Log.d(TAG, "auto-sync clipboard change (type=$contentType, chars=${content.length})")
            propagateClipboard(content)
        }
    }


    @VisibleForTesting
    fun propagateClipboard(content: String) {
        Log.d(TAG, "sending clipboard packet (chars=${content.length})")
        val np = NetworkPacket(PACKET_TYPE_CLIPBOARD)
        np["content"] = content
        device.sendPacket(np)
    }

    private fun sendConnectPacket() {
        val content = ClipboardListener.instance(context).currentContent ?: return // Send clipboard only if it had been initialized
        if (ClipboardListener.instance(context).currentContentType == ClipboardListener.ClipboardContentType.Password &&
            preferences.getBoolean(
                context.getString(R.string.clipboard_preference_key_skip_sensitive),
                false,
            )
        ) {
            return
        }
        val np = NetworkPacket(PACKET_TYPE_CLIPBOARD_CONNECT)
        val timestamp = ClipboardListener.instance(context).updateTimestamp
        np["timestamp"] = timestamp
        np["content"] = content
        device.sendPacket(np)
    }


    override fun onCreate() {
        ClipboardListener.instance(context).registerObserver(observer)
        sendConnectPacket()
    }

    override fun onDestroy() {
        ClipboardListener.instance(context).removeObserver(observer)
    }

    override val supportedPacketTypes: Array<String> = arrayOf(PACKET_TYPE_CLIPBOARD, PACKET_TYPE_CLIPBOARD_CONNECT)

    override val outgoingPacketTypes: Array<String> = arrayOf(PACKET_TYPE_CLIPBOARD, PACKET_TYPE_CLIPBOARD_CONNECT)

    override fun getUiButtons(): List<PluginUiButton> {
        return if (!canSyncAutomatically(context)) {
            listOf(PluginUiButton(context.getString(R.string.send_clipboard), R.drawable.ic_baseline_content_paste_24) { _: Activity? ->
                userInitiatedSendClipboard()
            })
        } else {
            emptyList()
        }
    }

    override fun getUiMenuEntries(): List<PluginUiMenuEntry> {
        return if (canSyncAutomatically(context)) {
            // If automatic sync is supported, add the manual action less prominently, as a menu entry instead of a button.
            listOf(PluginUiMenuEntry(context.getString(R.string.send_clipboard)) { _: Activity? ->
                userInitiatedSendClipboard()
            })
        } else {
            emptyList()
        }
    }

    private fun userInitiatedSendClipboard() {
        if (isDeviceInitialized) {
            val clipboardManager = context.getSystemService<ClipboardManager>()!!
            val item: ClipData.Item
            if (clipboardManager.hasPrimaryClip()) {
                item = clipboardManager.primaryClip!!.getItemAt(0)
                val content = item.coerceToText(this.context).toString()
                // Don't check if the content is sensitive, just send it
                this.propagateClipboard(content)
                Toast.makeText(this.context, R.string.pref_plugin_clipboard_sent, Toast.LENGTH_SHORT).show()
            }
        }
    }

    companion object {
        private const val TAG = "KdeConnectPlus/Clipboard"

        /** Echo markers are short-lived and bounded; only a 32-bit hash is retained. */
        private const val MAX_ECHO_ENTRIES = 8
        private const val ECHO_TTL_MS = 30_000L

        /**
         * Packet containing just clipboard contents, sent when a device updates its clipboard.
         *
         *
         * The body should look like so:
         * {
         * "content": "password"
         * }
         */
        private const val PACKET_TYPE_CLIPBOARD = "kdeconnect.clipboard"

        /**
         * Packet containing clipboard contents and a timestamp that the contents were last updated, sent
         * on first connection
         *
         *
         * The timestamp is milliseconds since epoch. It can be 0, which indicates that the clipboard
         * update time is currently unknown.
         *
         *
         * The body should look like so:
         * {
         * "timestamp": 542904563213,
         * "content": "password"
         * }
         */
        private const val PACKET_TYPE_CLIPBOARD_CONNECT = "kdeconnect.clipboard.connect"

        fun canSyncAutomatically(context: Context): Boolean {
            // kdeconnect+ : Shizuku gives us the shell identity, so background clipboard reads
            // work without the READ_LOGS logcat hack.
            if (ShizukuClipboardBridge.isReady()) {
                return true
            }
            if (ShizukuClipboardBridge.isShizukuAvailable() && ShizukuClipboardBridge.hasPermission()) {
                // Permission granted but the user service is still binding; treat as supported.
                return true
            }
            if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) {
                return true
            }
            // Check if hack to read clipboard changes from logs is enabled
            return ContextCompat.checkSelfPermission(context, Manifest.permission.READ_LOGS) == PackageManager.PERMISSION_GRANTED
        }

    }
}
