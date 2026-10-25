# foto: implementation spec

Source: Vellum note "🖼️ foto.md" (personal vault). That note was used as a reference, not a spec.
This document is the spec for a single build pass, split into commits.

`foto` is a **working name** (see "Naming").

## Goal

A local-first photo and video library. Phones are the main source. Every device syncs directly with
the others over Iroh, with no cloud storage, accounts or servers to run. One shared Rust sync core
runs under three native shells:

| Shell | Stack | Role |
|-------|-------|------|
| Desktop (macOS first, Linux second) | GPUI + gpui-kit | Full library, archive-capable, watched folders |
| Android (essential) | Kotlin + Jetpack Compose | Main photo source, browses the whole library |
| iOS (wanted) | Swift + SwiftUI | Photo source, browses the whole library |

## Changes from the design note

| # | Design note says | Problem | Spec |
|---|------------------|---------|------|
| 1 | Iroh 0.35, Rust 1.75 | Out of date | `iroh 1.4`, `iroh-blobs 0.103`, `iroh-docs 0.101`, Rust 1.95, edition 2024 (same stack as Vellum) |
| 2 | `exifr` for EXIF | No such Rust crate | `nom-exif` (JPEG/HEIC/MOV/MP4 metadata) |
| 3 | `ed25519-dalek` for device identity | Redundant | The persisted iroh `SecretKey` is the device identity |
| 4 | `iroh-gossip` for LAN discovery | Gossip doesn't find peers | `iroh-mdns-address-lookup` for LAN, n0 DNS plus relays for internet. iroh-docs uses gossip internally for live updates |
| 5 | Hand-rolled "sync state per peer" | Would mean reinventing reconciliation, tombstones and resume | Index = **iroh-docs replica**. Content = **iroh-blobs** (verified, resumable) |
| 6 | `sqlx` + SQLite | Heavier than needed | `rusqlite` (bundled) as a **derived** local index that can always be rebuilt |
| 7 | `embed-resource` | That crate compiles Windows `.rc` files, it doesn't embed assets | `include_bytes!` / `gpui-kit-assets` |
| 8 | Pairing by QR code on desktop | Desktops can't scan a QR code | Desktop *shows* the QR code and a copyable ticket. Phones *scan* it |
| 9 | Unspecified original storage | A copy-in store doubles disk use | Originals are plain files that the blob store only references (see "Storage") |
| 10 | `image` crate for thumbnails | No HEIC/RAW support, and it's slow at scale | Thumbnails are made **once, by the importing device, with that platform's decoders**, then synced as JPEG blobs. Other devices never decode foreign formats |
| 11 | Unspecified deletes | Deleted photos would come back on the next sync | Tombstones plus a 30-day trash (see "Deletes") |
| 12 | Originals sync first | Browsing a new device would wait on gigabytes | Thumbnails sync first, originals after them |
| 13 | Mobile shells optional | Phones are the main photo source | Android is required, iOS is wanted. Shared core over UniFFI |
| 14 | 15–25 MB binary, <200 ms start | Unmeasured | Release profile: LTO, `strip`, `opt-level=3`, `codegen-units=1`. Measure and report. These are soft goals |

## Decisions

