/*
 * SPDX-FileCopyrightText: 2026 KDE Connect++ contributors
 *
 * SPDX-License-Identifier: GPL-2.0-only OR GPL-3.0-only OR LicenseRef-KDE-Accepted-GPL
 */
package org.kde.kdeconnect.plugins.sftp.shizuku

import android.os.ParcelFileDescriptor
import android.os.SystemClock
import org.json.JSONObject
import org.kde.kdeconnect.plus.ShizukuFileBridge
import java.io.FileInputStream
import java.io.FileNotFoundException
import java.io.FileOutputStream
import java.io.IOException
import java.io.RandomAccessFile
import java.net.URI
import java.nio.channels.FileChannel
import java.nio.channels.SeekableByteChannel
import java.nio.file.AccessDeniedException
import java.nio.file.AccessMode
import java.nio.file.CopyOption
import java.nio.file.DirectoryStream
import java.nio.file.FileAlreadyExistsException
import java.nio.file.FileStore
import java.nio.file.FileSystem
import java.nio.file.LinkOption
import java.nio.file.NoSuchFileException
import java.nio.file.OpenOption
import java.nio.file.Path
import java.nio.file.StandardCopyOption
import java.nio.file.StandardOpenOption
import java.nio.file.attribute.BasicFileAttributeView
import java.nio.file.attribute.BasicFileAttributes
import java.nio.file.attribute.FileAttribute
import java.nio.file.attribute.FileAttributeView
import java.nio.file.attribute.FileTime
import java.nio.file.attribute.GroupPrincipal
import java.nio.file.attribute.PosixFileAttributeView
import java.nio.file.attribute.PosixFileAttributes
import java.nio.file.attribute.PosixFilePermission
import java.nio.file.attribute.UserPrincipal
import java.nio.file.spi.FileSystemProvider
import java.util.LinkedHashMap

/**
 * NIO [FileSystemProvider] that forwards every operation to [ShizukuFileBridge], which in turn
 * runs it in the shell process. The app process therefore needs no storage permission.
 *
 * Structure and override set parallel
 * [org.kde.kdeconnect.plugins.sftp.saf.SafFileSystemProvider]; only the SSHD/SFTP-used subset is
 * implemented, the rest throws [UnsupportedOperationException] / [NotImplementedError].
 */
class ShizukuFileSystemProvider : FileSystemProvider() {

    private val bridge = ShizukuFileBridge

    /**
     * Short-lived metadata cache. A batch directory listing seeds it with every child's stat, so a
     * client's LSTAT of those children is served locally instead of costing one binder call each.
     * Access-ordered LRU, evicted above 256 entries; all access goes through the @Synchronized
     * helpers below. Only followLinks=true results are cached.
     */
    private class StatCacheEntry(val info: StatInfo, val timestamp: Long)

    private val statCache = object : LinkedHashMap<String, StatCacheEntry>(64, 0.75f, true) {
        override fun removeEldestEntry(
            eldest: MutableMap.MutableEntry<String, StatCacheEntry>?
        ): Boolean = size > 256
    }

    @Synchronized
    private fun cachedStat(key: String): StatInfo? {
        val cached = statCache[key] ?: return null
        if (SystemClock.elapsedRealtime() - cached.timestamp >= STAT_CACHE_TTL_MS) return null
        return cached.info
    }

    @Synchronized
    private fun rememberStat(key: String, info: StatInfo) {
        statCache[key] = StatCacheEntry(info, SystemClock.elapsedRealtime())
    }

    override fun getScheme(): String = "shizuku"

    // SSHD Core does not use these three; follow the SAF behaviour.
    override fun newFileSystem(uri: URI, env: MutableMap<String, *>?): FileSystem {
        throw NotImplementedError("newFileSystem($uri) not implemented")
    }

    override fun getFileSystem(uri: URI): FileSystem {
        throw NotImplementedError("getFileSystem($uri) not implemented")
    }

    override fun getPath(uri: URI): Path {
        throw NotImplementedError("getPath($uri) not implemented")
    }

    // ---------------------------------------------------------------- helpers

    private fun abs(path: Path): String {
        check(path is ShizukuPath) { "Expected a ShizukuPath, got ${path.javaClass}" }
        return path.pathString
    }

