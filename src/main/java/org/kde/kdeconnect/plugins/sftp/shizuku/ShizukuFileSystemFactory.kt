/*
 * SPDX-FileCopyrightText: 2026 KDE Connect++ contributors
 *
 * SPDX-License-Identifier: GPL-2.0-only OR GPL-3.0-only OR LicenseRef-KDE-Accepted-GPL
 */
package org.kde.kdeconnect.plugins.sftp.shizuku

import org.apache.sshd.common.file.FileSystemFactory
import org.apache.sshd.common.session.SessionContext
import java.nio.file.FileSystem
import java.nio.file.Path

/**
 * SSHD [FileSystemFactory] exposing the whole shell-visible file tree through
 * [ShizukuFileSystemProvider]. No `storageDirs` restriction is needed because the Shizuku
 * shell identity can access every path.
 */
class ShizukuFileSystemFactory : FileSystemFactory {
    private val provider = ShizukuFileSystemProvider()

    override fun createFileSystem(session: SessionContext?): FileSystem = ShizukuFileSystem(provider)

    override fun getUserHomeDir(session: SessionContext?): Path? = null

    companion object {
        private const val TAG = "ShizukuFileSystemFactory"
    }
}
