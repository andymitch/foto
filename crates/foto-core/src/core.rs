use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use futures_lite::StreamExt;
use iroh::endpoint::presets;
use iroh::protocol::Router;
use iroh::{Endpoint, EndpointAddr, EndpointId, SecretKey};
use iroh_blobs::api::downloader::Downloader;
use iroh_blobs::store::GcConfig;
use iroh_blobs::store::fs::FsStore;
use iroh_blobs::store::fs::options::Options;
use iroh_blobs::{BlobsProtocol, Hash};
use iroh_docs::api::Doc;
use iroh_docs::api::protocol::{AddrInfoOptions, ShareMode};
use iroh_docs::engine::{LiveEvent, ProtectCallbackHandler};
use iroh_docs::protocol::Docs;
use iroh_docs::store::Query;
use iroh_docs::{AuthorId, CapabilityKind, DocTicket, NamespaceId};
use iroh_gossip::net::Gossip;
use iroh_mdns_address_lookup::MdnsAddressLookup;
use tokio::sync::Notify;

use crate::index::{AssetSummary, Filter, Index};
use crate::model::{self, Device, Favorite, Key, Meta, Tombstone};
use crate::{
    AssetDetail, Config, DeviceInfo, Event, EventSink, LibraryInfo, Platform, Policy, RootInfo,
    Status,
};

pub const TRASH_DAYS: i64 = 30;

type LibKey = (String, Vec<u8>);

pub struct Lib {
    pub doc: Doc,
    pub writable: bool,
}

