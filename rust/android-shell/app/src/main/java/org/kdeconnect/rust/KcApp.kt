package org.kdeconnect.rust

import android.app.Application

/** 进程启动时只加载 Rust 动态库；内核初始化与停止由 KcService 负责。 */
class KcApp : Application() {
    override fun onCreate() {
        super.onCreate()
        System.loadLibrary("kc_android")
    }
}
