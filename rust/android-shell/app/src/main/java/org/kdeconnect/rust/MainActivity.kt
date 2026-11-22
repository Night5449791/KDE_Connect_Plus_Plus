package org.kdeconnect.rust

import android.Manifest
import android.app.Activity
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import android.util.Log
import android.view.ViewGroup
import android.widget.Button
import android.widget.EditText
import android.widget.HorizontalScrollView
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView
import android.widget.Toast
import org.json.JSONArray
import org.json.JSONObject

/** 纯 UI：只渲染当前屏幕，所有业务动作都委托给 KcNative / KcBridge。 */
class MainActivity : Activity() {
    private data class DeviceRow(
        val id: String,
        val name: String,
        val paired: Boolean,
        val reachable: Boolean,
    )

    private val listener = object : KcBridge.UiListener {
        override fun onDevicesChanged(json: String) {
            this@MainActivity.runOnUiThread { render(json) }
        }

        override fun onEvent(json: String) {
            this@MainActivity.runOnUiThread { toast(eventText(json)) }
        }

        override fun onClipboardChanged(text: String) {
            this@MainActivity.runOnUiThread { toast("剪贴板已同步") }
        }

        override fun onLog(level: Int, message: String) {
            // Rust 日志由 KcBridge 直接写入 Logcat，UI 不展示。
        }
    }

    private var selectedId: String? = null
    private lateinit var status: TextView
    private lateinit var deviceList: LinearLayout
    private lateinit var clipboardInput: EditText

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        val root = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(24, 24, 24, 24)
            layoutParams = ViewGroup.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT,
            )
        }
        status = TextView(this).apply {
            text = "设备列表加载中…"
            textSize = 18f
        }
        deviceList = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL }
        root.addView(status)
        root.addView(deviceList)

        val actions = LinearLayout(this).apply { orientation = LinearLayout.HORIZONTAL }
        actions.addView(actionButton("请求配对") { KcNative.pair(it) })
        actions.addView(actionButton("接受配对") { KcNative.accept(it) })
        actions.addView(actionButton("取消配对") { KcNative.unpair(it) })
        actions.addView(actionButton("Ping") { KcNative.ping(it, "Hello from Android") })
        actions.addView(actionButton("查找设备") { KcNative.find(it) })
        root.addView(HorizontalScrollView(this).apply { addView(actions) })

        clipboardInput = EditText(this).apply { hint = "剪贴板内容" }
        root.addView(clipboardInput)
        root.addView(Button(this).apply {
            text = "发送剪贴板"
            setOnClickListener {
                val id = selectedId
                if (id == null) {
                    toast("请先选择一个设备")
                } else {
                    val text = clipboardInput.text.toString()
                    runAction { KcNative.setClipboard(id, text) }
                }
            }
        })
        root.addView(Button(this).apply {
            text = "重新发现"
            setOnClickListener { runAction { KcNative.refresh() } }
        })

        setContentView(ScrollView(this).apply { addView(root) })

        try {
            startForegroundService(Intent(this, KcService::class.java))
        } catch (e: Throwable) {
            Log.w(TAG, "cannot start foreground service", e)
        }
        requestNotificationPermission()
    }

    override fun onStart() {
        super.onStart()
        KcBridge.setUiListener(listener)
        // 一次性快照；后续变化由 KcBridge.onDevicesChanged 推送，不轮询。
        Thread {
            val json = try {
                KcNative.devices()
            } catch (t: Throwable) {
                Log.w(TAG, "devices() failed", t)
                null
            }
            runOnUiThread { if (json != null) render(json) }
        }.apply { isDaemon = true }.start()
    }

    override fun onStop() {
        KcBridge.clearUiListener(listener)
        super.onStop()
    }

    private fun actionButton(label: String, action: (String) -> Boolean): Button =
        Button(this).apply {
            text = label
            setOnClickListener {
                val id = selectedId
                if (id == null) {
                    toast("请先选择一个设备")
                } else {
                    runAction { action(id) }
                }
            }
        }

    /** 单次调用放到后台线程，避免 JNI 阻塞 UI；不是轮询或常驻状态。 */
    private fun runAction(action: () -> Boolean) {
        Thread {
            val ok = try {
                action()
            } catch (t: Throwable) {
                Log.w(TAG, "JNI call failed", t)
                false
            }
            runOnUiThread { toast(if (ok) "已发送" else "失败（设备不可达或未配对？）") }
        }.apply { isDaemon = true }.start()
    }

    private fun render(json: String) {
        val rows = try {
            val array = JSONArray(json)
            (0 until array.length()).map { i ->
                val item = array.getJSONObject(i)
                DeviceRow(
                    id = item.optString("device_id").ifEmpty { item.optString("deviceId") },
                    name = item.optString("name"),
                    paired = item.optBoolean("paired"),
                    reachable = item.optBoolean("reachable"),
                )
            }
        } catch (t: Throwable) {
            Log.w(TAG, "invalid devices JSON", t)
            status.text = "设备列表解析失败"
            return
        }
        deviceList.removeAllViews()
        status.text = "设备 (${rows.size}) · 点击选择"
        for (row in rows) {
            val name = row.name.ifEmpty { row.id }
            val view = TextView(this).apply {
                text = "$name · ${if (row.paired) "已配对" else "未配对"} · " +
                    "${if (row.reachable) "在线" else "离线"}\n${row.id}"
                textSize = 16f
                setPadding(8, 12, 8, 12)
                if (row.id == selectedId) setBackgroundColor(0xFFDDEEFF.toInt())
                setOnClickListener {
                    selectedId = row.id
                    render(json)
                }
            }
            deviceList.addView(view)
        }
    }

    private fun eventText(json: String): String = try {
        val obj = JSONObject(json)
        val parts = mutableListOf<String>()
        obj.optString("kind").takeIf { it.isNotEmpty() }?.let { parts.add(it) }
        obj.optString("deviceId").takeIf { it.isNotEmpty() }?.let { parts.add(it) }
        obj.optString("verificationKey").takeIf { it.isNotEmpty() }?.let { parts.add("验证码 $it") }
        obj.optString("message").takeIf { it.isNotEmpty() }?.let { parts.add(it) }
        if (parts.isEmpty()) json else parts.joinToString(" · ")
    } catch (t: Throwable) {
        json
    }

    private fun toast(text: String) {
        Toast.makeText(this, text, Toast.LENGTH_SHORT).show()
    }

    private fun requestNotificationPermission() {
        if (Build.VERSION.SDK_INT >= 33 &&
            checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
        ) {
            requestPermissions(arrayOf(Manifest.permission.POST_NOTIFICATIONS), 1)
        }
    }

    companion object {
        private const val TAG = "MainActivity"
    }
}