pub struct Core {
    pub cfg: Config,
    pub me: String,
    pub endpoint: Endpoint,
    pub store: FsStore,
    pub docs: Docs,
    pub author: AuthorId,
    pub router: Router,
    pub downloader: Downloader,
    pub index: Mutex<Index>,
    pub platform: Arc<dyn Platform>,
    pub sink: Mutex<Option<Arc<dyn EventSink>>>,
    pub libs: Mutex<HashMap<String, Arc<Lib>>>,
    pub online: Mutex<HashMap<String, HashSet<String>>>,
    pub pending_content: Mutex<HashMap<Hash, Vec<LibKey>>>,
    pub wake: Notify,
    pub urgent: Mutex<Vec<(String, Hash)>>,
    pub paused: Mutex<Option<String>>,
    pub changed: Mutex<HashSet<String>>,
    pub status_dirty: std::sync::atomic::AtomicBool,
    pub indexing: AtomicU64,
    pub ingest_limit: Arc<tokio::sync::Semaphore>,
    pub watcher: Mutex<Option<crate::files::Watcher>>,
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn load_key(path: &Path) -> Result<SecretKey> {
    if let Ok(b) = std::fs::read(path)
        && let Ok(arr) = <[u8; 32]>::try_from(b.as_slice())
    {
        return Ok(SecretKey::from_bytes(&arr));
    }
    let sk = SecretKey::generate();
    std::fs::write(path, sk.to_bytes())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(sk)
}

pub fn parse_hash(s: &str) -> Result<Hash> {
    model::hash_from(s).ok_or_else(|| anyhow!("bad hash {s}"))
}

impl Core {
    pub async fn start(cfg: Config, platform: Arc<dyn Platform>) -> Result<Arc<Core>> {
        let dir = PathBuf::from(&cfg.data_dir);
        std::fs::create_dir_all(&dir)?;
        std::fs::create_dir_all(&cfg.library_dir)?;
        let secret = load_key(&dir.join("endpoint-secret"))?;
        let endpoint = Endpoint::builder(presets::N0)
            .secret_key(secret)
            .address_lookup(MdnsAddressLookup::builder())
            .bind()
            .await?;
        let (protect_handler, protect_cb) = ProtectCallbackHandler::new();
        let blobs_root = dir.join("blobs");
        let mut opts = Options::new(&blobs_root);
        opts.gc = Some(GcConfig {
            interval: Duration::from_secs(600),
            add_protected: Some(protect_cb),
        });
        let store = FsStore::load_with_opts(blobs_root.join("blobs.db"), opts).await?;
        let gossip = Gossip::builder().spawn(endpoint.clone());
        let docs = Docs::persistent(dir.clone())
            .protect_handler(protect_handler)
            .spawn(endpoint.clone(), (*store).clone(), gossip.clone())
            .await?;
        let router = Router::builder(endpoint.clone())
            .accept(iroh_blobs::ALPN, BlobsProtocol::new(&store, None))
            .accept(iroh_gossip::ALPN, gossip)
            .accept(iroh_docs::ALPN, docs.clone())
            .spawn();
        let author = docs.author_default().await?;
        let downloader = store.downloader(&endpoint);
        let index = Index::open(&dir.join("index.db"))?;
        if index.setting("device_name").is_none() {
            index.set_setting("device_name", &cfg.device_name)?;
            index.set_setting("originals_all", &cfg.originals_all.to_string())?;
        }
        let me = endpoint.id().to_string();
        let core = Arc::new(Core {
            cfg,
            me,
            endpoint,
            store,
            docs,
            author,
            router,
            downloader,
            index: Mutex::new(index),
            platform,
            sink: Mutex::new(None),
            libs: Mutex::new(HashMap::new()),
            online: Mutex::new(HashMap::new()),
            pending_content: Mutex::new(HashMap::new()),
            wake: Notify::new(),
            urgent: Mutex::new(Vec::new()),
            paused: Mutex::new(None),
            changed: Mutex::new(HashSet::new()),
            status_dirty: std::sync::atomic::AtomicBool::new(true),
            indexing: AtomicU64::new(0),
            ingest_limit: Arc::new(tokio::sync::Semaphore::new(
                std::thread::available_parallelism()
                    .map(|n| n.get())
                    .unwrap_or(4),
            )),
            watcher: Mutex::new(None),
        });
        let mut list = core.docs.list().await?;
        let mut found = Vec::new();
        while let Some(item) = list.next().await {
            found.push(item?);
        }
        for (ns, cap) in found {
            core.arm(ns, matches!(cap, CapabilityKind::Write)).await?;
        }
        crate::files::start_watcher(&core)?;
        tokio::spawn(core.clone().flush_events());
        tokio::spawn(core.clone().fetch_loop());
        tokio::spawn(core.clone().maintenance());
        let c = core.clone();
        tokio::spawn(async move { c.scan_all().await });
        Ok(core)
    }

    pub async fn shutdown(&self) {
        let _ = self.router.shutdown().await;
        let _ = self.store.shutdown().await;
    }

    pub fn ix(&self) -> std::sync::MutexGuard<'_, Index> {
        self.index.lock().unwrap()
    }

    pub fn lib(&self, lib: &str) -> Result<Arc<Lib>> {
        self.libs
            .lock()
            .unwrap()
            .get(lib)
            .cloned()
            .ok_or_else(|| anyhow!("unknown library {lib}"))
    }

    pub fn emit(&self, event: Event) {
        let sink = self.sink.lock().unwrap().clone();
        if let Some(sink) = sink {
            sink.on_event(event);
        }
    }

    pub fn touch(&self, lib: &str) {
        self.changed.lock().unwrap().insert(lib.to_string());
        self.status_dirty.store(true, Ordering::Relaxed);
    }

    async fn flush_events(self: Arc<Self>) {
        let mut last = Status::default();
        loop {
            tokio::time::sleep(Duration::from_millis(300)).await;
            let libs: Vec<String> = self.changed.lock().unwrap().drain().collect();
            for library in libs {
                self.emit(Event::LibraryChanged { library });
            }
            if self.status_dirty.swap(false, Ordering::Relaxed) {
                let status = self.status();
                if status != last {
                    last = status.clone();
                    self.emit(Event::StatusChanged { status });
                }
            }
        }
    }