    private fun statInfo(path: ShizukuPath, followLinks: Boolean = true): StatInfo {
        val key = path.pathString
        if (followLinks) cachedStat(key)?.let { return it }
        val json = bridge.stat(key, followLinks) ?: throw NoSuchFileException(key)
        val info = StatInfo(json)
        if (followLinks) rememberStat(key, info)
        return info
    }

    private data class OpenedFile(val pfd: ParcelFileDescriptor, val mode: String)

    /**
     * @see org.apache.sshd.sftp.server.FileHandle.getOpenOptions
     */
    private fun openFile(path: ShizukuPath, options: Set<OpenOption>): OpenedFile {
        val absolute = path.pathString
        val read = StandardOpenOption.READ in options
        val write = StandardOpenOption.WRITE in options
        val readWrite = read && write
        val create = StandardOpenOption.CREATE in options || StandardOpenOption.CREATE_NEW in options
        val createNew = StandardOpenOption.CREATE_NEW in options

        val mode = when {
            readWrite -> "rw"
            write && StandardOpenOption.APPEND in options -> "a"
            write && StandardOpenOption.TRUNCATE_EXISTING in options -> "w"
            write -> "w"
            read -> "r"
            else -> "r"
        }

        // Open first: the common "already exists" path now costs a single binder call, and no
        // separate exists() probe is issued. A null result is treated as "not openable/absent".
        // CREATE_NEW probes with "r" so an existing file is never truncated before we reject it.
        val existing = bridge.openFile(absolute, if (createNew) "r" else mode)
        if (existing != null) {
            if (createNew) {
                existing.close()
                throw FileAlreadyExistsException(absolute)
            }
            return OpenedFile(existing, mode)
        }
        if (!create) throw FileNotFoundException("Failed to open $absolute (mode=$mode)")

        // CREATE: "w" both materialises the file and yields a descriptor (2 calls total); for
        // other modes reopen once in the requested mode.
        val created = bridge.openFile(absolute, "w")
            ?: throw FileNotFoundException("Failed to create $absolute")
        if (mode == "w") return OpenedFile(created, mode)
        created.close()
        val pfd = bridge.openFile(absolute, mode)
            ?: throw FileNotFoundException("Failed to open $absolute (mode=$mode)")
        return OpenedFile(pfd, mode)
    }

    private fun channelFrom(opened: OpenedFile): FileChannel = when (opened.mode) {
        "r" -> FileInputStream(opened.pfd.fileDescriptor).channel
        "rw" -> {
            // RandomAccessFile has no constructor taking a FileDescriptor. Duplicate the
            // descriptor and open it through /proc/self/fd to get a seekable read/write channel.
            val dup = opened.pfd.dup()
            val fd = dup.detachFd()
            RandomAccessFile("/proc/self/fd/$fd", "rw").channel
        }
        else -> FileOutputStream(opened.pfd.fileDescriptor).channel // "w" / "a"
    }

    // ---------------------------------------------------------------- channels

    /**
     * @see org.apache.sshd.sftp.server.FileHandle.getOpenOptions
     */
    override fun newByteChannel(
        path: Path,
        options: Set<OpenOption>,
        vararg attrs: FileAttribute<*>
    ): SeekableByteChannel {
        val opened = openFile(path as ShizukuPath, options)
        val channel = channelFrom(opened)
        return object : SeekableByteChannel by channel {
            @Volatile
            private var closed = false
            @Volatile
            private var pfdClosed = false

            override fun close() {
                if (closed) return
                closed = true
                try {
                    channel.close()
                } finally {
                    // channel and pfd may reference the same fd; each is closed at most once.
                    if (!pfdClosed) {
                        pfdClosed = true
                        opened.pfd.close()
                    }
                }
            }
        }
    }

    /**
     * @see org.apache.sshd.sftp.server.FileHandle.getOpenOptions
     */
    override fun newFileChannel(
        path: Path,
        options: Set<OpenOption>,
        vararg attrs: FileAttribute<*>
    ): FileChannel {
        val opened = openFile(path as ShizukuPath, options)
        return channelFrom(opened)
    }

    // ---------------------------------------------------------------- directory / mutation

