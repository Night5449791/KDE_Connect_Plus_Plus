# KDE Connect++ — 状态

自用构建，基于官方 KDE Connect Android **1.37.3**。包名保持 `org.kde.kdeconnect_tp`，
与官方版不能共存。GitHub 仓库：`Night5449791/KDEConnectPlusPlus`（待推送）。

plus/ 与 .plus-patches/ 已删除，相关实现按官方代码结构并入 `src/`。

## 已实现

| 功能 | 位置 |
|---|---|
| Shizuku UserService 剪贴板读写 | `src/main/java/org/kde/kdeconnect/plus/ShizukuClipboardService.kt` |
| AIDL 接口 | `src/main/aidl/org/kde/kdeconnect/plus/IClipboardService.aidl` |
| `cmd clipboard` 命令封装（root 模式） | `plus/ShellClipboardCommands.kt` |
| 应用侧桥接 + 自动重启 | `plus/ShizukuClipboardBridge.kt` |
| 权限请求 Activity | `plus/ShizukuPermissionActivity.kt` |
| 接入官方 ClipboardListener | `plugins/clipboard/ClipboardListener.kt` |
| 接入官方 ClipboardPlugin | `plugins/clipboard/ClipboardPlugin.kt` |
| Shizuku 设置区块 | `plugins/clipboard/ClipboardSettingsFragment.kt` |
| Manifest provider | `AndroidManifest.xml` |
| 依赖 | `gradle/libs.versions.toml`（shizuku 13.6.0） |

### 设计要点

- **shell 模式（adb/无线调试）**：UserService uid 2000，直接 `ClipboardManager` 读写。
- **root/Sui 模式**：uid 0 绝不直接碰剪贴板（Android 12+ 会清空），改走 `cmd clipboard`。
- **自动重启**：`Shizuku.OnBinderReceivedListener` 在 Shizuku 重启后重新 bind UserService
  并拉起 `BackgroundService`，不需要打开 App。
- **防回声**：`setText` 先更新 `currentContent`；插件层再用 `lastAppliedFromRemote` 兜底。
- **轮询间隔**：直接读取 700ms，shell 模式 1500ms（每次都要起进程）。

## 待办

- [ ] 在 GitHub 建 `Night5449791/KDEConnectPlusPlus` 空仓库并推送
- [ ] CI 首次构建验证（`.github/workflows/build.yml`，需 JDK 21 + SDK 37）
- [ ] 真机验证（vivo V2445A / Android 15）：ColorOS 后台复制、电脑→手机写入
- [ ] 若 `cmd clipboard get-primary-clip` 输出格式不同，调整 `ShellClipboardCommands.parseGet`

## 参考

- 官方源码基线：KDE Connect 1.37.3（tag `baseline-1.37.3`）
- Shizuku API：https://github.com/RikkaApps/Shizuku-API

## 注意

- 仓库自带的 `debug.keystore` 是上游的，不能用于发布签名。
- 本目录不符合 `../AGENTS.md` 的 `<Target>_RE` 命名规范，是源码工作副本。