    async fn maintenance(self: Arc<Self>) {
        let mut ticks = 0u64;
        loop {
            if ticks.is_multiple_of(8640)
                && let Err(e) = self.purge().await
            {
                tracing::warn!("purge: {e:#}");
            }
            let before = std::time::SystemTime::now();
            tokio::time::sleep(Duration::from_secs(10)).await;
            ticks += 1;
            let gap = std::time::SystemTime::now()
                .duration_since(before)
                .unwrap_or_default();
            if gap > Duration::from_secs(40) {
                self.endpoint.network_change().await;
                self.redial().await;
            } else if ticks.is_multiple_of(30) {
                self.redial().await;
            }
        }
    }

    pub fn device_name(&self) -> String {
        self.ix().setting("device_name").unwrap_or_default()
    }

    pub fn policy(&self) -> Policy {
        let ix = self.ix();
        let flag = |k: &str, d: bool| ix.setting(k).map(|v| v == "true").unwrap_or(d);
        Policy {
            originals_all: flag("originals_all", true),
            archive: flag("archive", false),
            metered_ok: flag("metered_ok", false),
        }
    }

    pub async fn set_device_name(self: &Arc<Self>, name: String) -> Result<()> {
        self.ix().set_setting("device_name", &name)?;
        self.publish_device().await
    }

    pub async fn set_policy(self: &Arc<Self>, p: Policy) -> Result<()> {
        {
            let ix = self.ix();
            ix.set_setting("originals_all", &p.originals_all.to_string())?;
            ix.set_setting("archive", &p.archive.to_string())?;
            ix.set_setting("metered_ok", &p.metered_ok.to_string())?;
        }
        self.ensure_library_roots().await?;
        self.wake.notify_one();
        self.publish_device().await
    }

    async fn publish_device(self: &Arc<Self>) -> Result<()> {
        let libs: Vec<String> = self.libs.lock().unwrap().keys().cloned().collect();
        for lib in libs {
            self.put_device(&lib).await?;
        }
        Ok(())
    }

    async fn put_device(self: &Arc<Self>, lib: &str) -> Result<()> {
        if !self.lib(lib)?.writable {
            return Ok(());
        }
        let p = self.policy();
        let d = Device {
            name: self.device_name(),
            platform: self.cfg.platform.clone(),
            originals_all: p.originals_all,
            archive: p.archive,
            last_seen: now(),
        };
        self.put_apply(lib, Key::Device(self.me.clone()), model::encode(&d))
            .await
    }

    pub fn library_dir(&self, lib: &str) -> PathBuf {
        let name = display_name(self.ix().library_name(lib), lib);
        PathBuf::from(&self.cfg.library_dir).join(sanitize(&name))
    }

