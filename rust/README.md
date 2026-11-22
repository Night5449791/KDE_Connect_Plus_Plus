# KDE Connect++ Rust core

Rust 重写的后端 + 极薄 Kotlin UI/系统 API 适配层。原 520 个 Kotlin/Java/AIDL 源文件（KDE Connect Android 1.37.3 + Shizuku 剪贴板/文件代理）已按用户要求完全删除；协议行为已映射到 Rust，见 `docs/ARCHITECTURE.md`。

## 工作区结构

| crate | 作用 | 对应 Kotlin 代码 |
|---|---|---|
| `kc-protocol` | NetworkPacket 线协议、identity、证书/TLS、配对状态机 | `NetworkPacket.kt`、`DeviceInfo.kt`、`PairingHandler.kt`、`RsaHelper.kt`、`SslHelper.kt` |
| `kc-core` | 设备状态机、插件注册表、平台回调、存储 | `Device.kt`、`Plugin.kt`、`PluginFactory.kt`、`TrustedDevices.kt` |
| `kc-transport` | LAN UDP/mDNS 发现、TLS 链路、payload 传输、loopback | `LanLinkProvider.kt`、`LanLink.kt`、`MdnsDiscovery.kt` |
| `kc-plugins` | 24 个插件的包体构造与收包处理 | `plugins/**/*.kt` |
| `kc-daemon` | headless 守护进程与 CLI | 新增（便于桌面验证协议） |
| `kc-android` | JNI `cdylib` | 新增（供 Kotlin 壳调用） |

## 构建与测试

```bash
cd rust
cargo test --workspace        # 单元测试
cargo clippy --workspace --all-targets -- -D warnings
cargo build -p kc-daemon      # headless CLI
cargo build -p kc-android     # 本机 JNI 库（Android 目标见 CI）
```

CI：`.github/workflows/rust.yml`（cargo test/clippy/fmt + cargo-ndk 交叉编译 arm64-v8a / armeabi-v7a / x86_64 + Gradle 组装 `app-debug.apk`，产物 artifact 为 `kc-android-debug-apk`）。

## 运行 headless daemon

```bash
cargo run -p kc-daemon -- devices
cargo run -p kc-daemon -- serve
# 另一个终端（共享同一 store 文件）
cargo run -p kc-daemon -- pair <deviceId>
cargo run -p kc-daemon -- ping <deviceId> hello
cargo run -p kc-daemon -- clipboard set "text"
```

`RUST_LOG=debug` 打开详细日志。默认 store 路径见 `kc-daemon/src/main.rs`。

## Android 壳

见 `android-shell/README.md`。流程：

```bash
cd rust
cargo ndk -t arm64-v8a -o ../android-shell/app/src/main/jniLibs build --release -p kc-android
cd android-shell && ./gradlew assembleDebug
```

## 协议兼容性要点

- 线上包：换行结尾的 UTF-8 JSON `{id,type,body,payloadSize?,payloadTransferInfo?}`，`payloadSize` 与 `payloadTransferInfo` 必须成对出现。
- LAN：UDP 1716 广播 → TCP 1716..1764 → TLS **1.2** → v8 在 TLS 内二次交换 identity；payload 走独立端口（1739+）的第二条 TLS 连接，字节流本身没有长度头。
- 密钥：EC P-256 自签证书，DN `CN=<deviceId>, OU=KDE Connect, O=KDE`；信任模型 TOFU + 证书 pinning；配对校验码 = SHA256(sorted(pubkeys) || timestamp) 的前 8 位大写 hex。
- 包类型与 24 个插件的收发能力表见 `docs/ARCHITECTURE.md`。

## 已知限制

- 蓝牙后端、Shizuku 剪贴板/文件代理（`plus/`）尚未移植：Android 壳目前只覆盖 LAN 路径，剪贴板走普通 `ClipboardManager`，没有 shell uid 2000 的无焦点读写与看门狗自愈。
- Android 平台特定插件（通知读取、SMS、通讯录、系统音量、SFTP 服务端）在 Rust 层只实现协议包与解析；真实数据需要 Kotlin 壳通过 JNI 回调填充，部分回调当前为占位实现。
- `kc-daemon` 的各子命令通过共享 store 文件协同；`serve` 与 `pair/ping` 之间没有 RPC，未运行 `serve` 时这些命令只会构造/持久化状态，不会真正发包。
- 所有验证均为静态/单元测试；未在真机或真实 KDE Connect 桌面端做端到端互通测试。
