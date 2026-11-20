/*
 * SPDX-FileCopyrightText: 2026 KDE Connect++ contributors
 *
 * SPDX-License-Identifier: GPL-2.0-only OR GPL-3.0-only OR LicenseRef-KDE-Accepted-GPL
 */
package org.kde.kdeconnect.plugins.sftp.shizuku

import org.apache.sshd.common.file.util.BasePath
import java.net.URI
import java.nio.file.LinkOption

/**
 * A [java.nio.file.Path] backed by an absolute shell path (e.g. `/storage/emulated/0/Download`).
 *
 * Just like [org.kde.kdeconnect.plugins.sftp.saf.SafPath] it delegates the bulk of the
 * [java.nio.file.Path] contract (resolve / relativize / normalize / compareTo / ...) to SSHD's
 * [BasePath]. All real I/O is performed by [ShizukuFileSystemProvider] through
 * [org.kde.kdeconnect.plus.ShizukuFileBridge], so the app itself needs no storage permission.
 */
class ShizukuPath(
    fileSystem: ShizukuFileSystem,
    root: String?,
    names: List<String>
) : BasePath<ShizukuPath, ShizukuFileSystem>(fileSystem, root, names) {

    fun getNames(): List<String> = super.names

    /**
     * The absolute path string understood by the shell, e.g. `/storage/emulated/0/Download`.
     * Derived directly from the root + names that [BasePath] maintains, so it always reflects
     * the result of resolve / normalize / getParent operations.
     */
    val pathString: String
        get() {
            val base = root ?: "/"
            return if (names.isEmpty()) {
                base
            } else {
                base.trimEnd('/') + "/" + names.joinToString("/")
            }
        }

    fun isRoot(): Boolean = root == "/" && names.isEmpty()

    override fun toRealPath(vararg options: LinkOption?): ShizukuPath = normalize()

    override fun toUri(): URI = URI("shizuku", null, pathString, null)

    companion object {
        fun newRootPath(fileSystem: ShizukuFileSystem): ShizukuPath =
            ShizukuPath(fileSystem, "/", emptyList())
    }
}
