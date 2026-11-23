/*
 * SPDX-FileCopyrightText: 2014 Samoilenko Yuri <kinnalru@gmail.com>
 * SPDX-FileCopyrightText: 2024 ShellWen Chen <me@shellwen.com>
 *
 * SPDX-License-Identifier: GPL-2.0-only OR GPL-3.0-only OR LicenseRef-KDE-Accepted-GPL
 */
package org.kde.kdeconnect.plugins.sftp

import android.content.Context
import android.os.Build
import android.util.Log
import androidx.core.net.toUri
import org.apache.sshd.common.keyprovider.AbstractKeyPairProvider
import org.apache.sshd.common.session.Session
import org.apache.sshd.common.session.SessionContext
import org.apache.sshd.common.session.SessionListener
import org.apache.sshd.common.util.io.PathUtils
import org.apache.sshd.common.util.security.SecurityUtils.SECURITY_PROVIDER_REGISTRARS
import org.apache.sshd.scp.server.ScpCommandFactory
import org.apache.sshd.server.ServerBuilder
import org.apache.sshd.server.SshServer
import org.apache.sshd.server.auth.password.PasswordAuthenticator
import org.apache.sshd.server.auth.pubkey.PublickeyAuthenticator
import org.apache.sshd.server.session.ServerSession
import org.apache.sshd.server.subsystem.SubsystemFactory
import org.apache.sshd.sftp.server.FileHandle
import org.apache.sshd.sftp.server.SftpFileSystemAccessor
import org.apache.sshd.sftp.server.SftpSubsystemFactory
import org.apache.sshd.sftp.server.SftpSubsystemProxy
import org.kde.kdeconnect.Device
import org.kde.kdeconnect.helpers.MediaStoreHelper
import org.kde.kdeconnect.helpers.RandomHelper
import org.kde.kdeconnect.helpers.security.RsaHelper
import org.kde.kdeconnect.helpers.security.constantTimeCompare
import org.kde.kdeconnect.plugins.sftp.saf.SafFileSystemFactory
import org.kde.kdeconnect.plugins.sftp.saf.SafPath
import org.kde.kdeconnect.plugins.sftp.shizuku.ShizukuFileSystemFactory
import org.slf4j.impl.HandroidLoggerAdapter
import java.io.IOException
import java.nio.channels.Channel
import java.nio.channels.SeekableByteChannel
import java.nio.charset.StandardCharsets
import java.nio.file.CopyOption
import java.nio.file.OpenOption
import java.nio.file.Path
import java.nio.file.StandardOpenOption
import java.nio.file.attribute.FileAttribute
import java.security.KeyPair
import java.security.MessageDigest
import java.security.NoSuchAlgorithmException
import java.security.PublicKey
import java.util.Timer
import java.util.TimerTask

internal class SimpleSftpServer {
    private lateinit var sshd: SshServer

    val port: Int
        get() {
            if (!::sshd.isInitialized) return -1
            return sshd.port
        }

    val isStarted: Boolean
        get() {
            if (!::sshd.isInitialized) return false
            return sshd.isStarted
        }

    val isClosed: Boolean
        get() {
            if (!::sshd.isInitialized) return false
            return sshd.isClosed
        }

    private val passwordAuth = SimplePasswordAuthenticator()
    private val keyAuth = SimplePublicKeyAuthenticator()

    val isInitialized: Boolean
        get() = ::sshd.isInitialized

    private lateinit var safFileSystemFactory: SafFileSystemFactory
    private lateinit var nativeFileSystemFactory: NativeStorageFileSystemFactory

    private var shizukuMode = false

    // ---- 空闲自动停止（省电）----
    // SSH 服务启动后会在 1739 端口常驻监听并持有 NIO 线程池。桌面端关闭文件管理器后如果没有任何
    // 连接，这些资源会一直占用并阻止系统进入 Doze。这里用 SSHD 的 SessionListener 跟踪活跃连接：
    // 有连接时取消倒计时；连接归零（或启动后一直没有连接）时启动 IDLE_TIMEOUT_MS 倒计时，超时
    // 调用 stop(true) 释放端口和线程。省电效果：不用文件时不再有常驻监听 socket / 线程被唤醒。
    private val idleLock = Any()
    @Volatile private var activeSessionCount = 0
    private var idleTimer: Timer? = null
    @Volatile private var serverStopping = false

    /** [enabled] defaults to true for compatibility; pass false when switching back to Android storage mode. */
    fun setShizukuMode(enabled: Boolean = true) {
        shizukuMode = enabled
    }

    fun setSafStorageDirs(storageInfoList: List<SftpPlugin.StorageInfo>) {
        safFileSystemFactory.initRoots(storageInfoList)
    }

    fun setNativeStorageDirs(paths: List<Path>) {
        nativeFileSystemFactory.storageDirs = paths
    }