    override fun newDirectoryStream(
        dir: Path,
        filter: DirectoryStream.Filter<in Path>
    ): DirectoryStream<Path> {
        val absolute = abs(dir)
        val entries = bridge.listDirectoryEntries(absolute, true)
            ?: throw NoSuchFileException(absolute)
        val fileSystem = dir.fileSystem
        val paths = mutableListOf<Path>()
        for (entry in entries) {
            val obj = JSONObject(entry)
            val name = obj.optString("name")
            if (name.isEmpty()) continue
            val child = fileSystem.getPath(absolute, name)
            // Seed the cache so the client's LSTAT of each child needs no further binder call.
            obj.optJSONObject("stat")?.let { rememberStat(abs(child), StatInfo(it.toString())) }
            if (filter.accept(child)) paths.add(child)
        }
        return object : DirectoryStream<Path> {
            override fun iterator(): MutableIterator<Path> = paths.iterator()

            override fun close() {
                // no-op
            }
        }
    }

    override fun createDirectory(dir: Path, vararg attrs: FileAttribute<*>) {
        val absolute = abs(dir)
        if (absolute == "/") throw IOException("Cannot create root directory")
        if (!bridge.createDirectory(absolute)) {
            throw IOException("Failed to create directory $absolute")
        }
    }

    override fun delete(path: Path) {
        val absolute = abs(path)
        if (absolute == "/") throw IOException("Cannot delete root directory")
        if (!bridge.delete(absolute)) throw NoSuchFileException(absolute)
    }

    override fun copy(source: Path, target: Path, vararg options: CopyOption) {
        val src = abs(source)
        val dst = abs(target)
        val overwrite = StandardCopyOption.REPLACE_EXISTING in options
        if (!overwrite && bridge.exists(dst)) throw FileAlreadyExistsException(dst)
        if (!bridge.copyFile(src, dst, overwrite)) {
            throw IOException("Failed to copy $src to $dst")
        }
    }

    override fun move(source: Path, target: Path, vararg options: CopyOption) {
        val src = abs(source)
        val dst = abs(target)
        val overwrite = StandardCopyOption.REPLACE_EXISTING in options
        if (bridge.rename(src, dst)) return

        // Cross-device / rename failure fallback: copy then delete.
        if (!overwrite && bridge.exists(dst)) throw FileAlreadyExistsException(dst)
        if (!bridge.copyFile(src, dst, overwrite)) {
            throw IOException("Failed to move $src to $dst")
        }
        if (!bridge.delete(src)) {
            throw IOException("Failed to delete source $src after move")
        }
    }

    override fun isSameFile(p1: Path, p2: Path): Boolean {
        if (p1 === p2) return true
        val a1 = abs(p1)
        val a2 = abs(p2)
        val c1 = bridge.canonicalPath(a1) ?: a1
        // Same path string: reuse c1 instead of asking the bridge to resolve it twice.
        val c2 = if (a2 == a1) c1 else (bridge.canonicalPath(a2) ?: a2)
        return c1 == c2
    }

    override fun isHidden(path: Path): Boolean {
        val shizukuPath = path as ShizukuPath
        return shizukuPath.fileName?.toString()?.startsWith(".") == true
    }

    override fun getFileStore(path: Path): FileStore? {
        // No file store abstraction through the shell bridge (SAF behaves the same way).
        return null
    }

    override fun checkAccess(path: Path, vararg modes: AccessMode) {
        val shizukuPath = path as ShizukuPath
        val info = statInfo(shizukuPath)
        for (mode in modes) {
            val allowed = when {
                shizukuPath.isRoot() || info.mode == 0 -> true
                else -> when (mode) {
                    AccessMode.READ -> info.mode and READ_BITS != 0
                    AccessMode.WRITE -> info.mode and WRITE_BITS != 0
                    AccessMode.EXECUTE -> info.mode and EXECUTE_BITS != 0
                }
            }
            if (!allowed) throw AccessDeniedException(shizukuPath.pathString)
        }
    }

    // ---------------------------------------------------------------- attributes

