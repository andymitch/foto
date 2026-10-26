# foto

A local-first, peer-to-peer photo and video library. Your photos stay on your own devices and sync
directly between them over [Iroh](https://iroh.computer), with no cloud storage, accounts or
servers. Works on macOS, Linux and Android.

- Watched folders are imported where they already are. Nothing gets copied into a hidden store.
- Photos from your other devices land in a normal folder (`~/Pictures/foto/<library>/YYYY/MM`),
  stored once.
- Previews sync first, so a new device can browse the whole library in seconds. Originals follow:
  every original on computers, on demand on phones.
- Deletes reach every device and can be undone for 30 days. An "archive" device keeps everything.

Design and decisions: [docs/PLAN.md](docs/PLAN.md). iOS is planned: [#2](https://github.com/andymitch/foto/issues/2).

## Releases

Cut from GitHub Actions (**Actions → Release → Run workflow**), the same way as Vellum and Pulse:

- `major` makes the next `vN`.
- `hotfix` makes `vN.X` off the latest major.
- `beta` makes `vN-beta.X`, a pre-release of the next major.

The workflow creates a draft and attaches `foto_aarch64.dmg`, `foto_amd64.deb`,
`foto_x86_64.AppImage` and `foto-release.apk` (signed with the release keystore in the
repo secrets). It publishes once every build succeeds. Notes are grouped by the label of the
issue each change closes, so label issues and write `Closes #N`.

## Layout

| Path | What |
|------|------|
| `crates/foto-core` | Everything that isn't UI: iroh node, library replica (iroh-docs), blobs, SQLite index, watcher, import, sync, trash |
| `crates/foto-ffi` | UniFFI cdylib/staticlib for the mobile apps |
| `crates/foto-desktop` | GPUI desktop app (macOS: ImageIO/AVFoundation thumbnails, menu-bar tray) |
| `crates/foto-cli` | Headless `foto` binary |
| `android/` | Kotlin + Jetpack Compose app |

## Build and run

```sh
cargo run -p foto-desktop            # desktop app
cargo run -p foto-cli -- status      # headless
cargo test --workspace               # unit + multi-node sync tests
./scripts/two-node.sh                # two CLI nodes on this machine, end to end
cargo packager -p foto-desktop --release   # .app/.dmg (macOS) or .deb/.AppImage (Linux)
cd android && ./gradlew installDebug # needs the Android SDK/NDK, cargo-ndk and JDK 17+
```

`FOTO_HOME` (data dir) and `FOTO_PICTURES` (library folder) override the defaults, so several
instances can run on one machine.

## Pairing

1. Desktop: create a library, add folders.
2. Desktop: **Devices → Invite one of my devices** shows a QR code and a ticket.
3. Phone: scan it. Another computer: paste the ticket.

Devices find each other on the LAN (mDNS) or anywhere through n0's public relays.

## Measured (M-series Mac, release build)

| | |
|-|-|
| Desktop binary (stripped, LTO) | 31.8 MB, `.dmg` 15 MB |
| Core startup (open node + library) | ~50 ms |
| Desktop idle memory (62-item library, grid shown) | 115 MB footprint |
| Timeline query, 100K assets | 49 ms (`cargo test -p foto-core --release -- --ignored`) |
