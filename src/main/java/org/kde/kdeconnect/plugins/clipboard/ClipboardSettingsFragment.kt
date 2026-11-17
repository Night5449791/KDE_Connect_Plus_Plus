/*
 * kdeconnect+ : adds a Shizuku section to the official clipboard settings screen.
 *
 * SPDX-License-Identifier: GPL-2.0-only OR GPL-3.0-only OR LicenseRef-KDE-Accepted-GPL
 */
package org.kde.kdeconnect.plugins.clipboard

import android.content.Intent
import android.os.Bundle
import androidx.preference.Preference
import androidx.preference.PreferenceCategory
import org.kde.kdeconnect.helpers.ThreadHelper
import org.kde.kdeconnect.plus.ShizukuClipboardBridge
import org.kde.kdeconnect.plus.ShizukuPermissionActivity
import org.kde.kdeconnect.ui.PluginSettingsFragment
import org.kde.kdeconnect_tp.R

class ClipboardSettingsFragment : PluginSettingsFragment() {
    private var statusPreference: Preference? = null

    private val refreshRunnable = Runnable { refresh() }

    override fun onCreatePreferences(savedInstanceState: Bundle?, rootKey: String?) {
        super.onCreatePreferences(savedInstanceState, rootKey)

        val context = preferenceManager.context
        val category = PreferenceCategory(context).apply {
            title = getString(R.string.plus_shizuku_category)
            isIconSpaceReserved = false
        }
        preferenceScreen.addPreference(category)

        statusPreference = Preference(context).apply {
            isPersistent = false
            isIconSpaceReserved = false
            title = getString(R.string.plus_shizuku_status)
            setOnPreferenceClickListener {
                startActivity(Intent(context, ShizukuPermissionActivity::class.java))
                true
            }
        }.also(category::addPreference)

        Preference(context).apply {
            isPersistent = false
            isIconSpaceReserved = false
            title = getString(R.string.plus_shizuku_auto_sync)
            summary = getString(R.string.plus_shizuku_auto_sync_summary)
            setOnPreferenceClickListener {
                ShizukuClipboardBridge.requestPermission()
                true
            }
        }.also(category::addPreference)
    }

    override fun onStart() {
        super.onStart()
        refresh()
    }

    override fun onResume() {
        super.onResume()
        // Shizuku permission is granted in another activity; re-check when we come back.
        ThreadHelper.runOnNewThread(refreshRunnable)
    }

    private fun refresh() {
        val preference = statusPreference ?: return
        val text = when {
            !ShizukuClipboardBridge.isShizukuAvailable() -> R.string.plus_shizuku_status_unavailable
            !ShizukuClipboardBridge.hasPermission() -> R.string.plus_shizuku_status_no_permission
            ShizukuClipboardBridge.isReady() -> R.string.plus_shizuku_status_ready
            else -> R.string.plus_shizuku_status_binding
        }
        preference.post { preference.summary = getString(text) }
    }

    companion object {
        /** Mirrors [PluginSettingsFragment.newInstance] but keeps this subclass. */
        fun newInstance(pluginKey: String, deviceId: String, vararg settingsLayout: Int): ClipboardSettingsFragment {
            val fragment = ClipboardSettingsFragment()
            fragment.setArguments(pluginKey, deviceId, *settingsLayout)
            return fragment
        }
    }
}
