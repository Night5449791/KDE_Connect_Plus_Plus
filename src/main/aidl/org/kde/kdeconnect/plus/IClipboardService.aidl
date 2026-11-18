// SPDX-License-Identifier: GPL-2.0-only OR GPL-3.0-only OR LicenseRef-KDE-Accepted-GPL
package org.kde.kdeconnect.plus;

import org.kde.kdeconnect.plus.IClipboardWatcher;

/** Minimal clipboard bridge executed as the shell user (uid 2000) via Shizuku. */
interface IClipboardService {
    /** Returns the current primary clip as text, or null when the clipboard is empty / not text. */
    String getPrimaryClipText();

    /** Writes plain text into the primary clip. Returns true on success. */
    boolean setPrimaryClipText(String text);

    /** Registers a callback that is invoked whenever the shell-visible clipboard changes. */
    void registerWatcher(IClipboardWatcher watcher);

    void unregisterWatcher(IClipboardWatcher watcher);

    /**
     * Watchdog: immediately check whether the app's BackgroundService is alive and relaunch it
     * via shell (uid 2000) if not. Safe to call from the app process (e.g. boot receiver).
     */
    void requestRelaunch();

    /** Watchdog: refresh the shell bypasses (clear stopped state, appops, standby whitelist). */
    void refreshBypasses();
}
