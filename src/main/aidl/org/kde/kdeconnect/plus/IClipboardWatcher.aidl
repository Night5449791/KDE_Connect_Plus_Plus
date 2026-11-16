// SPDX-License-Identifier: GPL-2.0-only OR GPL-3.0-only OR LicenseRef-KDE-Accepted-GPL
package org.kde.kdeconnect.plus;

/** Callback for shell-side clipboard changes. */
interface IClipboardWatcher {
    void onClipboardChanged(String text);
}
