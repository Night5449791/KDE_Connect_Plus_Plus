# Rust 重写架构与 Kotlin → Rust 映射

## 1. 总览

```
原 Kotlin 应用（已删除；此处仅保留映射记录作为协议基线）
   │
   ▼
kc-protocol  ──► kc-core ──► kc-transport ──► kc-daemon (CLI)
                  │              │
                  │              └──► LinkEvent::Connected/Packet/Disconnected
                  ▼
              kc-plugins (24 个插件，Platform trait 回调宿主)
                  │
                  ▼
        kc-android (JNI cdylib: 后台监督/事件循环/平台回调)
                  │  JNI 双向调用（见 docs/JNI_CONTRACT.md）
                  ▼
     android-shell (Kotlin: 纯 UI + 无状态 Android API 适配器)
```

数据流：

1. `kc-transport` 发现设备并完成 TLS 握手，发出 `LinkEvent::Connected { link, info, cert_der }`。
2. `DeviceManager::handle_link_event` 创建/更新 `Device`，按能力匹配插件。
3. 链路收到 JSON 行 → `NetworkPacket::deserialize` → 若有 payload，传输层建立第二连接读满 `payloadSize` 字节 → `LinkEvent::Packet`。
4. `Device::on_packet` 先处理 `kdeconnect.pair`，其余按 `PluginRegistry::handlers_for(type)` 分发。
5. 插件通过 `PluginContext::sender` 把回复投递到 `DeviceManager::take_outgoing_receiver()`，由 daemon/JNI 事件循环调用 `manager.send_packet()` 发出。

## 2. 类映射

| Kotlin 类 | Rust 类型 | 状态 |
|---|---|---|
| `NetworkPacket` | `kc_protocol::NetworkPacket` | 完整（含 payload 抽象） |
| `DeviceInfo` | `kc_protocol::DeviceInfo` | 完整 |
| `PairingHandler` | `kc_protocol::PairingHandler` | 完整（v7/v8 校验码、时钟偏差、状态机） |
| `RsaHelper` / `SslHelper` | `kc_protocol::{generate_identity, TlsContext}` | 完整（EC P-256、TLS 1.2、pinning） |
| `Plugin` / `PluginFactory` | `kc_core::{Plugin, PluginRegistry}` | 完整（能力并集/交集） |
| `Device` | `kc_core::Device` | 完整（link 优先级、插件重载、配对回调） |
| `KdeConnect` | `kc_core::DeviceManager` | 完整 |
| `BaseLink` / `BaseLinkProvider` | `kc_core::{Link, LinkProvider}` | 接口完整 |
| `LanLinkProvider` / `LanLink` | `kc_transport::LanLinkProvider` | 完整 |
| `MdnsDiscovery` | `kc_transport` 内部 | 基础实现 |
| `BluetoothLink*` | — | 未移植 |
| `TrustedDevices` / SharedPreferences | `kc_core::{FileStore, MemoryStore}` | 完整 |
| `plus/**`（Shizuku） | — | 未移植 |
| 前台服务轮询/设备状态管理 | `kc_android::supervisor` | Rust 拥有；Kotlin 无轮询线程 |
| 各 `plugins/**/*.kt` | `kc_plugins::*` | 24/24 有实现与包体测试 |

## 3. 插件与包类型表

| 插件 | 接收类型 | 发送类型 |
|---|---|---|
| BatteryPlugin | `kdeconnect.battery` | `kdeconnect.battery` |
| ClipboardPlugin | `kdeconnect.clipboard`, `.clipboard.connect` | 同左 |
| ConnectivityReportPlugin | `kdeconnect.connectivity_report` | 同左 |
| ContactsPlugin | `.contacts.request_all_uids_timestamps`, `.contacts.request_vcards_by_uid` | `.contacts.response_uids_timestamps`, `.contacts.response_vcards` |
| DigitizerPlugin | — | `.digitizer.session`, `.digitizer` |
| FindMyPhonePlugin | `.findmyphone.request` | — |
| FindRemoteDevicePlugin | — | `.findmyphone.request` |
| InputDevicesReceiver | `.mousepad.request`, `.shareinputdevices.request` | `.shareinputdevices` |
| MouseReceiverPlugin | `.mousepad.request` | — |
| MousePadPlugin | `.mousepad.keyboardstate` | `.mousepad.request` |
| MprisPlugin | `.mpris` | `.mpris.request` |
| MprisReceiverPlugin | `.mpris.request` | `.mpris` |
| NotificationsPlugin | `.notification.request`, `.reply`, `.action` | `.notification` |
| PingPlugin | `.ping` | `.ping` |
| PresenterPlugin | — | `.mousepad.request`, `.presenter` |
| ReceiveNotificationsPlugin | `.notification` | `.notification.request` |
| RemoteKeyboardPlugin | `.mousepad.request` | `.mousepad.echo`, `.mousepad.keyboardstate` |
| RemoteKeyboardIMEPlugin | `.mousepad.request` | `.mousepad.echo`, `.mousepad.keyboardstate` |
| RunCommandPlugin | `.runcommand`, `.runcommand.output` | `.runcommand.request` |
| SftpPlugin | `.sftp.request` | `.sftp` |
| SharePlugin | `.share.request`, `.share.request.update` | `.share.request` |
| SMSPlugin | `.sms.request`, `.request_conversations`, `.request_conversation`, `.request_attachment` | `.sms.messages`, `.sms.attachment_file` |
| SystemVolumePlugin | `.systemvolume` | `.systemvolume.request` |
| TelephonyPlugin | `.telephony.request_mute` | `.telephony` |

（表中前缀 `kdeconnect` 省略处均为 `kdeconnect.*`。）

## 4. 配对流程

1. `Device::request_pairing()` → `{type:"kdeconnect.pair", body:{pair:true,timestamp:<秒>}}`，状态 `NotPaired → Requested`，30s 超时（超时任务由宿主调度，Rust 层暴露状态）。
2. 对端 `on_packet` 校验 timestamp（±1800s），计算校验码，发 `PairingEvent::PairingRequested`，状态 `RequestedByPeer`。
3. 用户接受 → `accept_pairing()` 发 `pair:true` 并进入 `Paired`；`on_paired()` 把对端证书写入 store（TOFU）并重载插件。
4. 收到 `pair:false` → `Unpaired` 事件 + 清除信任数据。
5. 校验码：`SHA256(sorted_concat(myPub, peerPub) || timestampDec)` 的 hex 前 8 位大写。

## 5. 测试与验证边界

- `cargo test --workspace` 覆盖协议序列化、证书/TLS 配置、配对状态机、24 个插件的包体构造/解析。
- CI 包含 `cargo-ndk` 交叉编译，验证 `kc-android` 可产出 Android `.so`。
- **未验证**：真实设备发现/配对互通、payload 大文件传输、蓝牙、Shizuku 相关能力、Android 壳真机运行。这些都是静态实现 + 单元测试级别。