    pub fn libraries(&self) -> Vec<LibraryInfo> {
        let libs: Vec<(String, bool)> = self
            .libs
            .lock()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.writable))
            .collect();
        let ix = self.ix();
        let mut out: Vec<LibraryInfo> = libs
            .into_iter()
            .map(|(id, writable)| LibraryInfo {
                name: display_name(ix.library_name(&id), &id),
                id,
                writable,
            })
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    pub async fn create_library(self: &Arc<Self>, name: String) -> Result<LibraryInfo> {
        let doc = self.docs.create().await?;
        let id = doc.id().to_string();
        self.ix().set_library(&id, Some(&name))?;
        self.arm(doc.id(), true).await?;
        self.put(&id, Key::Meta, model::encode(&Meta { name: name.clone() }))
            .await?;
        Ok(LibraryInfo {
            id,
            name,
            writable: true,
        })
    }

    pub async fn join(self: &Arc<Self>, ticket: String) -> Result<LibraryInfo> {
        let ticket = DocTicket::from_str(ticket.trim()).context("invalid ticket")?;
        let ns = ticket.capability.id();
        let writable = matches!(ticket.capability, iroh_docs::Capability::Write(_));
        let id = ns.to_string();
        {
            let ix = self.ix();
            for n in &ticket.nodes {
                ix.add_peer(&id, &n.id.to_string())?;
            }
        }
        let doc = match self.docs.open(ns).await {
            Ok(Some(doc)) => doc,
            _ => self.docs.import_namespace(ticket.capability).await?,
        };
        doc.start_sync(ticket.nodes).await?;
        self.ix().set_library(&id, None)?;
        self.arm(ns, writable).await?;
        let name = display_name(self.ix().library_name(&id), &id);
        Ok(LibraryInfo { id, name, writable })
    }

    pub async fn share(&self, lib: &str, writable: bool) -> Result<String> {
        let mode = if writable {
            ShareMode::Write
        } else {
            ShareMode::Read
        };
        Ok(self
            .lib(lib)?
            .doc
            .share(mode, AddrInfoOptions::RelayAndAddresses)
            .await?
            .to_string())
    }

    fn peer_addrs(&self, lib: &str) -> Vec<EndpointAddr> {
        self.ix()
            .peers(lib)
            .unwrap_or_default()
            .into_iter()
            .filter(|p| *p != self.me)
            .filter_map(|p| EndpointId::from_str(&p).ok())
            .map(EndpointAddr::new)
            .collect()
    }

    pub async fn redial(&self) {
        let libs: Vec<(String, Arc<Lib>)> = self
            .libs
            .lock()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        for (id, lib) in libs {
            let _ = lib.doc.start_sync(self.peer_addrs(&id)).await;
        }
    }

    async fn arm(self: &Arc<Self>, ns: NamespaceId, writable: bool) -> Result<()> {
        let id = ns.to_string();
        if self.libs.lock().unwrap().contains_key(&id) {
            return Ok(());
        }
        let doc = self
            .docs
            .open(ns)
            .await?
            .ok_or_else(|| anyhow!("library {id} not found"))?;
        self.ix().set_library(&id, None)?;
        let mut events = doc.subscribe().await?;
        self.libs.lock().unwrap().insert(
            id.clone(),
            Arc::new(Lib {
                doc: doc.clone(),
                writable,
            }),
        );
        doc.start_sync(self.peer_addrs(&id)).await?;
        let core = self.clone();
        let lib = id.clone();
        tokio::spawn(async move {
            while let Some(ev) = events.next().await {
                let Ok(ev) = ev else { continue };
                if let Err(e) = core.on_live(&lib, ev).await {
                    tracing::warn!("live event: {e:#}");
                }
            }
        });
        self.reconcile(&id).await?;
        self.put_device(&id).await?;
        self.ensure_library_roots().await?;
        self.touch(&id);
        Ok(())
    }

    pub async fn ensure_library_roots(self: &Core) -> Result<()> {
        let libs: Vec<(String, bool)> = self
            .libs
            .lock()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.writable))
            .collect();
        if !self.policy().originals_all {
            return Ok(());
        }
        for (lib, _) in libs {
            let dir = self.library_dir(&lib);
            std::fs::create_dir_all(&dir)?;
            let dir = dir.canonicalize()?.to_string_lossy().into_owned();
            let known = self
                .ix()
                .roots()?
                .iter()
                .any(|(l, p, _)| *l == lib && *p == dir);
            if !known {
                self.ix().add_root(&lib, &dir)?;
                crate::files::watch(self, &dir);
            }
        }
        Ok(())
    }

    async fn on_live(self: &Arc<Self>, lib: &str, ev: LiveEvent) -> Result<()> {
        match ev {
            LiveEvent::InsertRemote { entry, .. } => {
                let key = entry.key().to_vec();
                self.apply_key(lib, &key).await?;
            }
            LiveEvent::ContentReady { hash } => {
                let waiting = self.pending_content.lock().unwrap().remove(&hash);
                for (lib, key) in waiting.unwrap_or_default() {
                    self.apply_key(&lib, &key).await?;
                }
            }
            LiveEvent::NeighborUp(id) => {
                let id = id.to_string();
                self.ix().add_peer(lib, &id)?;
                self.online
                    .lock()
                    .unwrap()
                    .entry(lib.to_string())
                    .or_default()
                    .insert(id);
                self.touch(lib);
                self.wake.notify_one();
            }
            LiveEvent::NeighborDown(id) => {
                if let Some(s) = self.online.lock().unwrap().get_mut(lib) {
                    s.remove(&id.to_string());
                }
                self.touch(lib);
            }
            LiveEvent::SyncFinished(ev) => {
                let id = ev.peer.to_string();
                self.ix().add_peer(lib, &id)?;
                if ev.result.is_ok() {
                    self.online
                        .lock()
                        .unwrap()
                        .entry(lib.to_string())
                        .or_default()
                        .insert(id);
                }
                self.touch(lib);
                self.wake.notify_one();
            }
            _ => {}
        }
        Ok(())
    }

    async fn reconcile(self: &Arc<Self>, lib: &str) -> Result<()> {
        let known = self.ix().entry_hashes(lib)?;
        let doc = self.lib(lib)?.doc.clone();
        let mut stream = Box::pin(doc.get_many(Query::single_latest_per_key()).await?);
        while let Some(entry) = stream.next().await {
            let entry = entry?;
            let hash = entry.content_hash().to_string();
            if known.get(entry.key()) == Some(&hash) {
                continue;
            }
            self.apply_entry(lib, entry.key(), entry.content_hash(), entry.content_len())
                .await?;
        }
        Ok(())
    }

    async fn apply_key(self: &Arc<Self>, lib: &str, key: &[u8]) -> Result<()> {
        let doc = self.lib(lib)?.doc.clone();
        if let Some(e) = doc
            .get_one(Query::single_latest_per_key().key_exact(key))
            .await?
        {
            self.apply_entry(lib, key, e.content_hash(), e.content_len())
                .await?;
        }
        Ok(())
    }

    async fn apply_entry(
        self: &Arc<Self>,
        lib: &str,
        key: &[u8],
        hash: Hash,
        len: u64,
    ) -> Result<()> {
        let Some(k) = Key::parse(key) else {
            return Ok(());
        };
        let value = if len == 0 {
            None
        } else {
            match self.store.blobs().get_bytes(hash).await {
                Ok(b) => Some(b.to_vec()),
                Err(_) => {
                    self.pending_content
                        .lock()
                        .unwrap()
                        .entry(hash)
                        .or_default()
                        .push((lib.to_string(), key.to_vec()));
                    return Ok(());
                }
            }
        };
        self.apply(lib, &k, value.as_deref()).await?;
        self.ix().set_entry(lib, key, &hash.to_string())?;
        Ok(())
    }

    pub async fn apply(self: &Arc<Self>, lib: &str, key: &Key, value: Option<&[u8]>) -> Result<()> {
        match (key, value) {
            (Key::Meta, Some(v)) => {
                if let Some(m) = model::decode::<Meta>(v) {
                    self.ix().set_library(lib, Some(&m.name))?;
                }
            }
            (Key::Asset(h), Some(v)) => {
                if let Some(a) = model::decode::<model::Asset>(v) {
                    self.ix().put_asset(lib, h, &a)?;
                    if self.store.blobs().has(a.thumb_s).await.unwrap_or(false) {
                        self.ix().set_thumb_flag(lib, h, false)?;
                    }
                    if self.store.blobs().has(a.thumb_l).await.unwrap_or(false) {
                        self.ix().set_thumb_flag(lib, h, true)?;
                    }
                    self.wake.notify_one();
                }
            }
            (Key::Favorite(h), Some(v)) => {
                if let Some(f) = model::decode::<Favorite>(v) {
                    self.ix().set_favorite(lib, h, f.favorite)?;
                }
            }
            (Key::Tombstone(h), Some(v)) => {
                if let Some(t) = model::decode::<Tombstone>(v) {
                    self.ix().set_tombstone(lib, h, t.deleted_at, t.restored)?;
                    if t.restored {
                        self.local_restore(lib, h).await?;
                    } else {
                        self.local_trash(lib, h).await?;
                    }
                }
            }
            (Key::Holding(dev, h), v) => {
                self.ix().set_holding(lib, dev, h, v.is_some())?;
                self.wake.notify_one();
            }
            (Key::Device(id), Some(v)) => {
                if let Some(d) = model::decode::<Device>(v) {
                    self.ix().put_device(lib, id, &d)?;
                }
            }
            _ => {}
        }
        self.touch(lib);
        Ok(())
    }

    pub async fn put(self: &Core, lib: &str, key: Key, value: Vec<u8>) -> Result<()> {
        let l = self.lib(lib)?;
        if !l.writable {
            bail!("library is read-only");
        }
        let hash = l.doc.set_bytes(self.author, key.bytes(), value).await?;
        self.ix().set_entry(lib, &key.bytes(), &hash.to_string())?;
        Ok(())
    }

    pub async fn put_apply(self: &Arc<Self>, lib: &str, key: Key, value: Vec<u8>) -> Result<()> {
        self.put(lib, key.clone(), value.clone()).await?;
        self.apply(lib, &key, Some(&value)).await
    }

    pub async fn set_holding(self: &Arc<Self>, lib: &str, hash: &Hash, held: bool) -> Result<()> {
        if !self.lib(lib)?.writable {
            return Ok(());
        }
        let already = self.ix().holders(lib, hash)?.contains(&self.me);
        if already == held {
            return Ok(());
        }
        let key = Key::Holding(self.me.clone(), *hash);
        if held {
            self.put(lib, key, model::HOLDING.to_vec()).await?;
        } else {
            self.lib(lib)?.doc.del(self.author, key.bytes()).await?;
        }
        self.ix().set_holding(lib, &self.me, hash, held)?;
        Ok(())
    }

    pub fn roots(&self) -> Vec<RootInfo> {
        self.ix()
            .roots()
            .unwrap_or_default()
            .into_iter()
            .map(|(library, path, files)| RootInfo {
                online: Path::new(&path).is_dir(),
                library,
                path,
                files,
            })
            .collect()
    }

    pub fn timeline(&self, lib: &str, filter: Filter) -> Vec<AssetSummary> {
        let mut out = self.ix().timeline(lib, filter).unwrap_or_default();
        if filter == Filter::Deleted {
            let cutoff = now() - TRASH_DAYS * 86400;
            let ix = self.ix();
            out.retain(|a| {
                parse_hash(&a.hash)
                    .ok()
                    .and_then(|h| ix.deleted_at(lib, &h))
                    .is_some_and(|d| d > cutoff)
            });
        }
        out
    }

    pub fn detail(&self, lib: &str, hash: &str) -> Option<AssetDetail> {
        let h = parse_hash(hash).ok()?;
        let ix = self.ix();
        let a = ix.asset(lib, &h)?;
        let name = |id: &str| {
            ix.device_name(id)
                .unwrap_or_else(|| id[..8.min(id.len())].to_string())
        };
        let summary = ix
            .timeline(lib, Filter::All)
            .ok()?
            .into_iter()
            .chain(ix.timeline(lib, Filter::Deleted).ok()?)
            .find(|s| s.hash == hash)?;
        let path = ix
            .paths_of(lib, &h)
            .ok()?
            .into_iter()
            .find(|p| Path::new(p).exists())
            .or_else(|| ix.cached(lib, &h));
        Some(AssetDetail {
            hash: hash.to_string(),
            kind: a.kind,
            taken_at: a.taken_at,
            w: a.w,
            h: a.h,
            duration_ms: a.duration_ms,
            size: a.size,
            name: a.name,
            mime: a.mime,
            camera: a.camera,
            favorite: summary.favorite,
            deleted: ix.deleted_at(lib, &h).is_some(),
            live: summary.live,
            origin: name(&a.origin),
            holders: ix.holders(lib, &h).ok()?.iter().map(|d| name(d)).collect(),
            path,
        })
    }

    pub async fn thumbnail(&self, lib: &str, hash: &str, large: bool) -> Option<Vec<u8>> {
        let h = parse_hash(hash).ok()?;
        let a = self.ix().asset(lib, &h)?;
        let t = if large { a.thumb_l } else { a.thumb_s };
        self.store
            .blobs()
            .get_bytes(t)
            .await
            .ok()
            .map(|b| b.to_vec())
    }

    pub fn original(&self, lib: &str, hash: &str) -> Option<String> {
        let h = parse_hash(hash).ok()?;
        let local = {
            let ix = self.ix();
            ix.paths_of(lib, &h)
                .ok()?
                .into_iter()
                .find(|p| Path::new(p).exists())
                .or_else(|| ix.cached(lib, &h).filter(|p| Path::new(p).exists()))
        };
        if local.is_none() {
            self.urgent.lock().unwrap().push((lib.to_string(), h));
            self.wake.notify_one();
        }
        local
    }

    pub async fn set_favorite(
        self: &Arc<Self>,
        lib: &str,
        hash: &str,
        favorite: bool,
    ) -> Result<()> {
        let h = parse_hash(hash)?;
        self.put_apply(lib, Key::Favorite(h), model::encode(&Favorite { favorite }))
            .await
    }

    pub async fn set_deleted(
        self: &Arc<Self>,
        lib: &str,
        hashes: &[String],
        deleted: bool,
    ) -> Result<()> {
        for hash in hashes {
            let h = parse_hash(hash)?;
            let t = Tombstone {
                deleted_at: if deleted {
                    now()
                } else {
                    self.ix().deleted_at(lib, &h).unwrap_or(0)
                },
                restored: !deleted,
            };
            self.put_apply(lib, Key::Tombstone(h), model::encode(&t))
                .await?;
        }
        Ok(())
    }

    pub fn devices(&self, lib: &str) -> Vec<DeviceInfo> {
        let online = self
            .online
            .lock()
            .unwrap()
            .get(lib)
            .cloned()
            .unwrap_or_default();
        let ix = self.ix();
        let total = ix.asset_count(lib);
        ix.devices(lib)
            .unwrap_or_default()
            .into_iter()
            .map(|(id, d, held)| DeviceInfo {
                online: online.contains(&id) || id == self.me,
                this_device: id == self.me,
                id,
                name: d.name,
                platform: d.platform,
                originals_all: d.originals_all,
                archive: d.archive,
                held,
                total,
                last_seen: d.last_seen,
            })
            .collect()
    }

    pub fn status(&self) -> Status {
        let (thumbs_pending, originals_pending) = self.ix().pending_counts().unwrap_or((0, 0));
        let peers: HashSet<String> = self
            .online
            .lock()
            .unwrap()
            .values()
            .flat_map(|s| s.iter().cloned())
            .collect();
        Status {
            indexing: self.indexing.load(Ordering::Relaxed),
            thumbs_pending,
            originals_pending: if self.policy().originals_all {
                originals_pending
            } else {
                0
            },
            paused: self.paused.lock().unwrap().clone(),
            peers_online: peers.len() as u64,
        }
    }
}

fn display_name(name: Option<String>, id: &str) -> String {
    name.unwrap_or_else(|| format!("library-{}", &id[..6.min(id.len())]))
}

pub fn sanitize(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c == '/' || c == '\\' || c == ':' || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    let s = s.trim().trim_start_matches('.').to_string();
    if s.is_empty() { "library".into() } else { s }
}
