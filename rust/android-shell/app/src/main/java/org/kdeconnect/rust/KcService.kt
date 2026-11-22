package org.kdeconnect.rust

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.os.IBinder
import android.util.Log

/**
 * 前台服务的纯外壳：建通知渠道、startForeground、启动/停止 Rust 内核。
 * 没有轮询线程、没有唤醒锁、没有设备逻辑。
 */
class KcService : Service() {
    override fun onCreate() {
        super.onCreate()
        createChannel()
        startForeground(NOTIFICATION_ID, notification())
        KcBridge.attach(applicationContext)
        val ok = KcNative.init(filesDir.absolutePath)
        if (!ok) Log.e(TAG, "KcNative.init failed")
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int = START_STICKY

    override fun onBind(intent: Intent): IBinder? = null

    override fun onDestroy() {
        KcNative.stop()
        super.onDestroy()
    }

    private fun createChannel() {
        val channel = NotificationChannel(CHANNEL_ID, "KC Rust", NotificationManager.IMPORTANCE_LOW)
        manager().createNotificationChannel(channel)
    }

    private fun manager(): NotificationManager =
        getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager

    private fun notification(): Notification {
        val pending = PendingIntent.getActivity(
            this,
            0,
            Intent(this, MainActivity::class.java),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        return Notification.Builder(this, CHANNEL_ID)
            .setContentTitle("KDE Connect Rust")
            .setContentText("后台服务运行中")
            .setSmallIcon(android.R.drawable.stat_sys_warning)
            .setContentIntent(pending)
            .setOngoing(true)
            .build()
    }

    companion object {
        private const val TAG = "KcService"
        private const val CHANNEL_ID = "kc-rust"
        private const val NOTIFICATION_ID = 1
    }
}
