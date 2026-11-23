// SPDX-License-Identifier: GPL-2.0-only OR GPL-3.0-only OR LicenseRef-KDE-Accepted-GPL
package org.kde.kdeconnect.plus

import android.os.Bundle
import android.widget.Toast
import androidx.appcompat.app.AppCompatActivity
import org.kde.kdeconnect_tp.R

/**
 * kdeconnect+ : transparent activity that asks Shizuku for permission. Shizuku requires the
 * request to come from a foreground component, so the clipboard settings screen launches this.
 */
class ShizukuPermissionActivity : AppCompatActivity() {

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        if (!ShizukuClipboardBridge.isShizukuAvailable()) {
            Toast.makeText(this, R.string.plus_shizuku_unavailable, Toast.LENGTH_LONG).show()
            finish()
            return
        }
        if (ShizukuClipboardBridge.hasPermission()) {
            ShizukuClipboardBridge.ensureBound()
            finish()
            return
        }
        ShizukuClipboardBridge.requestPermission()
        Toast.makeText(this, R.string.plus_shizuku_requesting, Toast.LENGTH_SHORT).show()
        finish()
    }
}
