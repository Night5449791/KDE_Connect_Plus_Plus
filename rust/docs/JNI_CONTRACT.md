# JNI 契约（Rust 拥有全部后台逻辑）

Kotlin 侧只允许做两件事：**UI 渲染** 和 **Android 系统 API 的无状态适配**。
所有后台逻辑（发现、配对、TLS、插件、存储、事件循环、周期任务、平台决策）都在 Rust。

## 1. Kotlin → Rust（`org.kdeconnect.rust.KcNative`）

全部 `@JvmStatic external`，可从任意线程调用；Rust 不跨线程持有 `JNIEnv`。

| 方法 | 说明 |
|---|---|
| `boolean init(String dataDir)` | 创建/加载 identity + store + registry + DeviceManager，启动 Rust 后台事件循环与 LAN provider；重复调用会先停止旧实例 |
| `String devices()` | 返回设备快照 JSON 数组（即使无设备也返回 `[]`，不返回 null） |
| `boolean pair(String deviceId)` | 请求配对 |
| `boolean accept(String deviceId)` | 接受配对 |
| `boolean unpair(String deviceId)` | 解除配对 |
| `boolean ping(String deviceId, String message)` | 发送 ping |
| `boolean setClipboard(String deviceId, String text)` | 发送剪贴板包 |
| `boolean find(String deviceId)` | 发送 findmyphone 请求 |
| `boolean refresh()` | 主动触发一次重新发现 |
| `void stop()` | 停止后台事件循环与 provider |

Rust 导出的符号名格式：`Java_org_kdeconnect_rust_KcNative_<method>`。

## 2. Rust → Kotlin（`org.kdeconnect.rust.KcBridge`）

Kotlin 侧是 `object KcBridge`，只暴露无状态静态方法；Rust 缓存 `JavaVM`，每次回调都
`attach_current_thread` 并查找该类，不缓存 `JNIEnv` 或 jobject 引用。

| Kotlin 方法（@JvmStatic） | Rust 调用时机 | 约束 |
|---|---|---|
| `void onDevicesChanged(String json)` | 设备列表/配对状态/可达性变化 | Kotlin 只负责把 JSON 提交给 UI（如 `LiveData.postValue`） |
| `void onEvent(String json)` | 配对请求、错误、日志等事件，形如 `{"kind":"pairing_request","deviceId":"...","verificationKey":"...","message":"..."}` | Kotlin 只负责显示 Toast/通知或让 UI 决定是否弹窗；不做状态判断 |
| `void onClipboardChanged(String text)` | 收到远端剪贴板内容 | Kotlin 只调用 `ClipboardManager.setPrimaryClip` |
| `void onLog(int level, String message)` | Rust 日志桥接 | 只写 `Log`；级别约定：**1=error, 2=warn, 3=info, 4=debug, 其他=verbose** |

### 系统 API 适配器（Rust 主动拉取）

Rust 的 `AndroidPlatform` 在需要时通过 JNI 调用以下 Kotlin 静态方法；这些方法里**不允许有状态或逻辑**，只做一次系统调用并返回：

| Kotlin 方法（@JvmStatic） | 返回 | 用途 |
|---|---|---|
| `String deviceName()` | 设备名 | identity 广播 |
| `int deviceType()` | 0=phone,1=tablet,2=desktop,3=laptop,4=tv | identity 广播 |
| `String clipboardText()` | 当前剪贴板文本或 `null` | 剪贴板包构造 |
| `boolean setClipboardText(String)` | 是否成功 | `Platform::clipboard_set` |
| `void notify(String title, String message)` | — | ping / 配对请求 / findmyphone |
| `void requestServiceRestart()` | — | Rust 判断需要自愈时请求重启前台服务 |

## 3. 后台逻辑归属清单

| 逻辑 | 负责方 |
|---|---|
| UDP/TCP/mDNS 发现、TLS 握手、payload 传输 | Rust `kc-transport` |
| 配对状态机、校验码、信任存储 | Rust `kc-protocol` / `kc-core` |
| 24 插件收发、包体构造 | Rust `kc-plugins` |
| 设备列表、插件重载、事件分发 | Rust `kc-core::DeviceManager` |
| 周期重发现、断线重连、状态推送 UI | Rust `kc-android` 后台任务 |
| 剪贴板“读取/写入”的**决策**（何时发、防回声） | Rust |
| 剪贴板/通知/SMS 等 Android API 的**调用** | Kotlin 无状态适配方法 |
| UI（按钮、列表、输入框）、前台服务生命周期壳 | Kotlin |

## 4. 明确禁止

- Kotlin 侧不得持有设备列表、配对状态、剪贴板历史等后台状态。
- Kotlin 侧不得起轮询线程（设备列表由 Rust 通过 `onDevicesChanged` 推送）。
- Kotlin 侧不得直接构造或解析 KDE Connect 包（一律走 `KcNative`）。