| Topic | Decision |
|-------|----------|
| UI | GPUI desktop, Kotlin/Compose Android, Swift/SwiftUI iOS. Shared Rust core via UniFFI |
| Platforms | macOS first, then Linux, Android, iOS |
| Storage | Visible library folder, with no second copy of any file |
| Formats | Photos (JPEG/PNG/WebP/GIF/TIFF/HEIC/AVIF), RAW (embedded preview), video (poster frame + duration). Android Motion Photos and iPhone Live Photos are each one item |
| Deletes | A delete in foto removes the item on every device. 30-day recoverable trash. A file deleted from a watched folder is deleted everywhere. A missing root never counts as a delete |
| Archive device | Per-device switch. That device ignores deletes and keeps the file |
| Edits | A changed file replaces the old asset (new hash, old one tombstoned) |
| Library folder layout | `<library>/YYYY/MM/<name>`. On a name clash, a short hash is appended |
| Local files | Watched folders stay where they are. The library folder only holds files received from peers |
| Sync scope | Per-device originals policy: `All` (desktop default) or `OnDemand` (phone default) |
| Metered networks | Originals download only on unmetered networks (toggle). Index and thumbnails sync on any network |
| Disk full | Pause downloads, keep 2 GB headroom, show a warning |
| Sharing | Read-only tickets for other people, with minimal UI |
| Revocation | Future work: [#1](https://github.com/andymitch/foto/issues/1). "Remove device" is a soft action |
| Desktop background | Tray/menu bar, launch at login |
| Video playback | Hand-off to the system player |
| Organisation | Timeline + favourites + recently deleted. No albums or search |
| Lightbox | Zoom/pan, arrow keys/swipe, info panel, reveal in file manager. No editing |
| Distribution | Unsigned desktop builds, n0 public relays |
| Libraries | Core supports N libraries. UI starts with one and shows a switcher once there are two |
| Import | Watched folders with live events plus a rescan at startup (desktop). MediaStore/PhotoKit observers (mobile) |

## Repository layout

```
foto/
├── Cargo.toml                 workspace, shared deps, release profile
├── crates/
│   ├── foto-core/             all logic. No UI, no platform SDKs
│   ├── foto-ffi/              UniFFI surface (cdylib for Android, staticlib for iOS)
│   ├── foto-desktop/          GPUI app + desktop Platform impl (ImageIO/AVFoundation on macOS)
│   └── foto-cli/              headless `foto` binary for scripting, tests and stress runs
├── android/                   Gradle project (Kotlin, Compose). Consumes foto-ffi via cargo-ndk
├── ios/                       XcodeGen `project.yml` + SwiftUI sources. Consumes an XCFramework
├── scripts/                   build-android.sh, build-ios.sh, bindgen.sh, two-node.sh
└── .github/workflows/         ci.yml, release.yml
```

## Core (`foto-core`)

### Runtime and API shape

- `Foto::open(Config) -> Foto` starts a tokio multi-thread runtime that the core owns. Every
  public method is a cheap handle call that is safe from any thread, so GPUI, the Android main
  thread and Swift actors never touch tokio.
- `Config { data_dir, library_dir, device_name, platform: Arc<dyn Platform>, policy }`.
- Events go out through a single `subscribe(Arc<dyn EventSink>)` callback. UniFFI exports it as a
  callback interface. The desktop bridges it into a GPUI channel.
- `--data-dir` / `FOTO_HOME` lets several instances run on one machine.

### Modules

| Module | Responsibility |
|--------|----------------|
| `node` | Endpoint (persisted secret key), Router (blobs, docs, gossip), mDNS, network-change hook (`network_changed()` for mobile, as Vellum does) |
| `library` | Namespaces (N libraries), create/join/share (read-write or read-only tickets), entry codecs, doc event subscription |
| `index` | rusqlite timeline and local state. Rebuilt from the replica plus a rescan |
| `ingest` | Takes candidate files from watchers/observers. Size+mtime fast path, BLAKE3 import (`TryReference`), metadata, thumbnail jobs, pairing of Live/Motion photos |
| `media` | Metadata via `nom-exif`. Thumbnail generation calls `Platform::thumbnail`, with an `image`-crate fallback for common formats |
| `watch` | Desktop: `notify` + `notify-debouncer-full`, startup rescan, offline-root detection. Skips `.foto-trash` |
| `sync` | Doc live sync, prioritised blob fetch queue, export to library folder, holdings, policies, disk/metered gates, per-peer status |
| `trash` | Tombstones, 30-day retention, restore, archive behaviour, purge |

### Platform trait (implemented by each shell)

```rust
trait Platform: Send + Sync {
    fn thumbnail(&self, path: &str, max_px: u32) -> Option<Vec<u8>>;
    fn poster(&self, path: &str, max_px: u32) -> Option<Vec<u8>>;
    fn trash(&self, paths: Vec<String>) -> TrashOutcome;
    fn restore(&self, paths: Vec<String>) -> bool;
    fn is_metered(&self) -> bool;
    fn free_space(&self, path: &str) -> u64;
}
```

`TrashOutcome::Deferred` covers mobile, where the OS needs user confirmation. The core keeps a
pending set and the shell asks the user in one batch the next time the app is in the foreground.

Implementations:
- macOS: ImageIO (HEIC/RAW/all stills) and AVAssetImageGenerator (posters) via `objc2-*`.
- Linux: `image` crate, embedded EXIF/RAW previews, `ffmpeg` on PATH for posters (placeholder
  if it's missing).
- Android: `ImageDecoder` / `MediaMetadataRetriever`.
- iOS: ImageIO / AVFoundation.

Thumbnails are JPEG q80 with orientation applied: **S** has a 360 px long edge (grid), **L** a
1600 px long edge (lightbox / mobile viewer).

## Storage

- The blob store (`<data_dir>/blobs`) holds outboards, metadata, thumbnail blobs (small, inline)
  and partial downloads.
- **Local originals** are imported with `ImportMode::TryReference`: the store references the file
  where it is.
- **Received originals** download into the store, so they are verified and resumable. Then they
  are exported with `ExportMode::TryReference` into `<library>/YYYY/MM/<name>`. That export is a
  rename plus an external reference (verified in iroh-blobs 0.103 `store/fs.rs::export_file`).
  Across volumes it copies and then deletes, so disk use is still 1x once it finishes.
- iroh-blobs GC never deletes external files (`meta.rs`). foto only deletes user files through
  the trash flow.
- If a referenced file is edited in place, receivers reject the mismatched bytes (bao
  verification) and the watcher turns the change into an edit.
- **OnDemand devices** (phones) keep originals they fetched in a store cache capped at 2 GB
  (LRU). Those are not exported. On Android they're shown as "Save to device".
- **iOS** can't reference PhotoKit files. Swift exports each asset into `<data_dir>/staging`,
  and the core imports it by reference. Once a non-iOS device has a holdings entry for that hash,
  the staged copy is evicted (thumbnails stay). The index keeps `localIdentifier ↔ hash` so an
  asset is never exported twice.
- **Android** reads media paths directly (`READ_MEDIA_*`, and `ACCESS_MEDIA_LOCATION` for
  unredacted GPS) and imports by reference. Kotlin enumerates the selected MediaStore buckets and
  sends candidates to the core. A ContentObserver triggers incremental rescans.

## Data model (one iroh-docs namespace per library)

Values are `postcard`-encoded structs with a `v` field for format evolution. Each device writes
with its own author key. Readers take the latest entry per key.

| Key | Written by | Value |
|-----|-----------|-------|
| `meta` | creator | `{ name, created_at }` |
| `a/<hash>` | importing device | `{ kind, taken_at, tz_offset, mime, w, h, duration_ms?, size, name, thumb_s, thumb_l, pair?, motion, origin, added_at }` |
| `f/<hash>` | any | `{ favorite: bool, at }` |
| `x/<hash>` | any | `{ deleted_at, by, restored_at? }` (a restore clears it by setting `restored_at`) |
| `h/<endpoint>/<hash>` | that device | empty. "This device has the complete original" |
| `d/<endpoint>` | that device | `{ name, platform, policy: All\|OnDemand, archive, last_seen }` |

- Assets are keyed by content hash, so duplicates across folders and devices are one asset.
- Live Photos: the HEIC/JPEG is the asset and `pair` is the MOV hash (a second blob). On desktop,
  folder pairs are matched by ContentIdentifier, falling back to same stem and same directory.
  Motion Photos are one file with `motion = true`.
- Local paths, roots and staging state are **device-local** (index only). They are never synced.
- Read-only members don't write `h/`, `d/`, `f/` or `x/`. They just consume.

## Index (rusqlite, `<data_dir>/index.db`, WAL)

```
assets(hash PK, kind, taken_at, tz_offset, w, h, duration_ms, size, name, mime,
       thumb_s, thumb_l, pair, motion, favorite, deleted_at, has_thumb_s, has_original)
paths(path PK, hash, root_id, size, mtime)
roots(id PK, path, kind, online)
ext_ids(platform_id PK, hash)
holdings(endpoint, hash, PK(endpoint, hash))
pending_trash(path PK, hash, reason)
```

The timeline is a keyset page on `(taken_at DESC, hash)` that skips rows where `deleted_at` is
set. A month histogram query backs the scrubber and month headers. If the schema version
changes, the index is dropped and rebuilt instead of migrated.

## Sync

1. **Index:** iroh-docs live sync with every known peer (`d/` entries plus ticket nodes). mDNS
   on LAN, n0 discovery and relays elsewhere. Doc events update the index and wake the fetch queue.
2. **Fetch queue** (persistent, rebuilt from the index on start), in priority order: thumb S, then
   thumb L, then originals. Originals are only fetched when policy is `All`, the network is
   unmetered (or the toggle allows it) and free space is above 2 GB. Providers are the devices
   with `h/` entries for that hash, plus the origin device. Downloads use the iroh-blobs
   downloader with multiple providers, and partial blobs resume after restart.
3. **On complete:** export to the library folder, write `h/me/<hash>`, emit an event.
4. **On demand:** opening an original that isn't present fetches it at top priority.
5. **Per-peer status:** online state, last sync time, originals held vs total, and bytes we still
   need from that peer.

## Deletes, edits, restore

- **Delete** (UI or a file removed from a watched root) writes `x/<hash>`. Every non-archive
  device then trashes its local file(s):
  - Desktop moves them into `<root>/.foto-trash/` (same volume, so it's a rename), and moves
    them to the OS trash after 30 days.
  - Android uses `MediaStore.createTrashRequest`, which is native 30-day trash, batched
    confirmation.
  - iOS uses `PHAssetChangeRequest.deleteAssets`, which goes to Recently Deleted, batched
    confirmation.
- **Restore** within 30 days sets `restored_at`. Devices move the files back out of
  `.foto-trash`. On mobile the native trash restore is used where it's available, and otherwise
  the original is refetched from an archive or desktop device.
- **Archive devices** keep the file and don't trash it. They still hide the asset.
- **Tombstones** are kept forever (tens of bytes each), so late joiners never resurrect assets.
- **Edit** (same path, new content) imports the new hash and tombstones the old hash in one
  batch.
- **A root that goes offline** marks its paths unavailable and never deletes anything.

## Desktop app (`foto-desktop`, GPUI + gpui-kit)

- **Layout:** sidebar (Timeline, Favourites, Recently deleted, Folders, Devices, Settings) plus
  the main view. A library switcher shows up once there are two libraries.
- **Timeline:** `uniform_list` of rows. Each row holds N square tiles, with N computed from the
  width. Month header rows and a month scrubber. Thumbnails are decoded from S blobs into a
  byte-capped LRU (64 MB). Badges for video duration, favourite, Live/Motion, and "original not
  local".
- **Lightbox:** L thumbnail immediately, then the full original (macOS uses ImageIO for HEIC/RAW).
  Zoom/pan, ←/→ to move, `i` for the info panel (date, dimensions, size, camera, source device,
  which devices hold it), favourite, delete, reveal (`open -R` / `xdg-open` on the parent
  directory), and play video in the system player.
- **Devices:** this device's name and policy, pairing (QR + copy text, read-only toggle), join
  (paste a ticket), and a peer list with status and per-device policy/archive display.
- **Folders:** add/remove watched folders, online state, import progress.
- **Settings:** library folder location, device name, metered toggle (shown on mobile only),
  launch at login.
- **Tray:** `tray-icon` + `muda` (status, open, pause sync, quit). Closing the window keeps foto
  running. Launch at login via `auto-launch`.
- **Async bridge:** core events arrive on a channel, a GPUI foreground task drains it and calls
  `cx.notify()`.

## Android app (`android/`)

- `minSdk 30` (Android 11, needed for the trash request), latest `targetSdk`, Compose Material 3,
  Material You colours.
- Rust built with `cargo-ndk` (arm64-v8a, x86_64 for the emulator). Kotlin bindings come from
  `uniffi-bindgen` in Gradle. JNI context init and network/lifecycle bridges follow Vellum's
  proven setup (`ndk-context`, app class loader caching, R8 keep rules).
- **Screens:**
  - Timeline: `LazyVerticalGrid` with month headers. Coil with a custom Fetcher that reads
    thumbnail bytes from the core.
  - Viewer: `HorizontalPager`, pinch zoom, info sheet, "Save to device" for on-demand originals,
    video hand-off via intent.
  - Favourites and Recently deleted.
  - Devices: pairing by scanning the QR code (CameraX + ML Kit bundled barcode model) or pasting
    a ticket.
  - Folders: MediaStore bucket picker, Camera by default.
  - Settings.
- **Permissions:** `READ_MEDIA_IMAGES`, `READ_MEDIA_VIDEO`, `ACCESS_MEDIA_LOCATION`, `CAMERA`,
  `POST_NOTIFICATIONS`, `FOREGROUND_SERVICE_DATA_SYNC`, `ACCESS_NETWORK_STATE`,
  `CHANGE_WIFI_MULTICAST_STATE` (a MulticastLock is held while foregrounded, for mDNS). Partial
  photo access gets a prompt asking for full access.
- **Background:** WorkManager periodic job (15 min, network-constrained), plus a `dataSync`
  foreground service with a progress notification only while transfers are active.
- **Deletes:** a pending trash batch is confirmed with one system dialog on the next foreground.

## iOS app (`ios/`)

- iOS 17+, SwiftUI. The XCFramework is built by `scripts/build-ios.sh` (device + simulator) and
  Swift bindings come from `uniffi-bindgen`. The project is generated by XcodeGen, so no
  `.pbxproj` churn.
- PhotoKit full access, `PHPhotoLibraryChangeObserver`, `PHAssetResourceManager` export into
  staging, eviction once a non-iOS device holds the asset.
- Screens mirror Android. QR scanning via VisionKit `DataScannerViewController`. "Save to Photos"
  for received originals.
- Background: `BGAppRefreshTask` + `BGProcessingTask`. Sync mainly happens while the app is open.
- LAN discovery: iroh's mDNS needs Apple's multicast entitlement (paid account plus approval).
  Without it, iOS falls back to n0 DNS discovery and relays, which still upgrade to direct LAN
  connections by hole-punching. `NSLocalNetworkUsageDescription` is still declared.

## Testing

- **Core unit tests:** codecs, index queries, path layout and clash naming, pairing heuristics.
- **Core integration tests** (in-process, tempdir nodes, like Vellum):
  - two nodes converge
  - thumbnails arrive before originals
  - resume after a node restart mid-transfer
  - delete propagates and the archive device keeps its file
  - restore
  - a rename doesn't re-transfer
  - an edit replaces the asset
  - an offline root doesn't delete
  - disk-full pause (fake `free_space`)
  - an OnDemand node fetches on open
  - a read-only member can't write
  - iOS-style eviction once holdings exist
- **CLI:** `foto --data-dir X {init,join,share,watch add,scan,ls,status,stress N}`.
  `scripts/two-node.sh` runs two local instances end to end. `stress 100000` writes synthetic
  entries to check index and timeline performance.
- **Desktop:** GPUI `TestAppContext` for view logic, manual pass on macOS.
- **Android:** `assembleDebug` in CI, manual pass on a real device.
- **iOS:** build in CI. A manual pass depends on the questions below.

## CI (`.github/workflows/ci.yml`)

- Linux runner: fmt, clippy `-D warnings`, test, Android `assembleDebug`.
- macOS runner (10x minutes on private repos): desktop build + test and the iOS XCFramework +
  `xcodebuild`, only on pushes to `main`, not on every PR push.
- `release.yml` (tag): cargo-packager `.dmg`, `.deb`/AppImage, plus an Android APK as a GitHub
  Release.

## Commit plan

1. Workspace, release profile, CI skeleton, README.
2. `foto-core`: node, library, data model codecs, index.
3. `foto-core`: ingest, media, watch, trash. Desktop `Platform` impls.
4. `foto-core`: sync engine (queue, export, holdings, policies, gates, status).
5. `foto-cli` + integration test suite + stress.
6. `foto-ffi` (UniFFI) + bindgen scripts.
7. `foto-desktop`: shell, timeline, lightbox, devices/pairing, folders, settings, tray.
8. `android/` app.
9. `ios/` app.
10. Packaging + release workflow. Measure binary size, cold start and idle memory, and report
    them in README.

## Naming

Not final. `foto` is used for crate names, the binary, `<library>` (`~/Pictures/foto`) and app IDs
(`com.andymitch.foto`). The name lives in one constant per platform, so a rename is mechanical.

## Success metrics

- Import 1000+ photos and videos
- 60 fps timeline scroll on desktop and Android
- A phone pairs with the desktop by scanning a QR code
- 50 GB in under 2 h on gigabit LAN
- 100K-asset library stays responsive (stress test)
- No crashes in the integration suite or a soak run
- Binary size, cold start and idle memory measured against 15–25 MB, 200 ms and 150 MB (soft goals)
