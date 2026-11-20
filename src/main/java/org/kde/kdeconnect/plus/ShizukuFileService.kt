// SPDX-License-Identifier: GPL-2.0-only OR GPL-3.0-only OR LicenseRef-KDE-Accepted-GPL
package org.kde.kdeconnect.plus

import android.os.IBinder
import android.os.ParcelFileDescriptor
import android.os.Process
import android.util.Log
import org.json.JSONObject
import java.io.File
import java.nio.file.Files
import java.nio.file.LinkOption
import java.nio.file.Paths
import java.nio.file.StandardCopyOption
import java.nio.file.attribute.PosixFileAttributes
import java.nio.file.attribute.PosixFilePermission
import java.util.EnumSet

/**
 * Shizuku UserService that performs filesystem operations with the shell (uid 2000) or root
 * (uid 0) identity. The app process has no direct access to paths outside its sandbox, so every
 * operation is forwarded here where the java.nio APIs run with full privileges.
 *
 * No java.nio types cross the binder boundary: attributes are encoded as JSON strings and paths
 * are plain absolute POSIX strings. Every method is guarded so an exception never propagates
 * through the binder and crashes the remote caller.
 */
class ShizukuFileService : IShellFileService.Stub() {

    override fun listDirectory(path: String): Array<String> = try {
        Files.newDirectoryStream(Paths.get(path)).use { stream ->
            stream.map { it.fileName.toString() }.toTypedArray()
        }
    } catch (e: Exception) {
        Log.w(TAG, "listDirectory($path) failed", e)
        emptyArray()
    }

    /**
     * Batch variant of [listDirectory] + [stat]: a single directory traversal reads every entry's
     * attributes in the shell process and returns one JSON string per entry, so browsing a large
     * directory costs one binder round-trip instead of O(N). Each object has "name" and "stat"
     * (the same object returned by [stat], or JSON null when attributes are unavailable).
     */
    override fun listDirectoryEntries(path: String, followLinks: Boolean): Array<String> = try {
        Files.newDirectoryStream(Paths.get(path)).use { stream ->
            val entries = mutableListOf<String>()
            for (entry in stream) {
                val name = entry.fileName.toString()
                // A single unreadable/vanished entry must not fail the whole batch.
                val stat = runCatching { statJson(entry.toString(), followLinks) }
                    .onFailure { Log.w(TAG, "statJson($entry) failed", it) }
                    .getOrNull()
                entries += JSONObject().apply {
                    put("name", name)
                    // "stat" is a nested JSON object (per the AIDL contract), not a JSON string.
                    put("stat", stat?.let { JSONObject(it) } ?: JSONObject.NULL)
                }.toString()
            }
            entries.toTypedArray()
        }
    } catch (e: Exception) {
        Log.w(TAG, "listDirectoryEntries($path, followLinks=$followLinks) failed", e)
        emptyArray()
    }

    override fun stat(path: String, followLinks: Boolean): String? = try {
        statJson(path, followLinks)
    } catch (e: Exception) {
        Log.w(TAG, "stat($path, followLinks=$followLinks) failed", e)
        null
    }

    /** Read POSIX attributes of [path] and encode them as the JSON object returned by [stat]. */
    private fun statJson(path: String, followLinks: Boolean): String? {
        val options = if (followLinks) {
            emptyArray<LinkOption>()
        } else {
            arrayOf(LinkOption.NOFOLLOW_LINKS)
        }
        val p = Paths.get(path)
        if (!Files.exists(p, *options)) return null
        val attrs = Files.readAttributes(p, PosixFileAttributes::class.java, *options)
        return JSONObject().apply {
            put("size", attrs.size())
            put("directory", attrs.isDirectory)
            put("regularFile", attrs.isRegularFile)
            put("symbolicLink", attrs.isSymbolicLink)
            put("lastModified", attrs.lastModifiedTime().toMillis())
            put("mode", permissionsToMode(attrs.permissions()))
            put("owner", attrs.owner().name)
            put("group", attrs.group().name)
        }.toString()
    }

    override fun exists(path: String): Boolean = try {
        Files.exists(Paths.get(path))
    } catch (e: Exception) {
        Log.w(TAG, "exists($path) failed", e)
        false
    }

    override fun canonicalPath(path: String): String? = try {
        Paths.get(path).toRealPath().toString()
    } catch (e: Exception) {
        Log.w(TAG, "canonicalPath($path) failed", e)
        null
    }

    override fun readLink(path: String): String? = try {
        Files.readSymbolicLink(Paths.get(path)).toString()
    } catch (e: Exception) {
        Log.w(TAG, "readLink($path) failed", e)
        null
    }

    override fun openFile(path: String, mode: String): ParcelFileDescriptor? = try {
        val file = File(path)
        require(file.isAbsolute) { "path must be absolute: $path" }
        ParcelFileDescriptor.open(file, ParcelFileDescriptor.parseMode(mode))
    } catch (e: Exception) {
        Log.w(TAG, "openFile($path, $mode) failed", e)
        null
    }

    override fun createDirectory(path: String): Boolean = try {
        Files.createDirectories(Paths.get(path))
        true
    } catch (e: Exception) {
        Log.w(TAG, "createDirectory($path) failed", e)
        false
    }

    override fun delete(path: String): Boolean = try {
        Files.delete(Paths.get(path))
        true
    } catch (e: Exception) {
        Log.w(TAG, "delete($path) failed", e)
        false
    }

    override fun rename(from: String, to: String): Boolean = try {
        Files.move(Paths.get(from), Paths.get(to))
        true
    } catch (e: Exception) {
        Log.w(TAG, "rename($from, $to) failed", e)
        false
    }

