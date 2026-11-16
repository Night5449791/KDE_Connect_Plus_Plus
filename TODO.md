# KDE Connect++ — TODO

## 当前状态

已复制的内容：

| 来源 | 内容 | 说明 |
|---|---|---|
| `invent.kde.org/network/kdeconnect-android` | 完整源码（不含 `.git`） | 官方 **1.37.3**（versionCode 13703） |
| `cagedbird043/kdeconnect-android` @ `cagedbird/plus` | `plus/` 目录 | Shizuku 剪贴板 + 自定义 IP 重连 |
| 同上 | `PLUS.md` | 上游维护规则 |
| 同上 | `.plus-patches/*.patch` | 5 个 plus 补丁（`git format-patch` 导出） |

**未完成**：plus 对官方文件的修改**还没有应用**到本目录的源码上。

## 为什么还没应用

本目录的官方源码是 **1.37.3**，而 plus 分支基于 **1.37.0**（提交 `46377f5d Release 1.37.0`）。

补丁不能直接 `git am`，因为：
- 本目录没有 `.git`（不是仓库）
- 1.37.0 → 1.37.3 之间上游改了部分同名文件，补丁上下文会不匹配

## 补丁清单

`.plus-patches/` 下 5 个补丁，按顺序：

| 文件 | 提交 | 内容 | 涉及文件数 |
|---|---|---|---|
| `0001-plus.patch` | `24a7665e` | 独立包名、版本号、发布签名 | 6 |
| `0002-plus-PLUS.md.patch` | `d80f51e8` | PLUS.md + `.omp/config.yml` + README | 3 |
| `0003-plus-Shizuku.patch` | `e9b88e46` | **Shizuku 后台剪贴板同步**（核心） | 24 |
| `0004-plus-APK.patch` | `840ca554` | GitHub Actions 自动跟进 + 发版 | 4 |
| `0005-plus-IP.patch` | `89d54b74` | 自定义 IP 自动重连 | 10 |

## 需要应用到官方源码的文件

补丁 `0003`、`0005` 会修改这些官方文件（**已存在于本目录**）：

```
build.gradle.kts
src/main/AndroidManifest.xml
src/main/java/org/kde/kdeconnect/plugins/clipboard/ClipboardListener.kt
src/main/java/org/kde/kdeconnect/plugins/clipboard/ClipboardPlugin.kt
src/main/java/org/kde/kdeconnect/plugins/notifications/NotificationsPlugin.kt
src/main/java/org/kde/kdeconnect/plugins/share/CompositeReceiveFileJob.kt
src/main/java/org/kde/kdeconnect/backends/lan/LanLink.kt
src/main/java/org/kde/kdeconnect/backends/lan/LanLinkProvider.kt
src/main/java/org/kde/kdeconnect/ui/compose/screen/pairing/PairingScreen.kt
fastlane/Appfile
```

plus 新增、**已手动补入**本目录的文件：

```
src/main/java/org/kde/kdeconnect/plugins/clipboard/ClipboardSettingsFragment.kt
src/main/res/values/plus_strings.xml
```

## TODO

- [ ] 决定基准版本：跟随 **1.37.3**（改补丁上下文）或回退到 **1.37.0**（直接 `git am`）
- [ ] 建议做法：`git init` 本目录，先提交官方 1.37.3 为 baseline，再逐个 `git apply -3` 补丁并解冲突
- [ ] `build.gradle.kts` 末尾加 `kdeconnect+` 块（包名 `top.miceworld.kdeconnectplus`、versionCode = 官方 × 100 + N）
- [ ] `AndroidManifest.xml` 加 ShizukuProvider、`overrideLibrary`
- [ ] 处理 `NotificationsPlugin.kt` 的包名硬编码（自身通知过滤）
- [ ] 处理 `CompositeReceiveFileJob.kt` 的 FileProvider authority（用 `BuildConfig.APPLICATION_ID`）
- [ ] 搭 Android 构建环境（**当前缺失**）：JDK 17/21、Android SDK（platform 37、build-tools）、Gradle
  - 现装 JDK 27，KDE Connect 要求 JDK 17 或 21，需另装
  - 磁盘可用 31G，SDK + Gradle 缓存约需 15–20G
- [ ] 构建验证：`./gradlew assembleDebug`
- [ ] 在真机（vivo V2445A / Android 15）验证 Shizuku 剪贴板双向自动同步
- [ ] 如需重命名自己的包名，改 `build.gradle.kts` 的 `applicationId` 与 `plus/` 下 Kotlin 包路径

## 参考

- 上游规则：`PLUS.md`
- Shizuku 剪贴板实现：`plus/shizuku-clipboard/`
- 自定义 IP 重连实现：`plus/reconnect/`
- 官方源码基线：KDE Connect 1.37.3

## 注意

- 本目录不符合 `../AGENTS.md` 的 `<Target>_RE` 命名规范与产物-only 定位，是按要求直接复制的源码工作副本。
- 官方 `debug.keystore` 是上游自带的，**不能**用于发布签名。