    fun initialize(context: Context, device: Device) {
        val sshd = ServerBuilder.builder().apply {
            fileSystemFactory(
                if (shizukuMode) {
                    ShizukuFileSystemFactory()
                } else if (SUPPORTS_NATIVEFS) {
                    nativeFileSystemFactory = NativeStorageFileSystemFactory()
                    nativeFileSystemFactory
                } else {
                    safFileSystemFactory = SafFileSystemFactory(context)
                    safFileSystemFactory
                }
            )
        }.build()

        // Reuse this device keys for the ssh connection as well
        val keyPair = KeyPair(
            RsaHelper.getPublicKey(context),
            RsaHelper.getPrivateKey(context)
        )
        sshd.keyPairProvider = object : AbstractKeyPairProvider() {
            override fun loadKeys(session: SessionContext): Iterable<KeyPair> = listOf(keyPair)
        }

        sshd.commandFactory = ScpCommandFactory()
        sshd.subsystemFactories =
            listOf<SubsystemFactory>(SftpSubsystemFactory.Builder().apply {
                withFileSystemAccessor(object : SftpFileSystemAccessor {
                    fun notifyMediaStore(path: Path) {
                        kotlin.runCatching {
                            val uri = path.toUri().toString().toUri()
                            MediaStoreHelper.indexFile(context, uri)
                            uri
                        }.fold(
                            onSuccess = { Log.i(TAG, "Notified media store: $path, $it") },
                            onFailure = { Log.w(TAG, "Failed to notify media store: $path", it) }
                        )
                    }

                    override fun openFile(
                        subsystem: SftpSubsystemProxy?,
                        fileHandle: FileHandle?,
                        file: Path?,
                        handle: String?,
                        options: MutableSet<out OpenOption>?,
                        vararg attrs: FileAttribute<*>?
                    ): SeekableByteChannel {
                        if (file is SafPath) {
                            return file.fileSystem.provider().newByteChannel(file, options, *attrs)
                        }
                        return super.openFile(subsystem, fileHandle, file, handle, options, *attrs)
                    }

                    override fun removeFile(
                        subsystem: SftpSubsystemProxy?,
                        path: Path?,
                        isDirectory: Boolean
                    ) {
                        super.removeFile(subsystem, path, isDirectory)
                        path?.let { notifyMediaStore(it) }
                    }

                    override fun copyFile(
                        subsystem: SftpSubsystemProxy?,
                        src: Path?,
                        dst: Path?,
                        opts: MutableCollection<CopyOption>?
                    ) {
                        super.copyFile(subsystem, src, dst, opts)
                        dst?.let { notifyMediaStore(it) }
                    }

                    override fun renameFile(
                        subsystem: SftpSubsystemProxy?,
                        oldPath: Path?,
                        newPath: Path?,
                        opts: MutableCollection<CopyOption>?
                    ) {
                        super.renameFile(subsystem, oldPath, newPath, opts)
                        oldPath?.let { notifyMediaStore(it) }
                        newPath?.let { notifyMediaStore(it) }
                    }

                    override fun createLink(
                        subsystem: SftpSubsystemProxy?,
                        link: Path?,
                        existing: Path?,
                        symLink: Boolean
                    ) {
                        super.createLink(subsystem, link, existing, symLink)
                        link?.let { notifyMediaStore(it) }
                        existing?.let { notifyMediaStore(it) }
                    }

                    override fun closeFile(
                        subsystem: SftpSubsystemProxy?,
                        fileHandle: FileHandle?,
                        file: Path?,
                        handle: String?,
                        channel: Channel?,
                        options: MutableSet<out OpenOption>?
                    ) {
                        super.closeFile(subsystem, fileHandle, file, handle, channel, options)
                        if (options?.contains(StandardOpenOption.WRITE) == true) {
                            file?.let { notifyMediaStore(it) }
                        }
                    }
                })
            }.build())

        keyAuth.deviceKey = device.certificate.publicKey

        sshd.publickeyAuthenticator = keyAuth
        sshd.passwordAuthenticator = passwordAuth

        this.sshd = sshd

        // SSHD 2.20.1: SessionListenerManager.addSessionListener(SessionListener)，
        // sessionCreated/sessionClosed 均为 default 方法，只覆写这两个即可。
        // 在 start() 之前注册，避免连接建立/拆除过程中注册导致回调顺序不确定。
        sshd.addSessionListener(object : SessionListener {
            override fun sessionCreated(session: Session) {
                synchronized(idleLock) {
                    activeSessionCount++
                    cancelIdleTimerLocked()
                }
                Log.i(TAG, "SFTP session created (active=$activeSessionCount), cancelled idle-stop timer")
            }

            override fun sessionClosed(session: Session) {
                synchronized(idleLock) {
                    if (activeSessionCount > 0) activeSessionCount--
                }
                Log.i(TAG, "SFTP session closed (active=$activeSessionCount)")
                scheduleIdleStopIfNeeded()
            }
        })
    }

