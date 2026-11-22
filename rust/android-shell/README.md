# KC Rust — Android 薄壳

Kotlin 侧**不包含任何 KDE Connect 后台逻辑**。发现、配对、TLS、插件、存储、事件循环、
周期重发现与状态推送全部由 Rust（`crates/kc-android` 及 `kc-*` crate）完成。
本目录只提供 UI 与无状态的 Android 系统 API 适配。

权威接口定义：[`../docs/JNI_CONTRACT.md`](../docs/JNI_CONTRACT.md)。

## 目录

```
android-shell/
  settings.gradle.kts
  build.gradle.kts
  app/build.gradle.kts
  app/src/main/AndroidManifest.xml
  app/src/main/java/org/kdeconnect/rust/
    KcNative.kt      Kotlin → Rust 的 JNI 声明（无状态）
    KcBridge.kt      Rust → Kotlin 回调 + 系统 API 适配器（无状态）
    KcApp.kt         仅 System.loadLibrary("kc_android")
    MainActivity.kt  纯 UI：列表渲染与按钮
    KcService.kt     前台服务外壳：init/stop，无轮询、无唤醒锁
  app/src/main/jniLibs/   <- 编译出的 .so 放这里（本仓库不含）
```

## 数据流

1. `KcApp` 加载 `libkc_android.so`；`KcService.onCreate` 调 `KcBridge.attach(applicationContext)`
   与 `KcNative.init(filesDir)`，`onDestroy` 调 `KcNative.stop()`。
2. Rust 通过 `onDevicesChanged(json)` 推送设备快照；`MainActivity` 在 onStart/onStop
   注册/注销 `KcBridge.UiListener`，用 `org.json` 解析后只渲染当前屏幕。
3. 按钮只调用 `KcNative`（pair/accept/unpair/ping/find/setClipboard/refresh）；
   `onEvent` 显示 Toast，`onClipboardChanged` 经 `setClipboardText` 写一次系统剪贴板。
4. Rust 需要平台数据时调用 `KcBridge` 的适配器方法；这些方法只做一次系统调用并返回，
   不缓存设备、配对或剪贴板历史。

设备快照 JSON 字段：`device_id`, `name`, `device_type`, `protocol_version`, `paired`,
`reachable`, `link_count`, `loaded_plugins`。

## Kotlin → Rust（`org.kdeconnect.rust.KcNative`）

全部为 `@JvmStatic external`，可从任意线程调用。导出符号格式
`Java_org_kdeconnect_rust_KcNative_<method>`。

| 方法 | 说明 |
|---|---|
| `Boolean init(String dataDir)` | 加载/创建 identity、store、registry、DeviceManager，启动 Rust 后台循环与 LAN provider |
| `String devices()` | 设备快照 JSON 数组，无设备时为 `[]`，不返回 null |
| `Boolean pair(String)` | 请求配对 |
| `Boolean accept(String)` | 接受配对 |
| `Boolean unpair(String)` | 取消配对 |
| `Boolean ping(String, String)` | 发送 ping |
| `Boolean setClipboard(String, String)` | 发送剪贴板包 |
| `Boolean find(String)` | 发送 findmyphone 请求 |
| `Boolean refresh()` | 触发一次重新发现 |
| `Unit stop()` | 停止 Rust 后台循环与 provider |

## Rust → Kotlin（`org.kdeconnect.rust.KcBridge`）

Rust 缓存 `JavaVM`，每次回调 attach 当前线程后调用这些 `@JvmStatic` 静态方法。

| 方法 | 说明 |
|---|---|
| `onDevicesChanged(String)` | 转发给 UI listener 渲染列表 |
| `onEvent(String)` | 转发给 UI listener 显示 Toast |
| `onClipboardChanged(String)` | 一次 `ClipboardManager` 写入并转发给 UI listener |
| `onLog(Int, String)` | 写入 `android.util.Log` 并转发给 UI listener |
| `String deviceName()` | `Build.MODEL`（清洗/截断后的纯适配值） |
| `Int deviceType()` | 0=phone, 1=tablet, 2=desktop, 3=laptop, 4=tv |
| `String? clipboardText()` | 一次剪贴板读取 |
| `Boolean setClipboardText(String)` | 一次剪贴板写入 |
| `notify(String, String)` | 发一条通知 |
| `requestServiceRestart()` | 请求前台服务自愈重启 |

## 构建

**1. 编译 Rust 动态库**（在 `rust/` 工作区根目录执行）：

```sh
cargo ndk -t arm64-v8a -o android-shell/app/src/main/jniLibs build -p kc-android --release
```

产物为 `android-shell/app/src/main/jniLibs/arm64-v8a/libkc_android.so`。需要已安装 `cargo-ndk` 和 Android NDK，并设置 `ANDROID_HOME`/`ANDROID_NDK_HOME`。

**2. 编译 APK**（在 `android-shell/` 目录执行）：

```sh
./gradlew assembleDebug
```

本仓库**不包含 Gradle wrapper**（按设计保持最小）。若目录中没有 `gradlew`/`gradle/wrapper/`，可先在有 Gradle 的机器上运行 `gradle wrapper`，或直接使用系统 Gradle：

```sh
gradle assembleDebug
```

APK 输出在 `app/build/outputs/apk/debug/app-debug.apk`。GitHub Actions 会自动完成上述全部步骤，并把 APK 作为 artifact `kc-android-debug-apk` 上传。

## 说明与边界

- 本外壳没有 Bluetooth 后端；Android 平台特定插件（通知读取、SMS、通讯录、系统音量、
  SFTP 等）的真实系统数据需要 Rust 侧进一步接入，Kotlin 只提供无状态系统调用。
- 没有文件传输 UI、插件开关界面、翻译/主题/备份等 Kotlin 界面。
- Android 14+ 前台服务使用 `connectedDevice` 类型；如需其他类型需调整 manifest 与权限。
- Kotlin/Gradle 未在本机编译（无 Android SDK/kotlinc），也未在真机运行；代码按
  `compileSdk 35` / `minSdk 26` 与 JNI 契约逐项核对语法、导入和 API 级别。
