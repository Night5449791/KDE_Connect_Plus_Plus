/*
 * SPDX-FileCopyrightText: 2026 KDE Connect++ contributors
 *
 * SPDX-License-Identifier: GPL-2.0-only OR GPL-3.0-only OR LicenseRef-KDE-Accepted-GPL
 */
package org.kde.kdeconnect.plus;

/**
 * Binder interface for a Shizuku UserService that runs as shell (uid 2000) or root.
 *
 * The app process has no direct filesystem access to paths outside its sandbox.
 * Every filesystem operation is forwarded here, where the java.nio APIs run with
 * shell privileges. Paths are absolute POSIX paths (e.g. /storage/emulated/0/Download).
 *
 * Return values are kept primitive / Parcelable-friendly so that no java.nio types
 * cross the binder boundary. Attribute maps are encoded as JSON strings.
 */
interface IShellFileService {

    /** List the names of entries directly inside a directory. */
    String[] listDirectory(String path);

    /**
     * Return a JSON object describing a path, or null if it does not exist / is not accessible.
     * Fields: size (long), directory (bool), regularFile (bool), symbolicLink (bool),
     * lastModified (long, millis), mode (int, POSIX permission bits), owner (String), group (String).
     * When followLinks is false, attributes of the link itself are returned.
     */
    String stat(String path, boolean followLinks);

    /** Whether a path exists (optionally following symlinks). */
    boolean exists(String path);

    /** Resolve a path to its canonical (symlink-free) absolute form. */
    String canonicalPath(String path);

    /** Read the target of a symbolic link. */
    String readLink(String path);

    /**
     * Open a file and return a descriptor that the app process may read/write.
     * mode is one of: r, w, rw, a, rws, rwd (as in ParcelFileDescriptor.parseMode).
     */
    ParcelFileDescriptor openFile(String path, String mode);

    /** Create a directory, including parents. Returns true on success. */
    boolean createDirectory(String path);

    /** Delete a file or an empty directory. */
    boolean delete(String path);

    /** Rename / move a path. */
    boolean rename(String from, String to);

    /** Copy a file. When overwrite is true an existing target is replaced. */
    boolean copyFile(String from, String to, boolean overwrite);

    /** Set the last-modified time (millis since epoch). */
    boolean setLastModified(String path, long time);

    /** POSIX chmod — mode is the permission bit mask (e.g. 0755). */
    boolean chmod(String path, int mode);

    /** Create a symbolic link named link pointing at target. */
    boolean createSymlink(String target, String link);

    /** Usable / total bytes of the file store holding the path. */
    long usableSpace(String path);
    long totalSpace(String path);
}
