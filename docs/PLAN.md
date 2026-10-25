# foto — implementation plan

Source: Vellum note "🖼️ foto.md" (personal vault). That note is a reference, not a spec. This
document records where the plan departs from it and why.

## Goal

A local-first photo library that runs as a desktop app and syncs directly between your own devices
over Iroh, with no cloud storage, accounts or servers to run.

## Changes from the design note

| # | Design note says | Problem | Plan |
|---|------------------|---------|------|
| 1 | Iroh 0.35, Rust 1.75 | Out of date. Iroh is at 1.x, the toolchain at 1.95 | `iroh 1.4`, `iroh-blobs 0.103`, `iroh-docs 0.101`, edition 2024 (same stack as Vellum) |
| 2 | `exifr` for EXIF | No such Rust crate (exifr is a JS library) | `nom-exif`, which also reads HEIC/MOV metadata |
| 3 | `ed25519-dalek` for device identity | Not needed. An iroh `SecretKey` already is an ed25519 device key | Persist the endpoint secret key and use it as the device ID |
| 4 | `iroh-gossip` for LAN discovery | Gossip broadcasts messages and doesn't find peers | `iroh-mdns-address-lookup` for LAN, n0 DNS plus relays for internet. Gossip is still used, internally by iroh-docs for live updates |
| 5 | "Sync full library" plus "sync state per peer" | No index replication model is defined. Building one by hand means writing set reconciliation, tombstones and resume yourself | The library index is an **iroh-docs replica**: one entry per photo, with range-based set reconciliation and multi-writer support built in. Content (originals and thumbnails) is **iroh-blobs**, which already resumes and verifies with BLAKE3. Vellum already uses this pattern |
| 6 | `sqlx` + SQLite as the database | sqlx's async and compile-time macros add a lot for an embedded single-user DB | `rusqlite` (bundled) as a **derived local index** for timeline queries. It can be rebuilt from the doc replica, so it doesn't need migrations to keep data safe |
| 7 | `embed-resource` to bundle assets | That crate compiles Windows `.rc` resources. It doesn't embed assets | `include_bytes!` / `rust-embed`. `gpui-kit-assets` already ships icons and fonts |
| 8 | Pair via QR code (desktop) | Desktops usually can't scan a QR code | Pairing ticket offered as **copyable text and a QR code**. QR becomes useful once a mobile shell exists |
| 9 | Not specified: where originals live | A copy-in store doubles disk use for 50 GB+ libraries | Every original is a plain file in a folder you can see, and the blob store only *references* it. See "Storage" below |
| 10 | `image` 0.25 for thumbnails | Can't read HEIC or RAW, and full decodes of 100K photos are slow | MVP supports JPEG/PNG/WebP/GIF/TIFF. Use the embedded EXIF preview when there is one, otherwise decode and resize with `fast_image_resize`, on a bounded blocking pool. HEIC comes later via `libheif-rs` or the platform's image APIs |
| 11 | Not specified: deletes | Without tombstones, a deleted photo comes back on the next sync | A delete writes a tombstone entry to the doc. The blob is garbage collected once no live entry references it |
| 12 | Peers fetch originals before anything else | Browsing a new peer's library would wait on gigabytes | Thumbnails sync first (small blobs), originals afterwards in the background, so the timeline fills in quickly |
| 13 | ~500 LOC core, ~1000 LOC UI | Unrealistic. The import, sync, thumbnail and timeline paths each need more than that | Milestones below. LOC is not a target |
| 14 | 15–25 MB binary | gpui plus iroh plus bundled SQLite will probably go over that | Measure at M1. Use `opt-level="z"`, LTO, `strip`, `panic=abort` if size matters. This is a soft goal |
| 15 | `photosync/` naming | — | Project, crates and binary are named `foto` |

Async integration: GPUI has its own executor and iroh needs tokio. The core owns a tokio runtime
on its own threads and exposes a handle. The UI sends commands and gets events back over channels,
so the UI never touches tokio directly.

## Decisions

| Topic | Decision |
|-------|----------|
| Storage | Visible library folder, with no second copy of any file (see "Storage") |
| Videos | Imported and synced alongside photos |
| Libraries | Core supports N libraries from day one. The UI starts with one default library and shows a switcher once there are two |
| Import | Watched folders: live file events, plus a full rescan at startup |

## Storage

The blob store (`<data-dir>/blobs`) holds only outboards (BLAKE3 tree hashes), metadata and the
small thumbnail blobs. Originals always stay as ordinary files:

- **Local files in watched folders** are imported with `ImportMode::TryReference`. The store
  points at the file where it already is.
- **Files received from a peer** download into the store first, so transfers are verified and can
  resume. Then they are exported with `ExportMode::TryReference` into the **library folder**
  (default `~/Pictures/foto`). In iroh-blobs 0.103 that export is a `rename` of the store's file
  followed by an external reference, so nothing is copied. I checked this in
  `store/fs.rs::export_file`.
