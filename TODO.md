# KDE Connect++ — 状态

自用构建，基于官方 KDE Connect Android **1.37.3**。包名保持 `org.kde.kdeconnect_tp`，
与官方版不能共存。GitHub 仓库：`Night5449791/KDE_Connect_Plus_Plus`（代码已推送）。

`plus/` 与 `.plus-patches/` 目录已删除，相关实现按官方代码结构并入 `src/`。

## 已实现

| 功能 | 位置 |
|---|---|
| Shizuku UserService 剪贴板读写 | `src/main/java/org/kde/kdeconnect/plus/ShizukuClipboardService.kt` |
| 以 `com.android.shell` 包名直连 `IClipboard` binder | `src/main/java/org/kde/kdeconnect/plus/ShellClipboardAccess.kt` |
| AIDL 接口 | `src/main/aidl/org/kde/kdeconnect/plus/IClipboardService.aidl` |
| `cmd clipboard` 命令封装（root 模式） | `src/main/java/org/kde/kdeconnect/plus/ShellClipboardCommands.kt` |
| 应用侧桥接 + 自动重启 | `src/main/java/org/kde/kdeconnect/plus/ShizukuClipboardBridge.kt` |
| 独立 shell 看门狗（被杀后自动复活） | `src/main/java/org/kde/kdeconnect/plus/ShellWatchdog.kt` |
| 权限请求 Activity | `src/main/java/org/kde/kdeconnect/plus/ShizukuPermissionActivity.kt` |
| 接入官方 ClipboardListener | `src/main/java/org/kde/kdeconnect/plugins/clipboard/ClipboardListener.kt` |
| 接入官方 ClipboardPlugin | `src/main/java/org/kde/kdeconnect/plugins/clipboard/ClipboardPlugin.kt` |
| Shizuku 设置区块 | `src/main/java/org/kde/kdeconnect/plugins/clipboard/ClipboardSettingsFragment.kt` |
| Manifest provider | `src/main/AndroidManifest.xml` |
| 依赖 | `gradle/libs.versions.toml`、`build.gradle.kts`（Shizuku 13.1.5） |

### 设计要点

- **shell 模式（adb/无线调试）**：UserService uid 2000，`ShellClipboardAccess` 反射 `ServiceManager`
  拿到 `IClipboard` binder，显式传 callingPackage `com.android.shell`（AOSP 对 uid 2000 的 special-case，
  `mAppOps.checkPackage` + `addActiveOwnerLocked` 全过且免窗口焦点）。不能用 `ClipboardManager`：
  它传的是 `mContext.getOpPackageName()`（仍是应用包），`createPackageContext` 改不了它。
  签名按参数类型分派：AOSP 12/13 为 `getPrimaryClip(String,int)`，14/15 加 `attributionTag`/`deviceId`；
  vivo V2445A 实测扩展为 `getPrimaryClip(String,String,String,String,int,int,boolean)` 与
  `setPrimaryClip(ClipData,String,String,int,int,boolean)`，末尾 boolean 必须填 `false`（填 null 会
  抛 `IllegalArgumentException` 导致回退到公开 API 而失败）。厂商可能继续魔改，故按类型填值而非按位置硬编码。
- **root/Sui 模式**：uid 0 绝不直接碰剪贴板（Android 12+ 会清空），改走 `cmd clipboard`；但当前 V2445A / Android 15 返回 `No shell command implementation.`，该设备上的 root/Sui 剪贴板后端不可用，替代方案待实现。
- **自动重启（被杀后自动复活）**：`ShellWatchdog` 在 UserService 首次启动时用 shell(uid 2000)
  权限拉起一个**独立的 `nohup sh` 看门狗进程**（pid 写 `/data/local/tmp/kc_watchdog.pid`），
  每 20 秒用 `dumpsys activity services` 检测 `BackgroundService`，不在则
  `cmd package unstop` + appops/standby 白名单 + `am start-foreground-service` 拉起。
  关键：看门狗必须是独立进程——曾尝试把定时检测放进 Shizuku UserService，但 App 被 force-stop/
  冻结时 Shizuku 会取消 UserService 的 binder 工作，导致定时检测恰好在最需要时停止（死锁）。
  独立 `nohup` 进程经真机验证在 force-stop 期间持续运行。`BackgroundService` 已设 `exported=true`
  且无条件调用 `startForeground()`（关闭持久通知时用最小通知），否则 shell 拉起后 5 秒被杀。
  `Shizuku.OnBinderReceivedListener` 在 Shizuku 重启后重新 bind 并触发一次立即 relaunch。
- **防回声**：`setText` 先更新 `currentContent`；插件层再用 `lastAppliedFromRemote` 兜底。
- **轮询间隔**：直接读取 700ms，shell 模式 1500ms（每次都要起进程）。

## 待办

- [x] 创建 GitHub 仓库并推送：`Night5449791/KDE_Connect_Plus_Plus`（`master` 已同步）
- [x] CI 构建验证：run [44253139251](https://github.com/Night5449791/KDE_Connect_Plus_Plus/actions/runs/44253139251) 成功，含剪贴板命令错误回归测试，Debug APK 构建和上传均通过
- [x] 端到端真机验证（vivo V2445A / Android 15）：`IClipboard` 直连路径读、写双向均成功。
  读方向日志 `IClipboard read ok (chars=79)`；写方向桌面 `--send-clipboard` 后手机剪贴板由 0 字符变为
  27 字符探针文本。注意：UserService 是 daemon，改代码后需杀掉 `:clipboard` 进程才会用新 dex 重启；
  `am force-stop` 可能破坏 Shizuku provider，需重启 Shizuku manager 恢复。
- [x] 在 V2445A / Android 15 上探测 `cmd clipboard get-primary-clip`：返回 stderr `No shell command implementation.`、stdout 为空且退出码为 0；后端现将该响应判为失败，并由 `ShellClipboardCommandsTest` 覆盖
- [x] 真机验证"被杀后自动复活"：独立 `nohup sh` 看门狗（pid 写 `/data/local/tmp/kc_watchdog.pid`）
  在 `am force-stop` 后持续存活，2 秒内即重新拉起 `BackgroundService`（app 主进程从 4887 变为 6483），
  30 秒后仍在运行。注意：Shizuku UserService 内做定时检测不可行——force-stop 时 Shizuku 取消
  UserService 的 binder 工作，检测恰好停止（死锁），故改用独立进程。依赖：UserService 需被
  首次创建（用户至少打开过一次 App 并授予 Shizuku 权限）；设备重启后 adb 版 Shizuku 需手动重新激活。
- [ ] root/Sui 模式在该设备上无法使用 `cmd clipboard` shell 接口；需实现并验证替代后端

## 参考

- 官方源码基线：KDE Connect 1.37.3（tag `baseline-1.37.3`）
- Shizuku API：https://github.com/RikkaApps/Shizuku-API

## 注意

- 仓库自带的 `debug.keystore` 是上游的，不能用于发布签名。
- 本目录不符合 `../AGENTS.md` 的 `<Target>_RE` 命名规范，是源码工作副本。
