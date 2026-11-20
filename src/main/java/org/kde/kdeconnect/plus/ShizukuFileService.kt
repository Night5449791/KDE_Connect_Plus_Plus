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
    } catch (t: Throwable) {
        Log.w(TAG, "listDirectory($path) failed", t)
        emptyArray()
    }

    override fun stat(path: String, followLinks: Boolean): String? = try {
        val options = if (followLinks) {
            emptyArray<LinkOption>()
        } else {
            arrayOf(LinkOption.NOFOLLOW_LINKS)
        }
        val p = Paths.get(path)
        if (!Files.exists(p, *options)) return null
        val attrs = Files.readAttributes(p, PosixFileAttributes::class.java, *options)
        JSONObject().apply {
            put("size", attrs.size())
            put("directory", attrs.isDirectory)
            put("regularFile", attrs.isRegularFile)
            put("symbolicLink", attrs.isSymbolicLink)
            put("lastModified", attrs.lastModifiedTime().toMillis())
            put("mode", permissionsToMode(attrs.permissions()))
            put("owner", attrs.owner().name)
            put("group", attrs.group().name)
        }.toString()
    } catch (t: Throwable) {
        Log.w(TAG, "stat($path, followLinks=$followLinks) failed", t)
        null
    }

    override fun exists(path: String): Boolean = try {
        Files.exists(Paths.get(path))
    } catch (t: Throwable) {
        Log.w(TAG, "exists($path) failed", t)
        false
    }

    override fun canonicalPath(path: String): String? = try {
        Paths.get(path).toRealPath().toString()
    } catch (t: Throwable) {
        Log.w(TAG, "canonicalPath($path) failed", t)
        null
    }

    override fun readLink(path: String): String? = try {
        Files.readSymbolicLink(Paths.get(path)).toString()
    } catch (t: Throwable) {
        Log.w(TAG, "readLink($path) failed", t)
        null
    }

    override fun openFile(path: String, mode: String): ParcelFileDescriptor? = try {
        val file = File(path)
        require(file.isAbsolute) { "path must be absolute: $path" }
        ParcelFileDescriptor.open(file, ParcelFileDescriptor.parseMode(mode))
    } catch (t: Throwable) {
        Log.w(TAG, "openFile($path, $mode) failed", t)
        null
    }

    override fun createDirectory(path: String): Boolean = try {
        Files.createDirectories(Paths.get(path))
        true
    } catch (t: Throwable) {
        Log.w(TAG, "createDirectory($path) failed", t)
        false
    }

    override fun delete(path: String): Boolean = try {
        Files.delete(Paths.get(path))
        true
    } catch (t: Throwable) {
        Log.w(TAG, "delete($path) failed", t)
        false
    }

    override fun rename(from: String, to: String): Boolean = try {
        Files.move(Paths.get(from), Paths.get(to))
        true
    } catch (t: Throwable) {
        Log.w(TAG, "rename($from, $to) failed", t)
        false
    }

    override fun copyFile(from: String, to: String, overwrite: Boolean): Boolean = try {
        if (overwrite) {
            Files.copy(Paths.get(from), Paths.get(to), StandardCopyOption.REPLACE_EXISTING)
        } else {
            Files.copy(Paths.get(from), Paths.get(to))
        }
        true
    } catch (t: Throwable) {
        Log.w(TAG, "copyFile($from, $to, overwrite=$overwrite) failed", t)
        false
    }

    override fun setLastModified(path: String, time: Long): Boolean = try {
        Files.setLastModifiedTime(Paths.get(path), java.nio.file.attribute.FileTime.fromMillis(time))
        true
    } catch (t: Throwable) {
        Log.w(TAG, "setLastModified($path, $time) failed", t)
        false
    }

    override fun chmod(path: String, mode: Int): Boolean = try {
        Files.setPosixFilePermissions(Paths.get(path), modeToPermissions(mode))
        true
    } catch (t: Throwable) {
        Log.w(TAG, "chmod($path, $mode) failed", t)
        false
    }

    override fun createSymlink(target: String, link: String): Boolean = try {
        Files.createSymbolicLink(Paths.get(link), Paths.get(target))
        true
    } catch (t: Throwable) {
        Log.w(TAG, "createSymlink($target, $link) failed", t)
        false
    }

    override fun usableSpace(path: String): Long = try {
        Files.getFileStore(Paths.get(path)).usableSpace
    } catch (t: Throwable) {
        Log.w(TAG, "usableSpace($path) failed", t)
        0L
    }

    override fun totalSpace(path: String): Long = try {
        Files.getFileStore(Paths.get(path)).totalSpace
    } catch (t: Throwable) {
        Log.w(TAG, "totalSpace($path) failed", t)
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