    override fun copyFile(from: String, to: String, overwrite: Boolean): Boolean = try {
        if (overwrite) {
            Files.copy(Paths.get(from), Paths.get(to), StandardCopyOption.REPLACE_EXISTING)
        } else {
            Files.copy(Paths.get(from), Paths.get(to))
        }
        true
    } catch (e: Exception) {
        Log.w(TAG, "copyFile($from, $to, overwrite=$overwrite) failed", e)
        false
    }

    override fun setLastModified(path: String, time: Long): Boolean = try {
        Files.setLastModifiedTime(Paths.get(path), java.nio.file.attribute.FileTime.fromMillis(time))
        true
    } catch (e: Exception) {
        Log.w(TAG, "setLastModified($path, $time) failed", e)
        false
    }

    override fun chmod(path: String, mode: Int): Boolean = try {
        Files.setPosixFilePermissions(Paths.get(path), modeToPermissions(mode))
        true
    } catch (e: Exception) {
        Log.w(TAG, "chmod($path, $mode) failed", e)
        false
    }

    override fun createSymlink(target: String, link: String): Boolean = try {
        Files.createSymbolicLink(Paths.get(link), Paths.get(target))
        true
    } catch (e: Exception) {
        Log.w(TAG, "createSymlink($target, $link) failed", e)
        false
    }

    override fun usableSpace(path: String): Long = try {
        Files.getFileStore(Paths.get(path)).usableSpace
    } catch (e: Exception) {
        Log.w(TAG, "usableSpace($path) failed", e)
        0L
    }

    override fun totalSpace(path: String): Long = try {
        Files.getFileStore(Paths.get(path)).totalSpace
    } catch (e: Exception) {
        Log.w(TAG, "totalSpace($path) failed", e)
        0L
    }

    /** Convert a POSIX permission set into the traditional octal-style bit mask (e.g. 0755). */
    private fun permissionsToMode(permissions: Set<PosixFilePermission>): Int {
        var mode = 0
        if (PosixFilePermission.OWNER_READ in permissions) mode = mode or OWNER_READ_BIT
        if (PosixFilePermission.OWNER_WRITE in permissions) mode = mode or OWNER_WRITE_BIT
        if (PosixFilePermission.OWNER_EXECUTE in permissions) mode = mode or OWNER_EXECUTE_BIT
        if (PosixFilePermission.GROUP_READ in permissions) mode = mode or GROUP_READ_BIT
        if (PosixFilePermission.GROUP_WRITE in permissions) mode = mode or GROUP_WRITE_BIT
        if (PosixFilePermission.GROUP_EXECUTE in permissions) mode = mode or GROUP_EXECUTE_BIT
        if (PosixFilePermission.OTHERS_READ in permissions) mode = mode or OTHERS_READ_BIT
        if (PosixFilePermission.OTHERS_WRITE in permissions) mode = mode or OTHERS_WRITE_BIT
        if (PosixFilePermission.OTHERS_EXECUTE in permissions) mode = mode or OTHERS_EXECUTE_BIT
        return mode
    }

    /** Convert an octal-style permission bit mask back into a POSIX permission set. */
    private fun modeToPermissions(mode: Int): Set<PosixFilePermission> {
        val permissions = EnumSet.noneOf(PosixFilePermission::class.java)
        if (mode and OWNER_READ_BIT != 0) permissions.add(PosixFilePermission.OWNER_READ)
        if (mode and OWNER_WRITE_BIT != 0) permissions.add(PosixFilePermission.OWNER_WRITE)
        if (mode and OWNER_EXECUTE_BIT != 0) permissions.add(PosixFilePermission.OWNER_EXECUTE)
        if (mode and GROUP_READ_BIT != 0) permissions.add(PosixFilePermission.GROUP_READ)
        if (mode and GROUP_WRITE_BIT != 0) permissions.add(PosixFilePermission.GROUP_WRITE)
        if (mode and GROUP_EXECUTE_BIT != 0) permissions.add(PosixFilePermission.GROUP_EXECUTE)
        if (mode and OTHERS_READ_BIT != 0) permissions.add(PosixFilePermission.OTHERS_READ)
        if (mode and OTHERS_WRITE_BIT != 0) permissions.add(PosixFilePermission.OTHERS_WRITE)
        if (mode and OTHERS_EXECUTE_BIT != 0) permissions.add(PosixFilePermission.OTHERS_EXECUTE)
        return permissions
    }

    companion object {
        private const val TAG = "KdeConnectPlus/FileSvc"

        private const val OWNER_READ_BIT = 0b100_000_000      // 0400
        private const val OWNER_WRITE_BIT = 0b010_000_000     // 0200
        private const val OWNER_EXECUTE_BIT = 0b001_000_000   // 0100
        private const val GROUP_READ_BIT = 0b000_100_000      // 0040
        private const val GROUP_WRITE_BIT = 0b000_010_000     // 0020
        private const val GROUP_EXECUTE_BIT = 0b000_001_000   // 0010
        private const val OTHERS_READ_BIT = 0b000_000_100     // 0004
        private const val OTHERS_WRITE_BIT = 0b000_000_010    // 0002
        private const val OTHERS_EXECUTE_BIT = 0b000_000_001  // 0001

        /**
         * Entry point Shizuku calls to instantiate the service: a public static no-arg method
         * returning an IBinder.
         */
        @JvmStatic
        fun create(): IBinder {
            Log.i(TAG, "creating file user service, uid=${Process.myUid()}")
            return ShizukuFileService()
        }
    }
}
