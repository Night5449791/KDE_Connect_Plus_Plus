/*
 * SPDX-FileCopyrightText: 2014 Albert Vaca Cintora <albertvaka@gmail.com>
 *
 * SPDX-License-Identifier: GPL-2.0-only OR GPL-3.0-only OR LicenseRef-KDE-Accepted-GPL
 */
package org.kde.kdeconnect

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.net.ConnectivityManager
import android.net.wifi.WifiManager
import android.util.Log
import org.kde.kdeconnect.plus.ShizukuClipboardBridge

class KdeConnectBroadcastReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        // Log.e("KdeConnect", "Broadcast event: "+intent.getAction());

        val action = intent.action

        when (action) {
            Intent.ACTION_MY_PACKAGE_REPLACED -> {
                Log.i("KdeConnect", "MyUpdateReceiver")
                relaunch(context)
            }

            Intent.ACTION_BOOT_COMPLETED,
            Intent.ACTION_TIME_SET,
            Intent.ACTION_TIMEZONE_CHANGED,
            Intent.ACTION_LOCALE_CHANGED -> {
                Log.i("KdeConnect", "KdeConnectBroadcastReceiver: $action")
                relaunch(context)
            }

            WifiManager.SUPPLICANT_CONNECTION_CHANGE_ACTION, WifiManager.WIFI_STATE_CHANGED_ACTION, ConnectivityManager.CONNECTIVITY_ACTION -> {
                Log.i("KdeConnect", "Connection state changed, trying to connect")
                BackgroundService.ForceRefreshConnections(context)
            }

            Intent.ACTION_SCREEN_ON -> {
                BackgroundService.ForceRefreshConnections(context)
            }

            else -> Log.i("BroadcastReceiver", "Ignoring broadcast event: ${intent.action}")
        }
    }

    /**
     * kdeconnect+ : prefer the shell-side watchdog (uid 2000) which can relaunch the foreground
     * service even when the app is in the background / stopped state. The watchdog also binds
     * asynchronously on a cold boot, so it will take over shortly. Keep the plain Start as a
     * fallback for when Shizuku is unavailable.
     */
    private fun relaunch(context: Context) {
        if (ShizukuClipboardBridge.isShizukuAvailable() && ShizukuClipboardBridge.hasPermission()) {
            runCatching { ShizukuClipboardBridge.requestWatchdogRelaunch() }
                .onFailure { Log.w("KdeConnect", "watchdog relaunch failed", it) }
        }
        runCatching { BackgroundService.Start(context) }
            .onFailure { Log.w("KdeConnect", "fallback Start failed", it) }
    }
}
