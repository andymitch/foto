use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, UNIX_EPOCH};

use anyhow::{Result, bail};
use iroh_blobs::api::blobs::{AddPathOptions, ImportMode};
use iroh_blobs::{BlobFormat, Hash};
use notify_debouncer_full::notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{DebounceEventResult, Debouncer, RecommendedCache, new_debouncer};

use crate::core::{Core, TRASH_DAYS, now};
use crate::media::{self, Probe};
use crate::model::{self, Asset, Key, Kind, Tombstone};

pub type Watcher = Debouncer<RecommendedWatcher, RecommendedCache>;

pub const TRASH_DIR: &str = ".foto-trash";
const LIVE_MAX_MS: u64 = 4000;
const STILL_EXT: &[&str] = &["HEIC", "heic", "JPG", "jpg", "JPEG", "jpeg"];
const MOTION_EXT: &[&str] = &["MOV", "mov", "MP4", "mp4"];

pub fn start_watcher(core: &Arc<Core>) -> Result<()> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Vec<PathBuf>>();
    let debouncer = new_debouncer(
        Duration::from_secs(2),
        None,
        move |res: DebounceEventResult| {
            if let Ok(events) = res {
                let paths: Vec<PathBuf> = events.into_iter().flat_map(|e| e.event.paths).collect();
                let _ = tx.send(paths);
            }
        },
    )?;
    *core.watcher.lock().unwrap() = Some(debouncer);
    for (_, root, _) in core.ix().roots()? {
        watch(core, &root);
    }
    let c = core.clone();
    tokio::spawn(async move {
        while let Some(paths) = rx.recv().await {
            c.on_paths(paths).await;
        }
    });
    Ok(())
}

pub fn watch(core: &Core, root: &str) {
    if let Some(w) = core.watcher.lock().unwrap().as_mut()
        && let Err(e) = w.watch(Path::new(root), RecursiveMode::Recursive)
    {
        tracing::warn!("watch {root}: {e}");
    }
}

fn hidden(rel: &Path) -> bool {
    rel.components()
        .any(|c| c.as_os_str().to_str().is_some_and(|s| s.starts_with('.')))
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if e.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        match e.file_type() {
            Ok(t) if t.is_dir() => walk(&p, out),
            Ok(t) if t.is_file() && media::kind_of(&p).is_some() => out.push(p),
            _ => {}
        }
    }
}

