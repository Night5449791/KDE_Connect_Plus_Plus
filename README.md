# KDE Connect++ (Rust backend)

本仓库现在是 **Rust 实现的 KDE Connect Android 客户端后端 + 极薄 Kotlin UI/系统 API 适配层**。
原先的 520 个 Kotlin/Java/AIDL 源文件（官方 KDE Connect Android 1.37.3 基线 + Shizuku 剪贴板/文件代理）
已按要求完全删除，不再参与构建。协议行为在重写过程中已逐项映射到 Rust，详见 `rust/docs/ARCHITECTURE.md`。

- 协议基线：KDE Connect protocol v8（官方 Android 1.37.3）
- 许可证：GPL-2.0-only OR GPL-3.0-only OR LicenseRef-KDE-Accepted-GPL（见 `COPYING`、`LICENSES/`）
- 上游：KDE Connect（invent.kde.org/network/kdeconnect-android）

## 目录

```
rust/
  crates/kc-protocol    NetworkPacket / identity / TLS / 配对
  crates/kc-core        设备状态机 / 插件注册表 / 存储 / Platform trait
  crates/kc-transport   LAN UDP+mDNS 发现 / TLS 1.2 链路 / payload 传输
  crates/kc-plugins     24 个插件
  crates/kc-daemon      headless CLI（桌面验证用）
  crates/kc-android     JNI cdylib：拥有全部后台逻辑，推送事件给 Kotlin
  android-shell/        极薄 Kotlin：UI + Android API 无状态适配器
  docs/                 架构、JNI 契约
```

## 构建与测试

```bash
cd rust
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
cargo build -p kc-daemon
```

Android APK 需要 NDK 与 Android SDK：

```bash
cd rust
cargo ndk -t arm64-v8a -o android-shell/app/src/main/jniLibs build --release -p kc-android
cd android-shell && gradle assembleDebug
```

CI：`.github/workflows/rust.yml`（test / clippy / fmt + cargo-ndk 交叉编译 + Gradle 组装 debug APK）。APK 产物为 artifact `kc-android-debug-apk`，内部已包含三个 ABI 的 `libkc_android.so`。

## 与旧实现的关系

Rust 层实现了原 Kotlin 应用的以下后端能力：

- KDE Connect v8 线协议、UDP 1716 广播发现、TCP 1716..1764、mDNS `_kdeconnect._udp`
- TLS 1.2 + EC P-256 自签证书 + TOFU 证书固定 + 隧道内二次 identity 交换
- 配对状态机、8 位校验码、信任设备与插件开关持久化
- 独立 TLS 连接的 payload 传输（1739+）
- 原版全部 24 个插件的包体构造、解析与分发
- Android 侧设备发现、事件循环、周期任务、UI 状态推送

**未移植**：蓝牙链路、Shizuku 剪贴板桥/shell 看门狗/SFTP 文件代理、原版完整 UI（翻译/主题/备份/插件设置）。
Android 系统能力（剪贴板、通知等）由 Kotlin 无状态适配函数调用，逻辑归属见 `rust/docs/JNI_CONTRACT.md`。

## 验证边界

`cargo test --workspace --all-targets` 全部通过（含 v8 TLS 握手、payload 流、插件包体、状态机测试），
clippy/fmt 干净，JNI 符号已验证导出。Kotlin APK 未在本机编译、未在真机运行，未做完整端到端互通测试。