    @Suppress("UNCHECKED_CAST")
    override fun <V : FileAttributeView> getFileAttributeView(
        path: Path,
        type: Class<V>,
        vararg options: LinkOption?
    ): V? {
        val shizukuPath = path as ShizukuPath
        val followLinks = LinkOption.NOFOLLOW_LINKS !in options

        if (type == BasicFileAttributeView::class.java) {
            return object : BasicFileAttributeView {
                override fun name(): String = "basic"

                override fun readAttributes(): BasicFileAttributes =
                    this@ShizukuFileSystemProvider.readAttributes(
                        shizukuPath, BasicFileAttributes::class.java, *options
                    )

                override fun setTimes(
                    lastModifiedTime: FileTime?,
                    lastAccessTime: FileTime?,
                    createTime: FileTime?
                ) {
                    if (lastModifiedTime != null) {
                        bridge.setLastModified(shizukuPath.pathString, lastModifiedTime.toMillis())
                    }
                }
            } as V
        }
        if (type == PosixFileAttributeView::class.java) {
            return object : PosixFileAttributeView {
                override fun name(): String = "posix"

                override fun readAttributes(): PosixFileAttributes =
                    this@ShizukuFileSystemProvider.readAttributes(
                        shizukuPath, PosixFileAttributes::class.java, *options
                    )

                override fun setTimes(
                    lastModifiedTime: FileTime?,
                    lastAccessTime: FileTime?,
                    createTime: FileTime?
                ) {
                    if (lastModifiedTime != null) {
                        bridge.setLastModified(shizukuPath.pathString, lastModifiedTime.toMillis())
                    }
                }

                override fun getOwner(): UserPrincipal? =
                    statInfo(shizukuPath, followLinks).owner?.let { NamedPrincipal(it) }

                override fun setOwner(owner: UserPrincipal?) {
                    // Changing ownership through the shell bridge is not supported.
                }

                override fun setPermissions(perms: MutableSet<PosixFilePermission>?) {
                    if (perms != null) {
                        bridge.chmod(shizukuPath.pathString, permissionsToMode(perms))
                    }
                }

                override fun setGroup(group: GroupPrincipal?) {
                    // Changing the group through the shell bridge is not supported.
                }
            } as V
        }
        return null
    }

    @Suppress("UNCHECKED_CAST")
    override fun <A : BasicFileAttributes> readAttributes(
        path: Path,
        type: Class<A>,
        vararg options: LinkOption?
    ): A {
        val shizukuPath = path as ShizukuPath
        val info = statInfo(shizukuPath, LinkOption.NOFOLLOW_LINKS !in options)
        val attrs = object : PosixFileAttributes {
            override fun lastModifiedTime(): FileTime = FileTime.fromMillis(info.lastModified)
            override fun lastAccessTime(): FileTime = FileTime.fromMillis(info.lastModified)
            override fun creationTime(): FileTime = FileTime.fromMillis(info.lastModified)
            override fun size(): Long = info.size
            override fun fileKey(): Any? = null
            override fun isDirectory(): Boolean = info.directory
            override fun isRegularFile(): Boolean = info.regularFile
            override fun isSymbolicLink(): Boolean = info.symbolicLink
            override fun isOther(): Boolean =
                !info.directory && !info.regularFile && !info.symbolicLink

            override fun owner(): UserPrincipal? = info.owner?.let { NamedPrincipal(it) }
            override fun group(): GroupPrincipal? = info.group?.let { NamedGroupPrincipal(it) }
            override fun permissions(): Set<PosixFilePermission> = permissionsOf(info.mode)
        }
        if (type == BasicFileAttributes::class.java ||
            type == PosixFileAttributes::class.java ||
            type.isAssignableFrom(attrs.javaClass)
        ) {
            return attrs as A
        }
        throw UnsupportedOperationException("readAttributes($path)[${type.simpleName}] N/A")
    }

    override fun readAttributes(
        path: Path,
        attributes: String,
        vararg options: LinkOption?
    ): Map<String, Any?> {
        val shizukuPath = path as ShizukuPath
        val info = statInfo(shizukuPath, LinkOption.NOFOLLOW_LINKS !in options)
        val result = mutableMapOf<String, Any?>()
        if (attributes == "basic" || attributes.startsWith("basic:") || attributes == "*") {
            result += mapOf(
                "isDirectory" to info.directory,
                "isRegularFile" to info.regularFile,
                "isSymbolicLink" to info.symbolicLink,
                "isOther" to (!info.directory && !info.regularFile && !info.symbolicLink),
                "size" to info.size,
                "fileKey" to null,
                "lastModifiedTime" to FileTime.fromMillis(info.lastModified),
                "lastAccessTime" to FileTime.fromMillis(info.lastModified),
                "creationTime" to FileTime.fromMillis(info.lastModified)
            )
        }
        if (attributes == "posix" || attributes.startsWith("posix:") || attributes == "*") {
            result += mapOf(
                "owner" to info.owner?.let { NamedPrincipal(it) },
                "group" to info.group?.let { NamedGroupPrincipal(it) },
                "permissions" to permissionsOf(info.mode)
            )
        }
        return result
    }

