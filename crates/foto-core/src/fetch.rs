use std::collections::HashMap;
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use iroh::EndpointId;
use iroh_blobs::Hash;
use iroh_blobs::api::blobs::ExportOptions;
use iroh_blobs::api::proto::ExportMode;

use crate::core::Core;

const HEADROOM: u64 = 2 << 30;
const CACHE_MAX: u64 = 2 << 30;
const RETRY: Duration = Duration::from_secs(60);

#[cfg(unix)]
fn free_space(path: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    (unsafe { libc::statvfs(c.as_ptr(), &mut s) } == 0)
        .then(|| s.f_bavail as u64 * s.f_frsize as u64)
}

#[cfg(not(unix))]
fn free_space(_: &Path) -> Option<u64> {
    None
}

impl Core {
    fn providers(&self, lib: &str, hash: &Hash) -> Vec<EndpointId> {
        let ix = self.ix();
        let mut ids = ix.holders(lib, hash).unwrap_or_default();
        if let Some(a) = ix.asset(lib, hash) {
            ids.push(a.origin);
        }
        drop(ix);
        if let Some(on) = self.online.lock().unwrap().get(lib) {
            ids.extend(on.iter().cloned());
        }
        let mut seen = std::collections::HashSet::new();
        ids.into_iter()
            .filter(|id| *id != self.me && seen.insert(id.clone()))
            .filter_map(|id| EndpointId::from_str(&id).ok())
            .collect()
    }

    async fn download(&self, hash: Hash, providers: Vec<EndpointId>) -> Result<()> {
        if self.store.blobs().has(hash).await? {
            return Ok(());
        }
        if providers.is_empty() {
            return Err(anyhow!("no providers"));
        }
        self.downloader.download(hash, providers).await?;
        Ok(())
    }

    pub async fn fetch_loop(self: Arc<Self>) {
        let mut failed: HashMap<Hash, Instant> = HashMap::new();
        loop {
            let progressed = self.fetch_round(&mut failed).await;
            self.status_dirty.store(true, Ordering::Relaxed);
            if !progressed {
                tokio::select! {
                    _ = self.wake.notified() => {}
                    _ = tokio::time::sleep(Duration::from_secs(30)) => {}
                }
            }
        }
    }

    async fn fetch_round(self: &Arc<Self>, failed: &mut HashMap<Hash, Instant>) -> bool {
        failed.retain(|_, t| t.elapsed() < RETRY);
        let mut progressed = false;
        let urgent: Vec<(String, Hash)> = self.urgent.lock().unwrap().drain(..).collect();
        for (lib, hash) in urgent {
            match self.fetch_original(&lib, &hash).await {
                Ok(()) => progressed = true,
                Err(e) => tracing::debug!("original {hash}: {e:#}"),
            }
        }
        for large in [false, true] {
            let want = self.ix().missing_thumbs(large, 64).unwrap_or_default();
            let want: Vec<_> = want
                .into_iter()
                .filter(|w| !failed.contains_key(&w.2))
                .collect();
            let mut tasks = tokio::task::JoinSet::new();
            for (lib, asset, thumb) in want {
                let core = self.clone();
                tasks.spawn(async move {
                    let providers = core.providers(&lib, &asset);
                    let r = core.download(thumb, providers).await;
                    if r.is_ok() {
                        let _ = core.store.tags().set(format!("t/{thumb}"), thumb).await;
                        let _ = core.ix().set_thumb_flag(&lib, &asset, large);
                        core.touch(&lib);
                    }
                    (thumb, r)
                });
            }
            let results = tasks.join_all().await;
            for (thumb, r) in results {
                match r {
                    Ok(()) => progressed = true,
                    Err(_) => {
                        failed.insert(thumb, Instant::now());
                    }
                }
            }
            if progressed {
                return true;
            }
        }
        let policy = self.policy();
        if !policy.originals_all {
            self.set_paused(None);
            return progressed;
        }
        if self.platform.is_metered() && !policy.metered_ok {
            self.set_paused(Some("Waiting for an unmetered network".into()));
            return progressed;
        }
        let mut want = self.ix().missing_originals(16).unwrap_or_default();
        want.retain(|w| !failed.contains_key(&w.hash));
        for w in want {
            let free = free_space(Path::new(&self.cfg.library_dir));
            if free.is_some_and(|f| f < w.size + HEADROOM) {
                self.set_paused(Some("Not enough disk space".into()));
                return progressed;
            }
            self.set_paused(None);
            match self.fetch_original(&w.lib, &w.hash).await {
                Ok(()) => progressed = true,
                Err(e) => {
                    tracing::debug!("original {}: {e:#}", w.hash);
                    failed.insert(w.hash, Instant::now());
                }
            }
        }
        progressed
    }

    fn set_paused(&self, reason: Option<String>) {
        let mut p = self.paused.lock().unwrap();
        if *p != reason {
            *p = reason;
            self.status_dirty.store(true, Ordering::Relaxed);
        }
    }

    async fn export(&self, hash: Hash, target: &Path) -> Result<()> {
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        self.store
            .blobs()
            .export_with_opts(ExportOptions {
                hash,
                mode: ExportMode::TryReference,
                target: target.to_owned(),
            })
            .finish()
            .await?;
        Ok(())
    }

    pub async fn fetch_original(self: &Arc<Self>, lib: &str, hash: &Hash) -> Result<()> {
        let a = self
            .ix()
            .asset(lib, hash)
            .ok_or_else(|| anyhow!("unknown asset"))?;
        let local = self
            .ix()
            .paths_of(lib, hash)?
            .into_iter()
            .any(|p| Path::new(&p).exists())
            || self
                .ix()
                .cached(lib, hash)
                .is_some_and(|p| Path::new(&p).exists());
        if local {
            return Ok(());
        }
        let providers = self.providers(lib, hash);
        self.download(*hash, providers.clone()).await?;
        self.store.tags().set(format!("o/{hash}"), *hash).await?;
        if let Some(pair) = a.pair {
            self.download(pair, providers).await?;
            self.store.tags().set(format!("o/{pair}"), pair).await?;
        }
        if self.policy().originals_all {
            self.ensure_library_roots().await?;
            let target = self.target_for(lib, &a, hash);
            self.export(*hash, &target).await?;
            let m = std::fs::metadata(&target)?;
            let mt = m
                .modified()?
                .duration_since(std::time::UNIX_EPOCH)?
                .as_secs() as i64;
            self.ix()
                .put_path(&target.to_string_lossy(), lib, hash, m.len(), mt)?;
            if let Some(pair) = a.pair {
                let pt = target.with_extension("MOV");
                self.export(pair, &pt).await?;
                let m = std::fs::metadata(&pt)?;
                let mt = m
                    .modified()?
                    .duration_since(std::time::UNIX_EPOCH)?
                    .as_secs() as i64;
                self.ix()
                    .put_path(&pt.to_string_lossy(), lib, &pair, m.len(), mt)?;
            }
            self.set_holding(lib, hash, true).await?;
        } else {
            let ext = Path::new(&a.name)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("bin");
            let target = Path::new(&self.cfg.data_dir)
                .join("cache")
                .join(format!("{hash}.{ext}"));
            self.export(*hash, &target).await?;
            self.ix()
                .put_cache(lib, hash, &target.to_string_lossy(), a.size)?;
            let evicted = self.ix().evict_cache(CACHE_MAX)?;
            for (_, h, p) in evicted {
                let _ = std::fs::remove_file(p);
                let _ = self.store.tags().delete(format!("o/{h}")).await;
            }
        }
        self.touch(lib);
        Ok(())
    }
}
