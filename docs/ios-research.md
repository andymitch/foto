# iOS: platform capabilities for a P2P photo library

Status: **deferred** (#2). Research done to see how native foto can be on iOS. Sources: Apple PhotoKit / BackgroundTasks
documentation (fetched October 2026), and the iOS shells of Ente (`ente-io/ente`,
`mobile/apps/photos/ios`) and Immich (`immich-app/immich`, `mobile/ios`).

## What the reference apps do

Both apps have Flutter UIs with native Swift for background work.

| | Ente | Immich |
|-|------|--------|
| Background modes | `fetch`, `processing`, `remote-notification` | `fetch`, `processing` |
| BG tasks | `BGAppRefreshTask` + `BGProcessingTask` (two each, for legacy and native) | `BGAppRefreshTask` (capped at 20 s, every ≥5 min) + `BGProcessingTask` (`requiresNetworkConnectivity`, every ≥15 min) |
| Wake-ups | Silent push from their server (APNs) | none beyond BG tasks |
| Library change tracking | PhotoKit fetch | `PHPhotoLibrary.fetchPersistentChanges(since:)` with a stored `PHPersistentChangeToken`, and a full rescan when the token has expired |
| Transfer | HTTPS to their cloud | HTTPS, background `URLSession` |
| Local network | `NSBonjourServices` + `NSLocalNetworkUsageDescription` | same |
| New iOS 26 upload extension | not adopted | not adopted |

Neither app gets "always-on" background sync on iOS, because the OS doesn't allow it. Their
advantage over a P2P app is that background `URLSession` and silent push only need *a server*.

## iOS capabilities that matter for foto

| Capability | Fit for foto | Notes |
|------------|-------------|-------|
| `PHPersistentChangeToken` (iOS 16+) | **Use** | Cheap incremental detection of new, changed and deleted assets, even across app launches. This replaces observing the whole library |
| `PHPhotoLibraryChangeObserver` | **Use** while running | Live updates while the app is open |
| `PHAssetResourceManager` export | **Use** | Gets original bytes (HEIC + paired Live Photo MOV) into staging for the iroh-blobs import |
| `BGAppRefreshTask` | **Use** | About 30 s every so often: doc sync (metadata, holdings), and small thumbnail sends |
| `BGProcessingTask` | **Use** | Minutes-long, usually while charging and idle overnight. Bulk original transfer to desktop |
| `BGContinuedProcessingTask` (iOS 26) | **Use** | Starts from a user action ("Sync now" / first backup), keeps running after the app is backgrounded, and shows progress in a system Live Activity the user can cancel. Network allowed. Ideal for the initial library upload |
| Background `URLSession` | **Optional upgrade** | The only way transfers keep going after the app is suspended (run by `nsurlsessiond`). HTTP only, not iroh/QUIC. Could POST originals to a desktop's LAN HTTP ingest endpoint (`NSAllowsLocalNetworking` permits plain HTTP to LAN/`.local`). Needs a small authenticated HTTP endpoint in desktop foto-core |
| `PHBackgroundResourceUploadExtension` (iOS 26.1+, async variant iOS 27) | **Not now** | Apple's new system-managed photo backup: Photos itself uploads assets, even fetching from iCloud first. But it needs a fixed `BackgroundUploadURLBase` in Info.plist (build-time), the IETF resumable-upload protocol (OPTIONS preflight + `104` interim responses), and it can't be tested in the Simulator. A P2P desktop has no stable public URL. It could work for one personal build pointing at `http://<desktop>.local`, but that's fragile. Revisit after MVP |
| Silent push (`remote-notification`) | **Not possible** | Needs an APNs sender, which is a server. Out of scope by design |
| mDNS (iroh raw multicast) | **Limited** | Needs `com.apple.developer.networking.multicast`, which takes a paid account and Apple approval. Without it, use n0 DNS discovery + relays, which still hole-punch to direct LAN paths |
| VisionKit `DataScannerViewController` | **Use** | Native QR scanning for pairing (camera permission, no custom camera UI) |
| `PHAssetChangeRequest.deleteAssets` | **Use** | Remote deletes go to Recently Deleted (30 days) after one system confirmation per batch |
| `PHAssetCreationRequest` | **Use** | "Save to Photos" for received originals, keeping Live Photo pairs together |
| Camera capture | **Not needed** | Photos are taken with the system Camera and picked up through PhotoKit. The camera is only used for QR scanning |
| Limited photo access (iOS 14+) | **Handle** | Prompt for full access. In limited mode, only the selected assets sync |

## What this means for foto on iOS

1. **Foreground:** full iroh node, live doc sync, and transfers at full speed while the app is
   open.
2. **First backup or large catch-up:** `BGContinuedProcessingTask`, started from a "Sync now"
   button, with progress shown as a Live Activity.
3. **Ongoing:** `BGAppRefreshTask` keeps the index, holdings and thumbnails current.
   `BGProcessingTask` moves originals overnight.
4. **Change detection:** a persistent change token, so every wake-up only looks at what changed.
5. **Phase 2 option:** a LAN HTTP ingest endpoint on desktop plus background `URLSession`, for
   uploads that continue while the app is suspended. Then possibly the iOS 26 upload extension.

Expected experience: close to Immich and Ente, except that photos taken while the phone is away
from the desktop only move once both are online and iOS gives foto runtime. iroh relays handle
"not on the same LAN".

## Build constraint: getting the iOS SDK without the App Store

Every route to the current iOS SDK needs an **Apple ID** (a free one is enough). The App Store
is not required.

| Route | Apple ID? | Notes |
|-------|-----------|-------|
| Mac App Store | yes (App Store sign-in) | Blocked on this machine |
| developer.apple.com/download `.xip` | yes (browser sign-in, free account) | `download.developer.apple.com` returns 302 to `/unauthorized/` without the `ADCDownloadAuth` cookie (checked) |
| `xcodes` CLI | yes (Apple ID login) | Wraps the same download. `xcodes install --path X.xip` installs a .xip from anywhere |
| `xtool` (SwiftPM-based Xcode replacement) | yes | Still needs Xcode on macOS, or `Xcode.xip` on Linux, for the SDK. Its "password" login works with any free Apple ID, including a throwaway one |
| `theos/sdks` patched SDKs | no | Stub SDKs up to iOS 16.5, no Simulator, missing iOS 17+/26 APIs. Not usable for a SwiftUI app |
| GitHub Actions macOS runners | no | Xcode preinstalled. Unsigned device builds, Simulator builds and tests work |
| Installing on an iPhone | yes | Even free provisioning (7-day profiles via Xcode or xtool) needs an Apple ID |

Plan:
- **Now:** build and test iOS in GitHub Actions only.
- **If any Apple ID becomes available** (a throwaway one is fine and doesn't need the App
  Store): download the `.xip` in a browser and install it with `xcodes install --path`. That
  gives local Xcode, the Simulator, and xtool/Xcode on-device installs with 7-day free
  provisioning.
