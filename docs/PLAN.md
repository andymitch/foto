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
| 9 | Not specified: where originals live | A copy-in store doubles disk use for 50 GB+ libraries | Import **by reference** (iroh-blobs `ImportMode::TryReference`) for the local device's photos. Received photos go into the blob store and can be exported to a folder on demand. See open question 1 |
| 10 | `image` 0.25 for thumbnails | Can't read HEIC or RAW, and full decodes of 100K photos are slow | MVP supports JPEG/PNG/WebP/GIF/TIFF. Use the embedded EXIF preview when there is one, otherwise decode and resize with `fast_image_resize`, on a bounded blocking pool. HEIC comes later via `libheif-rs` or the platform's image APIs |
| 11 | Not specified: deletes | Without tombstones, a deleted photo comes back on the next sync | A delete writes a tombstone entry to the doc. The blob is garbage collected once no live entry references it |
| 12 | Peers fetch originals before anything else | Browsing a new peer's library would wait on gigabytes | Thumbnails sync first (small blobs), originals afterwards in the background, so the timeline fills in quickly |
| 13 | ~500 LOC core, ~1000 LOC UI | Unrealistic. The import, sync, thumbnail and timeline paths each need more than that | Milestones below. LOC is not a target |
| 14 | 15–25 MB binary | gpui plus iroh plus bundled SQLite will probably go over that | Measure at M1. Use `opt-level="z"`, LTO, `strip`, `panic=abort` if size matters. This is a soft goal |
| 15 | `photosync/` naming | — | Project, crates and binary are named `foto` |

Async integration: GPUI has its own executor and iroh needs tokio. The core owns a tokio runtime
on its own threads and exposes a handle. The UI sends commands and gets events back over channels,
so the UI never touches tokio directly.

## Architecture

```
foto/
├── Cargo.toml              workspace
├── crates/foto-core/       no UI. Library, import, thumbs, sync, index
│   ├── node.rs             iroh endpoint + router (blobs, docs, gossip), mDNS, key persistence
│   ├── library.rs          doc replica: photo entries, tombstones, pairing tickets
│   ├── import.rs           folder walk, hash, EXIF, thumbnail jobs
│   ├── thumbs.rs           decode/resize, sizes S (grid ~256px) and L (lightbox ~2048px)
│   ├── index.rs            rusqlite timeline index, rebuilt from doc events
│   └── sync.rs             blob fetch queue (thumbs, then originals), per-peer status
└── crates/foto-desktop/    GPUI + gpui-kit shell
    ├── timeline (virtualized grid, uniform_list), lightbox, import dialog, devices/pairing panel
    └── bounded texture LRU for thumbnails (keeps idle memory low)
```

### Data model (doc entries)

- Key `p/<blake3-of-original>` → value: small postcard/JSON record
  `{ taken_at, mime, w, h, size, name, thumb_s, thumb_l, added_by, added_at }`.
  Using the content hash as the key dedups across devices for free.
- Key `t/<hash>` → tombstone (`deleted_at`, `by`).
- Key `d/<endpoint-id>` → device record (name, last seen). Per-peer status comes from this plus
  live sync events.

Each device's blob store holds the originals (by reference for local imports) plus the S/L
thumbnail blobs. The rusqlite index stores `(hash, taken_at, w, h, thumb_s, has_original)` so the
grid can page and seek by date without touching the doc.

### Pairing

Device A creates the library, which is an iroh-docs namespace, and shows a write-capable
`DocTicket` as text and as a QR code. Device B joins with it. After that both devices reconnect
on their own: over LAN via mDNS, over the internet via n0 discovery and relays. Anyone holding the
ticket gets full access, and the UI says so.

## Milestones

Each milestone ends with something that works and a commit/PR.

- **M0 Scaffold.** Workspace, CI (fmt, clippy `-D warnings`, test on macOS and Linux), release
  profile. Empty GPUI window opens. Measure binary size and cold start.
- **M1 Local library.** Import a folder: walk, BLAKE3 hash, EXIF date (falling back to mtime),
  S/L thumbnails, doc entries, index. Headless CLI subcommand (`foto import <dir>`) so the core
  can be tested without the UI. Target: 1K photos imported, re-import is a no-op.
- **M2 Timeline UI.** Virtualized grid grouped by month, lightbox with L thumb (original on
  demand), import dialog with progress. Target: 60 fps scroll on 10K+ items, bounded memory.
- **M3 Pairing + sync.** Ticket share/join, mDNS, thumbs-first fetch queue, resume after restart,
  devices panel with per-peer state (online, entries behind, bytes pending). Target: two machines
  on LAN converge, and a sync killed halfway resumes.
- **M4 Deletes + hardening.** Tombstones, blob GC, stress test with 100K synthetic entries,
  50 GB LAN transfer benchmark, crash-free soak.
- **M5 Packaging.** cargo-packager (.dmg/.app, .deb/AppImage, NSIS), GitHub Releases workflow.

Later: HEIC/RAW, video playback, mobile shells, selective sync.

## Success metrics (from the note, kept)

- Import 1000+ photos locally
- 60 fps scrolling
- Pair two devices (ticket/QR)
- 50 GB in under 2 h on gigabit LAN (only about 56 Mbit/s is needed, so there is plenty of margin)
- No crashes during a stress test
- Cold start under 200 ms, idle memory under 150 MB, 100K-photo libraries

## Open questions

1. **Storage of synced originals.** Keep them only in foto's blob store (simplest), or also
   mirror them to a visible folder such as `~/Pictures/foto/`? Mirroring roughly doubles disk use
   unless hardlinks or reflinks are used.
2. **Videos in MVP.** Import and sync videos as opaque blobs, with a poster-frame thumbnail
   later? Or photos only?
3. **One library or several?** The plan assumes one library per device set.
4. **Watch folders.** Re-scan import folders automatically, or import only when asked (MVP)?
