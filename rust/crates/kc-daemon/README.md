# kc-daemon

Headless daemon and command-line client for the Rust KDE Connect rewrite. It
wires `kc-core`'s `DeviceManager` to a `kc-transport` link provider and the
`kc-plugins` registry, printing every host callback on stdout.

## Build

```sh
cargo build -p kc-daemon
```

Logging is via `tracing_subscriber`; set `RUST_LOG` (default `info`), e.g.
`RUST_LOG=debug kc-daemon serve`. Logs go to stderr, device/callback output to
stdout.

## Global options

| Option | Meaning |
| --- | --- |
| `--store <PATH>` | JSON store for identity and trusted devices. Default: `$XDG_CONFIG_HOME/kc-daemon/store.json` (or `~/.config/kc-daemon/store.json`). |
| `--device-name <NAME>` | Advertised name. Falls back to `KC_DEVICE_NAME`, then `KDE_CONNECT_DEVICE_NAME`, then `KDE Connect Rust`. |

Global options may appear before or after the subcommand.

## Commands

| Command | Description |
| --- | --- |
| `devices` | Print remembered/discovered devices: id, name, paired, reachable, loaded plugins. Exits immediately. |
| `serve [--no-broadcast]` | Run forever: load/generate identity, start the LAN link provider, pump `LinkEvent`s into `DeviceManager`, send queued `OutgoingPacket`s via `manager.send_packet`, print `DeviceEvent`s. Ctrl-C shuts down gracefully. `--no-broadcast` listens without sending UDP discovery broadcasts. |
| `pair <device_id>` | Request pairing with a remembered device. |
| `accept <device_id>` | Accept a pairing request. |
| `unpair <device_id>` | Remove a device from the trusted store. Works offline (falls back to a direct store removal). |
| `ping <device_id> [message]` | Send a `kdeconnect.ping` packet (built by `kc_plugins::ping::ping_packet`). |
| `clipboard set <text> [-d <device_id>]` | Set the local clipboard and send a `kdeconnect.clipboard` packet (built by `kc_plugins::clipboard::clipboard_packet`). With no `-d`, broadcasts to all remembered devices. |
| `find <device_id>` | Send a `kdeconnect.findmyphone.request` packet (ring a paired phone). |

## Architecture flow (`serve`)

```
FileStore (identity + trusted devices)
  -> DeviceManager::new(identity, store, CliPlatform, default_registry())
  -> manager.load_remembered_devices()
  -> DeviceInfo (device id, name, Desktop type, plugin capabilities)
  -> kc_transport::LanLinkProvider::new(identity, info, store, broadcast)
       |  start(link_tx)
       v
  link_rx -> manager.handle_link_event()      (connect / packet / disconnect)
  manager.subscribe() -> DeviceEvent printer
  manager.take_outgoing_receiver() -> manager.send_packet()
  tokio::signal::ctrl_c() -> provider.stop() + abort pumps
```

The identity is generated with `kc_protocol::identity::generate_device_id()` +
`kc_protocol::generate_identity()` on first run and persisted through
`kc_core::FileStore`. At startup the daemon installs rustls' `ring`
`CryptoProvider` explicitly, because transitive features otherwise leave the
process-level provider ambiguous and TLS setup panics.

## Known limitation: no IPC

`pair`, `accept`, `ping`, `clipboard` and `find` are separate CLI processes.
They read the same `FileStore` but do **not** share a running `kc-daemon`'s live
links or `DeviceManager`. Sending requires an active link, so those commands
will fail with "device is not reachable" unless invoked inside the serving
process. Use the `serve` process's stdout for interactive pairing (the
`[pairing]` callback prints the verification key); `unpair` and `devices` work
offline. Adding a local IPC/socket command channel would remove this limitation.