    fun start(): Boolean {
        if (isStarted) return true

        cancelIdleTimer()

        regeneratePassword()

        PORT_RANGE.forEach { port ->
            try {
                sshd.port = port
                sshd.start()
                // 省电：启动后即使没有客户端连接也安排空闲倒计时，防止端口/线程一直常驻
                scheduleIdleStopIfNeeded()

                return true
            } catch (e: IOException) {
                Log.w("SftpServer", "Failed to start server on port $port, trying next port", e)
            }
        }

        Log.e("SftpServer", "No more ports available")
        return false
    }

    fun stop() {
        cancelIdleTimer()
        if (!::sshd.isInitialized) return

        synchronized(idleLock) { serverStopping = true }
        try {
            sshd.stop(true)
        } catch (e: Exception) {
            Log.e("SFTP", "Exception while stopping the server", e)
        } finally {
            // stop 期间可能仍有 sessionClosed 回调到达，serverStopping 阻止它们重新安排定时器
            synchronized(idleLock) {
                activeSessionCount = 0
                serverStopping = false
            }
        }
    }

    // ---- 空闲倒计时实现 ----
    private fun scheduleIdleStopIfNeeded() {
        synchronized(idleLock) {
            if (activeSessionCount == 0 && isStarted && !serverStopping) {
                scheduleIdleStopLocked()
            }
        }
    }

    private fun scheduleIdleStopLocked() {
        cancelIdleTimerLocked()
        val timer = Timer("SftpIdleStop", true) // daemon 线程，不阻止进程退出
        idleTimer = timer
        timer.schedule(object : TimerTask() {
            override fun run() {
                // 锁内只做判定并释放锁，真正的 stop() 放到锁外执行，
                // 避免与 SSHD 关闭 session 的回调（同样需要 idleLock）互相等待。
                val shouldStop = synchronized(idleLock) {
                    if (activeSessionCount == 0 && isStarted && !serverStopping && idleTimer === timer) {
                        idleTimer = null
                        true
                    } else {
                        false
                    }
                }
                if (shouldStop) {
                    Log.i(TAG, "SFTP idle for ${IDLE_TIMEOUT_MS / 1000}s, stopping server to save battery")
                    stop()
                }
            }
        }, IDLE_TIMEOUT_MS)
    }

    private fun cancelIdleTimer() {
        synchronized(idleLock) { cancelIdleTimerLocked() }
    }

    private fun cancelIdleTimerLocked() {
        idleTimer?.cancel()
        idleTimer = null
    }

    fun regeneratePassword(): String {
        return RandomHelper.randomString(28).also {
            passwordAuth.setPassword(it)
        }
    }

    internal class SimplePasswordAuthenticator : PasswordAuthenticator {
        private val sha: MessageDigest = try {
            MessageDigest.getInstance("SHA-256")
        } catch (e: NoSuchAlgorithmException) {
            throw RuntimeException(e)
        }
        private var passwordHash: ByteArray = byteArrayOf()

        fun setPassword(password: String) {
            passwordHash = sha.digest(password.toByteArray(StandardCharsets.UTF_8))
        }

        override fun authenticate(user: String, password: String, session: ServerSession): Boolean {
            val receivedPasswordHash = sha.digest(password.toByteArray(StandardCharsets.UTF_8))
            return user == USER && constantTimeCompare(passwordHash, receivedPasswordHash)
        }
    }

    internal class SimplePublicKeyAuthenticator : PublickeyAuthenticator {
        var deviceKey: PublicKey? = null

        override fun authenticate(user: String, key: PublicKey, session: ServerSession): Boolean =
            user == USER && deviceKey == key
    }

    companion object {
        private const val TAG = "SimpleSftpServer"

        val SUPPORTS_NATIVEFS = Build.VERSION.SDK_INT >= Build.VERSION_CODES.R

        private val PORT_RANGE = 1739..1764
        const val USER: String = "kdeconnect"

        /** 空闲多少毫秒后停止 SFTP 服务（3 分钟），省电并允许系统进入 Doze */
        private const val IDLE_TIMEOUT_MS = 180_000L

        init {
            System.setProperty(SECURITY_PROVIDER_REGISTRARS, "") // disable BouncyCastle
            System.setProperty(
                "org.apache.sshd.common.io.IoServiceFactoryFactory",
                if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) {
                    // Use MINA instead NIO2 due to compatibility issues
                    // Android 7.1 (API 25) and below have issues with NIO2
                    // When we require API 26, we can remove this and the Mina dependency.
                    "org.apache.sshd.mina.MinaServiceFactoryFactory"
                } else {
                    "org.apache.sshd.common.io.nio2.Nio2ServiceFactoryFactory"
                }
            )
            // Remove it when SSHD Core is fixed.
            // Android has no user home folder, so we need to set it to something.
            // `System.getProperty("user.home")` is not available on Android,
            // but it exists in SSHD Core's `org.apache.sshd.common.util.io.PathUtils.LazyDefaultUserHomeFolderHolder`.
            PathUtils.setUserHomeFolderResolver { Path.of("/") }

            // Disable SSHD logging due to performance degradation and being very noisy even in development
            HandroidLoggerAdapter.DEBUG = false
        }
    }
}