fn mtime(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn sibling(path: &Path, exts: &[&str]) -> Option<PathBuf> {
    exts.iter()
        .map(|e| path.with_extension(e))
        .find(|p| p != path && p.is_file())
}

#[cfg(not(target_os = "android"))]
fn os_trash(path: &str) -> Result<()> {
    Ok(trash::delete(path)?)
}

#[cfg(target_os = "android")]
fn os_trash(path: &str) -> Result<()> {
    Ok(std::fs::remove_file(path)?)
}

pub fn date_dir(taken_at: i64) -> PathBuf {
    let d = chrono::DateTime::from_timestamp(taken_at, 0).unwrap_or_default();
    PathBuf::from(d.format("%Y").to_string()).join(d.format("%m").to_string())
}

pub fn unique_target(dir: &Path, name: &str, hash: &Hash) -> PathBuf {
    let candidate = dir.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let p = Path::new(name);
    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("file");
    let short = &hash.to_string()[..8];
    match p.extension().and_then(|e| e.to_str()) {
        Some(ext) => dir.join(format!("{stem}-{short}.{ext}")),
        None => dir.join(format!("{stem}-{short}")),
    }
}

impl Core {
    fn root_of(&self, path: &Path) -> Option<(String, String)> {
        self.ix()
            .roots()
            .ok()?
            .into_iter()
            .map(|(lib, root, _)| (lib, root))
            .filter(|(_, root)| path.starts_with(root))
            .max_by_key(|(_, root)| root.len())
    }

    async fn on_paths(self: &Arc<Self>, paths: Vec<PathBuf>) {
        let mut seen = HashSet::new();
        let mut gone = Vec::new();
        for path in paths {
            if !seen.insert(path.clone()) {
                continue;
            }
            let Some((lib, root)) = self.root_of(&path) else {
                continue;
            };
            if hidden(path.strip_prefix(&root).unwrap_or(&path)) {
                continue;
            }
            if path.is_dir() {
                self.scan_dir(&lib, &root, &path).await;
            } else if path.is_file() {
                if let Err(e) = self.ingest(&lib, &path, false).await {
                    tracing::warn!("ingest {}: {e:#}", path.display());
                }
            } else {
                gone.push(path);
            }
        }
        for path in gone {
            let p = path.to_string_lossy().into_owned();
            let known = self.ix().path_stat(&p).is_some();
            let under = if known {
                vec![p]
            } else {
                match self.root_of(&path) {
                    Some((lib, _)) => self.ix().paths_under(&lib, &p).unwrap_or_default(),
                    None => Vec::new(),
                }
            };
            for p in under {
                if let Err(e) = self.missing(&p).await {
                    tracing::warn!("missing {p}: {e:#}");
                }
            }
        }
    }

    pub async fn scan_all(self: &Arc<Self>) {
        let roots = self.ix().roots().unwrap_or_default();
        for (lib, root, _) in roots {
            self.scan_dir(&lib, &root, Path::new(&root)).await;
        }
        self.status_dirty.store(true, Ordering::Relaxed);
    }

    async fn scan_dir(self: &Arc<Self>, lib: &str, root: &str, dir: &Path) {
        if !Path::new(root).is_dir() || !dir.is_dir() {
            return;
        }
        let d = dir.to_owned();
        let files = tokio::task::spawn_blocking(move || {
            let mut out = Vec::new();
            walk(&d, &mut out);
            out
        })
        .await
        .unwrap_or_default();
        let found: HashSet<String> = files
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        let mut tasks = tokio::task::JoinSet::new();
        for f in files {
            let Ok(permit) = self.ingest_limit.clone().acquire_owned().await else {
                break;
            };
            let core = self.clone();
            let lib = lib.to_string();
            tasks.spawn(async move {
                if let Err(e) = core.ingest(&lib, &f, false).await {
                    tracing::warn!("ingest {}: {e:#}", f.display());
                }
                drop(permit);
            });
        }
        while tasks.join_next().await.is_some() {}
        let prefix = dir.to_string_lossy().into_owned();
        let known = self.ix().paths_under(lib, &prefix).unwrap_or_default();
        for p in known {
            if !found.contains(&p)
                && !Path::new(&p).exists()
                && let Err(e) = self.missing(&p).await
            {
                tracing::warn!("missing {p}: {e:#}");
            }
        }
    }

    async fn import_blob(&self, path: &Path) -> Result<Hash> {
        let tt = self
            .store
            .blobs()
            .add_path_with_opts(AddPathOptions {
                path: path.to_owned(),
                format: BlobFormat::Raw,
                mode: ImportMode::TryReference,
            })
            .temp_tag()
            .await?;
        let hash = tt.hash();
        self.store.tags().set(format!("o/{hash}"), hash).await?;
        Ok(hash)
    }

    pub async fn add_bytes_tagged(&self, bytes: Vec<u8>) -> Result<Hash> {
        let tt = self.store.blobs().add_bytes(bytes).temp_tag().await?;
        let hash = tt.hash();
        self.store.tags().set(format!("t/{hash}"), hash).await?;
        Ok(hash)
    }

    pub async fn ingest(self: &Arc<Self>, lib: &str, path: &Path, force: bool) -> Result<()> {
        let Some(kind) = media::kind_of(path) else {
            return Ok(());
        };
        let meta = tokio::fs::metadata(path).await?;
        if !meta.is_file() {
            return Ok(());
        }
        let (size, mt) = (meta.len(), mtime(&meta));
        let ps = path.to_string_lossy().into_owned();
        let prev = self.ix().path_stat(&ps);
        if !force
            && prev
                .as_ref()
                .is_some_and(|(_, _, s, m)| *s == size && *m == mt)
        {
            return Ok(());
        }
        if !self.lib(lib)?.writable {
            return Ok(());
        }
        self.indexing.fetch_add(1, Ordering::Relaxed);
        self.status_dirty.store(true, Ordering::Relaxed);
        let res = self
            .ingest_inner(lib, path, kind, size, mt, prev.map(|p| p.1))
            .await;
        self.indexing.fetch_sub(1, Ordering::Relaxed);
        self.status_dirty.store(true, Ordering::Relaxed);
        res
    }

    async fn probe(&self, path: &Path) -> Probe {
        let p = path.to_owned();
        tokio::task::spawn_blocking(move || media::probe(&p))
            .await
            .unwrap_or_default()
    }

    async fn ingest_inner(
        self: &Arc<Self>,
        lib: &str,
        path: &Path,
        kind: Kind,
        size: u64,
        mt: i64,
        prev: Option<Hash>,
    ) -> Result<()> {
        let ps = path.to_string_lossy().into_owned();
        if kind == Kind::Video
            && let Some(still) = sibling(path, STILL_EXT)
            && self.probe(path).await.duration_ms <= LIVE_MAX_MS
        {
            let hash = self.import_blob(path).await?;
            self.ix().put_path(&ps, lib, &hash, size, mt)?;
            return Box::pin(self.ingest(lib, &still, true)).await;
        }
        let pair = match (kind, sibling(path, MOTION_EXT)) {
            (Kind::Photo, Some(video)) if self.probe(&video).await.duration_ms <= LIVE_MAX_MS => {
                let vs = video.to_string_lossy().into_owned();
                let known = self.ix().path_stat(&vs).map(|p| p.1);
                match known {
                    Some(h) => Some(h),
                    None => {
                        let h = self.import_blob(&video).await?;
                        let vm = std::fs::metadata(&video)?;
                        self.ix().put_path(&vs, lib, &h, vm.len(), mtime(&vm))?;
                        Some(h)
                    }
                }
            }
            _ => None,
        };
        let hash = self.import_blob(path).await?;
        let edited = {
            let ix = self.ix();
            ix.put_path(&ps, lib, &hash, size, mt)?;
            match prev {
                Some(old)
                    if old != hash
                        && ix.paths_of(lib, &old)?.is_empty()
                        && ix.asset(lib, &old).is_some()
                        && ix.deleted_at(lib, &old).is_none() =>
                {
                    ix.set_tombstone(lib, &old, now(), false)?;
                    Some(old)
                }
                _ => None,
            }
        };
        let existing = self.ix().asset(lib, &hash);
        match existing {
            Some(mut a) if pair.is_some() && a.pair != pair => {
                a.pair = pair;
                a.added_at = now();
                self.put_apply(lib, Key::Asset(hash), model::encode(&a))
                    .await?;
            }
            Some(_) => {}
            None => {
                let platform = self.platform.clone();
                let p = path.to_owned();
                let (probe, thumbs) = tokio::task::spawn_blocking(move || {
                    (media::probe(&p), media::thumbs(&*platform, &p, kind))
                })
                .await?;
                let thumb_s = self.add_bytes_tagged(thumbs.small).await?;
                let thumb_l = self.add_bytes_tagged(thumbs.large).await?;
                let (w, h) = media::same_orientation(probe.w, probe.h, thumbs.w, thumbs.h);
                let meta = std::fs::metadata(path)?;
                let asset = Asset {
                    kind,
                    taken_at: probe
                        .taken_at
                        .unwrap_or_else(|| meta.modified().map(media::local_secs).unwrap_or(0)),
                    mime: media::mime_of(path),
                    w,
                    h,
                    duration_ms: probe.duration_ms,
                    size,
                    name: path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned(),
                    camera: probe.camera,
                    thumb_s,
                    thumb_l,
                    pair,
                    motion: probe.motion,
                    origin: self.me.clone(),
                    added_at: now(),
                };
                self.put_apply(lib, Key::Asset(hash), model::encode(&asset))
                    .await?;
            }
        }
        if self.ix().deleted_at(lib, &hash).is_none() {
            self.set_holding(lib, &hash, true).await?;
        }
        if let Some(old) = edited {
            let t = Tombstone {
                deleted_at: now(),
                restored: false,
            };
            self.put_apply(lib, Key::Tombstone(old), model::encode(&t))
                .await?;
        } else if let Some(old) = prev
            && old != hash
        {
            self.release(lib, &old, false).await?;
        }
        Ok(())
    }

    async fn release(self: &Arc<Self>, lib: &str, hash: &Hash, tombstone: bool) -> Result<()> {
        if !self.ix().paths_of(lib, hash)?.is_empty() {
            return Ok(());
        }
        self.set_holding(lib, hash, false).await?;
        let live =
            self.ix().asset(lib, hash).is_some() && self.ix().deleted_at(lib, hash).is_none();
        if tombstone && live && self.lib(lib)?.writable {
            let t = Tombstone {
                deleted_at: now(),
                restored: false,
            };
            self.put_apply(lib, Key::Tombstone(*hash), model::encode(&t))
                .await?;
        }
        Ok(())
    }

    async fn missing(self: &Arc<Self>, path: &str) -> Result<()> {
        let Some((lib, hash, ..)) = self.ix().path_stat(path) else {
            return Ok(());
        };
        let Some((_, root)) = self.root_of(Path::new(path)) else {
            return Ok(());
        };
        if !Path::new(&root).is_dir() {
            return Ok(());
        }
        self.ix().remove_path(path)?;
        let was_pending = self.ix().pending_trash()?.iter().any(|(p, ..)| p == path);
        if was_pending {
            self.ix().remove_pending_trash(path)?;
        }
        self.release(&lib, &hash, !was_pending).await
    }

    pub async fn local_trash(self: &Arc<Self>, lib: &str, hash: &Hash) -> Result<()> {
        if self.policy().archive {
            return Ok(());
        }
        let mut hashes = vec![*hash];
        hashes.extend(self.ix().asset(lib, hash).and_then(|a| a.pair));
        let mut paths = Vec::new();
        for h in &hashes {
            let found = self.ix().paths_of(lib, h)?;
            for p in found {
                paths.push((p, *h));
            }
            if let Some(c) = self.ix().cached(lib, h) {
                let _ = std::fs::remove_file(&c);
            }
        }
        if paths.is_empty() {
            return self.set_holding(lib, hash, false).await;
        }
        let list: Vec<String> = paths.iter().map(|(p, _)| p.clone()).collect();
        if self.platform.trash(list) {
            for (p, h) in &paths {
                self.ix().set_pending_trash(p, lib, h)?;
            }
            return Ok(());
        }
        for (p, h) in &paths {
            let Some((_, root)) = self.root_of(Path::new(p)) else {
                continue;
            };
            let rel = Path::new(p).strip_prefix(&root)?;
            let target = Path::new(&root).join(TRASH_DIR).join(rel);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            if Path::new(p).exists() {
                std::fs::rename(p, &target)?;
            }
            let ix = self.ix();
            ix.put_trashed(p, lib, h, &target.to_string_lossy(), now())?;
            ix.remove_path(p)?;
        }
        for h in &hashes {
            let _ = self.store.tags().delete(format!("o/{h}")).await;
        }
        self.set_holding(lib, hash, false).await
    }

    pub async fn local_restore(self: &Arc<Self>, lib: &str, hash: &Hash) -> Result<()> {
        let mut hashes = vec![*hash];
        hashes.extend(self.ix().asset(lib, hash).and_then(|a| a.pair));
        for h in &hashes {
            let pending = self.ix().pending_trash()?;
            for (p, ..) in pending.into_iter().filter(|(_, l, x)| l == lib && x == h) {
                self.ix().remove_pending_trash(&p)?;
            }
            let trashed = self.ix().trashed_of(lib, h)?;
            for (path, trash_path) in trashed {
                if Path::new(&trash_path).exists() && !Path::new(&path).exists() {
                    if let Some(parent) = Path::new(&path).parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    std::fs::rename(&trash_path, &path)?;
                }
                self.ix().remove_trashed(&path)?;
                if Path::new(&path).exists() {
                    Box::pin(self.ingest(lib, Path::new(&path), true)).await?;
                }
            }
        }
        self.wake.notify_one();
        Ok(())
    }

    pub fn pending_trash(&self) -> Vec<String> {
        self.ix()
            .pending_trash()
            .unwrap_or_default()
            .into_iter()
            .map(|(p, ..)| p)
            .collect()
    }

    pub async fn trash_done(self: &Arc<Self>, paths: Vec<String>, confirmed: bool) -> Result<()> {
        for p in paths {
            let entry = self
                .ix()
                .pending_trash()?
                .into_iter()
                .find(|(x, ..)| *x == p);
            let Some((_, lib, hash)) = entry else {
                continue;
            };
            self.ix().remove_pending_trash(&p)?;
            if confirmed {
                self.ix().remove_path(&p)?;
                let _ = self.store.tags().delete(format!("o/{hash}")).await;
                self.release(&lib, &hash, false).await?;
            }
        }
        Ok(())
    }

    pub async fn purge(&self) -> Result<()> {
        let cutoff = now() - TRASH_DAYS * 86400;
        let old = self.ix().trashed_before(cutoff)?;
        for (path, trash_path) in old {
            if Path::new(&trash_path).exists() && os_trash(&trash_path).is_err() {
                let _ = std::fs::remove_file(&trash_path);
            }
            self.ix().remove_trashed(&path)?;
        }
        let expired = self.ix().expired_assets(cutoff)?;
        for (_, _, s, l) in expired {
            let _ = self.store.tags().delete(format!("t/{s}")).await;
            let _ = self.store.tags().delete(format!("t/{l}")).await;
        }
        Ok(())
    }

    pub async fn add_root(self: &Arc<Self>, lib: &str, path: &str) -> Result<()> {
        if !self.lib(lib)?.writable {
            bail!("library is read-only");
        }
        let root = Path::new(path).canonicalize()?;
        if !root.is_dir() {
            bail!("{path} is not a folder");
        }
        let root = root.to_string_lossy().into_owned();
        self.ix().add_root(lib, &root)?;
        watch(self, &root);
        let core = self.clone();
        let lib = lib.to_string();
        tokio::spawn(async move { core.scan_dir(&lib, &root, Path::new(&root)).await });
        Ok(())
    }

    pub async fn remove_root(self: &Arc<Self>, lib: &str, path: &str) -> Result<()> {
        let hashes: Vec<Hash> = {
            let ix = self.ix();
            ix.paths_under(lib, path)?
                .iter()
                .filter_map(|p| ix.path_stat(p).map(|s| s.1))
                .collect()
        };
        self.ix().remove_root(lib, path)?;
        if let Some(w) = self.watcher.lock().unwrap().as_mut() {
            let _ = w.unwatch(Path::new(path));
        }
        for h in hashes {
            self.release(lib, &h, false).await?;
        }
        self.touch(lib);
        Ok(())
    }

    pub fn target_for(&self, lib: &str, a: &Asset, hash: &Hash) -> PathBuf {
        let dir = self.library_dir(lib).join(date_dir(a.taken_at));
        unique_target(&dir, &a.name, hash)
    }

    pub async fn save_original(self: &Arc<Self>, lib: &str, hash: &str) -> Result<Option<String>> {
        let h = crate::core::parse_hash(hash)?;
        let Some(src) = self.original(lib, hash) else {
            return Ok(None);
        };
        if !self.ix().paths_of(lib, &h)?.is_empty() {
            return Ok(Some(src));
        }
        let Some(a) = self.ix().asset(lib, &h) else {
            return Ok(None);
        };
        let dest = self.target_for(lib, &a, &h);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(&src, &dest)?;
        self.ensure_library_roots_for(lib)?;
        self.ingest(lib, &dest, false).await?;
        Ok(Some(dest.to_string_lossy().into_owned()))
    }

    fn ensure_library_roots_for(&self, lib: &str) -> Result<()> {
        let dir = self
            .library_dir(lib)
            .canonicalize()?
            .to_string_lossy()
            .into_owned();
        if !self
            .ix()
            .roots()?
            .iter()
            .any(|(l, p, _)| l == lib && *p == dir)
        {
            self.ix().add_root(lib, &dir)?;
            watch(self, &dir);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_paths() {
        assert!(hidden(Path::new(".foto-trash/a.jpg")));
        assert!(hidden(Path::new("x/.thumbs/a.jpg")));
        assert!(!hidden(Path::new("2024/05/a.jpg")));
    }

    #[test]
    fn unique_targets() {
        let dir = std::env::temp_dir().join(format!("foto-uniq-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let h = Hash::new(b"x");
        assert_eq!(unique_target(&dir, "a.jpg", &h), dir.join("a.jpg"));
        std::fs::write(dir.join("a.jpg"), b"1").unwrap();
        let t = unique_target(&dir, "a.jpg", &h);
        assert_eq!(t, dir.join(format!("a-{}.jpg", &h.to_string()[..8])));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn date_dirs() {
        assert_eq!(date_dir(1_700_000_000), PathBuf::from("2023").join("11"));
    }
}