- The store's data dir must be on the same volume as the library folder so the rename stays
  cheap. Across volumes, iroh-blobs falls back to copying and then deletes its own copy, so disk
  use is still 1x once the move finishes.
- iroh-blobs GC never deletes externally referenced files (`meta.rs`: `DataLocation::External`
  is skipped on delete). That means blob GC can't destroy user files. Deleting user files is
  always an explicit foto action.
- The library folder is itself a watched folder, so photos you drop into it are imported.

Risk: if a referenced file is edited in place, the store would serve bytes that don't match the
hash. Receivers verify with BLAKE3/bao and reject them, so nothing is corrupted. The watcher
catches the change, treats it as an edit and drops the old reference.

## Data model

One iroh-docs namespace per library. Each device signs with its own author key.

| Key | Value |
|-----|-------|
| `a/<blake3>` | Asset: `{ kind: photo\|video, taken_at, tz_offset, mime, w, h, duration?, size, name, rel_dir, thumb_s, thumb_l, added_by, added_at, favorite? }` |
| `x/<blake3>` | Tombstone: `{ deleted_at, by }` |
| `d/<endpoint-id>` | Device: `{ name, last_seen }` |
| `meta` | Library name |

- Assets are keyed by content hash, so the same bytes in two folders or on two devices are one
  asset.
- Local paths are **not** synced. Each device maps hash to its own local paths in its rusqlite
  index. A rename or move inside a watched folder only updates the local index and never
  re-transfers anything.
- The local rusqlite index holds the timeline (`taken_at`, dims, kind, thumb hashes, local paths,
  original present yes/no, tombstoned yes/no). It is rebuilt from the replica plus a folder scan,
  so it is never the source of truth.

## Watching

- The `notify` crate (FSEvents/inotify/ReadDirectoryChangesW) watches each folder, with events
  debounced.
- On startup each watched folder gets a full rescan. Files whose `(path, size, mtime)` matches the
  index are skipped. Only new or changed files get hashed.
- A watched root that is missing (unmounted drive, offline share) is marked *offline*, and its
  files are **never** treated as deleted.
- Hashing, EXIF and thumbnail work runs on a bounded blocking pool with progress events to the UI.

## Architecture

```
foto/
├── Cargo.toml              workspace
├── crates/foto-core/       no UI. Library, import, thumbs, sync, index
│   ├── node.rs             iroh endpoint + router (blobs, docs, gossip), mDNS, key persistence
│   ├── library.rs          replica: assets, tombstones, devices, tickets. N libraries
│   ├── watch.rs            notify + startup rescan, offline-root detection
│   ├── import.rs           hash, metadata, thumbnail jobs
│   ├── media.rs            decode backends: image crate, HEIC, video poster frames
│   ├── index.rs            rusqlite timeline + local path map
│   └── sync.rs             fetch queue (thumbs, then originals), export to library folder, per-peer status
└── crates/foto-desktop/    GPUI + gpui-kit shell
    ├── timeline (virtualized grid, month headers), lightbox, devices/pairing, folders, settings
    └── bounded texture LRU for thumbnails
```

The core supports `--data-dir` / `FOTO_HOME`, so two instances can run on one machine. Two-node
sync tests also run in-process, like Vellum's suite.

## Milestones

Each milestone ends with something that works and a PR.

- **M0 Scaffold.** Workspace, CI (fmt, clippy `-D warnings`, tests), release profile, empty GPUI
  window. Measure binary size and cold start.
- **M1 Core library.** Watched folders, rescan, hashing, metadata, thumbnails, replica, index.
  Headless `foto` CLI (`watch add`, `scan`, `ls`). Target: 1K photos plus videos indexed, rescan
  is a no-op.
- **M2 Timeline UI.** Virtualized grid, lightbox, video playback hand-off, folders panel, live
  progress. Target: 60 fps on 10K+ items with bounded memory.
- **M3 Pairing + sync.** Tickets (text and QR), mDNS, thumbs-first fetch, export to library
  folder, resume, devices panel. Target: two machines converge, and a sync killed halfway resumes.
- **M4 Deletes, edits, hardening.** Tombstones, trash, edit handling, 100K synthetic stress test,
  50 GB LAN benchmark, soak test.
- **M5 Packaging.** cargo-packager, GitHub Releases.

## Success metrics (from the note, kept)

- Import 1000+ photos locally
- 60 fps scrolling
- Pair two devices (ticket/QR)
- 50 GB in under 2 h on gigabit LAN (about 56 Mbit/s needed)
- No crashes during a stress test
- Cold start under 200 ms, idle memory under 150 MB, 100K-photo libraries

## Open questions (round 2)

See the conversation log. Answers get folded into "Decisions" above.