    override fun setAttribute(
        path: Path,
        attribute: String,
        value: Any?,
        vararg options: LinkOption?
    ) {
        val absolute = abs(path)
        when (attribute) {
            "posix:permissions", "permissions" -> {
                if (value is Set<*>) {
                    @Suppress("UNCHECKED_CAST")
                    val perms = value as Set<PosixFilePermission>
                    bridge.chmod(absolute, permissionsToMode(perms))
                }
            }

            "basic:lastModifiedTime", "lastModifiedTime" -> {
                if (value is FileTime) {
                    bridge.setLastModified(absolute, value.toMillis())
                }
            }

            "basic:lastAccessTime", "lastAccessTime",
            "basic:creationTime", "creationTime" -> {
                // The shell bridge only supports setting the modification time; ignore.
            }

            else -> {
                // Never throw here: SSHD's SFTP subsystem crashes on unknown attributes.
            }
        }
    }

    // ---------------------------------------------------------------- stat/json helpers

    private class StatInfo(json: String) {
        private val obj = JSONObject(json)

        val size: Long = obj.optLong("size", 0L)
        val directory: Boolean = obj.optBoolean("directory", false)
        val regularFile: Boolean = obj.optBoolean("regularFile", false)
        val symbolicLink: Boolean = obj.optBoolean("symbolicLink", false)
        val lastModified: Long = obj.optLong("lastModified", 0L)
        val mode: Int = obj.optInt("mode", 0)
        val owner: String? = obj.opt("owner") as? String
        val group: String? = obj.opt("group") as? String
    }

    private class NamedPrincipal(private val name: String) : UserPrincipal {
        override fun getName(): String = name
    }

    private class NamedGroupPrincipal(name: String) : GroupPrincipal {
        private val principal = NamedPrincipal(name)
        override fun getName(): String = principal.getName()
    }

    companion object {
        private const val TAG = "ShizukuFileSystemProvider"
        private const val STAT_CACHE_TTL_MS = 2000L

        // Owner / group / other read, write and execute bits of a POSIX mode word.
        private const val READ_BITS = 0x124 // 0444
        private const val WRITE_BITS = 0x092 // 0222
        private const val EXECUTE_BITS = 0x049 // 0111

        private fun permissionsOf(mode: Int): Set<PosixFilePermission> {
            val perms = mutableSetOf<PosixFilePermission>()
            if (mode and 0x100 != 0) perms += PosixFilePermission.OWNER_READ
            if (mode and 0x080 != 0) perms += PosixFilePermission.OWNER_WRITE
            if (mode and 0x040 != 0) perms += PosixFilePermission.OWNER_EXECUTE
            if (mode and 0x020 != 0) perms += PosixFilePermission.GROUP_READ
            if (mode and 0x010 != 0) perms += PosixFilePermission.GROUP_WRITE
            if (mode and 0x008 != 0) perms += PosixFilePermission.GROUP_EXECUTE
            if (mode and 0x004 != 0) perms += PosixFilePermission.OTHERS_READ
            if (mode and 0x002 != 0) perms += PosixFilePermission.OTHERS_WRITE
            if (mode and 0x001 != 0) perms += PosixFilePermission.OTHERS_EXECUTE
            return perms
        }

        private fun permissionsToMode(perms: Set<PosixFilePermission>): Int {
            var mode = 0
            if (PosixFilePermission.OWNER_READ in perms) mode = mode or 0x100
            if (PosixFilePermission.OWNER_WRITE in perms) mode = mode or 0x080
            if (PosixFilePermission.OWNER_EXECUTE in perms) mode = mode or 0x040
            if (PosixFilePermission.GROUP_READ in perms) mode = mode or 0x020
            if (PosixFilePermission.GROUP_WRITE in perms) mode = mode or 0x010
            if (PosixFilePermission.GROUP_EXECUTE in perms) mode = mode or 0x008
            if (PosixFilePermission.OTHERS_READ in perms) mode = mode or 0x004
            if (PosixFilePermission.OTHERS_WRITE in perms) mode = mode or 0x002
            if (PosixFilePermission.OTHERS_EXECUTE in perms) mode = mode or 0x001
            return mode
        }
    }
}
