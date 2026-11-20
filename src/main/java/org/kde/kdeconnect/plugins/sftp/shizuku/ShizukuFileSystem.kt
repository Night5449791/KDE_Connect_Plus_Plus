/*
 * SPDX-FileCopyrightText: 2026 KDE Connect++ contributors
 *
 * SPDX-License-Identifier: GPL-2.0-only OR GPL-3.0-only OR LicenseRef-KDE-Accepted-GPL
 */
package org.kde.kdeconnect.plugins.sftp.shizuku

import org.apache.sshd.common.file.util.BaseFileSystem
import java.nio.file.FileStore
import java.nio.file.Path
import java.nio.file.WatchService
import java.nio.file.attribute.UserPrincipalLookupService
import java.nio.file.spi.FileSystemProvider

/**
 * A stateless [java.nio.file.FileSystem] whose [Path]s are absolute shell paths.
 * Mirrors [org.kde.kdeconnect.plugins.sftp.saf.SafFileSystem]; the shell (via Shizuku) can reach
 * the whole file tree, so "/" is the single root directory.
 */
class ShizukuFileSystem(
    fileSystemProvider: FileSystemProvider
) : BaseFileSystem<ShizukuPath>(fileSystemProvider) {

    override fun create(root: String?, names: List<String>): ShizukuPath =
        if (root == "/" && names.isEmpty()) {
            ShizukuPath.newRootPath(this)
        } else {
            ShizukuPath(this, root, names)
        }

    override fun close() {
        // Stateless and shared for the lifetime of the SFTP session: nothing to release.
    }

    override fun isOpen(): Boolean = true

    override fun isReadOnly(): Boolean = false

    override fun getSeparator(): String = "/"

    override fun getRootDirectories(): Iterable<Path> = setOf(getPath("/"))

    override fun getFileStores(): Iterable<FileStore> = emptySet()

    override fun supportedFileAttributeViews(): Set<String> = setOf("basic", "posix")

    override fun getUserPrincipalLookupService(): UserPrincipalLookupService {
        throw UnsupportedOperationException("Shizuku does not support user principal lookup")
    }

    override fun newWatchService(): WatchService {
        throw UnsupportedOperationException("Shizuku does not support watch service")
    }

    companion object {
        private const val TAG = "ShizukuFileSystem"
    }
}
