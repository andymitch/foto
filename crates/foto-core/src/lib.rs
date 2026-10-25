mod core;
mod fetch;
mod files;
mod index;
pub mod media;
mod model;

use std::sync::Arc;

pub use index::{AssetSummary, Filter};
pub use model::Kind;

uniffi::setup_scaffolding!();

#[uniffi::export(with_foreign)]
pub trait Platform: Send + Sync {
    fn thumbnail(&self, path: String, video: bool, max_px: u32) -> Option<Vec<u8>>;
    fn trash(&self, paths: Vec<String>) -> bool;
    fn is_metered(&self) -> bool;
}

pub struct NoPlatform;

impl Platform for NoPlatform {
    fn thumbnail(&self, _: String, _: bool, _: u32) -> Option<Vec<u8>> {
        None
    }
    fn trash(&self, _: Vec<String>) -> bool {
        false
    }
    fn is_metered(&self) -> bool {
        false
    }
}

#[uniffi::export(with_foreign)]
pub trait EventSink: Send + Sync {
    fn on_event(&self, event: Event);
}

#[derive(Debug, Clone, PartialEq, uniffi::Enum)]
pub enum Event {
    LibraryChanged { library: String },
    StatusChanged { status: Status },
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct Config {
    pub data_dir: String,
    pub library_dir: String,
    pub device_name: String,
    pub platform: String,
    pub originals_all: bool,
}

impl Config {
    pub fn desktop(platform: &str) -> Config {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        let data = std::env::var("FOTO_HOME").unwrap_or_else(|_| match platform {
            "macos" => format!("{home}/Library/Application Support/foto"),
            _ => std::env::var("XDG_DATA_HOME")
                .map(|d| format!("{d}/foto"))
                .unwrap_or_else(|_| format!("{home}/.local/share/foto")),
        });
        let pictures =
            std::env::var("FOTO_PICTURES").unwrap_or_else(|_| format!("{home}/Pictures/foto"));
        let device_name = std::process::Command::new("hostname")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().trim_end_matches(".local").to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| platform.to_string());
        Config {
            data_dir: data,
            library_dir: pictures,
            device_name,
            platform: platform.to_string(),
            originals_all: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct LibraryInfo {
    pub id: String,
    pub name: String,
    pub writable: bool,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct RootInfo {
    pub library: String,
    pub path: String,
    pub online: bool,
    pub files: u64,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
    pub platform: String,
    pub originals_all: bool,
    pub archive: bool,
    pub online: bool,
    pub this_device: bool,
    pub held: u64,
    pub total: u64,
    pub last_seen: i64,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct AssetDetail {
    pub hash: String,
    pub kind: Kind,
    pub taken_at: i64,
    pub w: u32,
    pub h: u32,
    pub duration_ms: u64,
    pub size: u64,
    pub name: String,
    pub mime: String,
    pub camera: Option<String>,
    pub favorite: bool,
    pub deleted: bool,
    pub live: bool,
    pub origin: String,
    pub holders: Vec<String>,
    pub path: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, uniffi::Record)]
pub struct Status {
    pub indexing: u64,
    pub thumbs_pending: u64,
    pub originals_pending: u64,
    pub paused: Option<String>,
    pub peers_online: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, uniffi::Record)]
pub struct Policy {
    pub originals_all: bool,
    pub archive: bool,
    pub metered_ok: bool,
}

#[derive(Debug, uniffi::Error)]
#[uniffi(flat_error)]
pub enum FotoError {
    Failed(String),
}

impl std::fmt::Display for FotoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let FotoError::Failed(m) = self;
        f.write_str(m)
    }
}

impl std::error::Error for FotoError {}

impl From<anyhow::Error> for FotoError {
    fn from(e: anyhow::Error) -> Self {
        FotoError::Failed(format!("{e:#}"))
    }
}

type R<T> = Result<T, FotoError>;

#[derive(uniffi::Object)]
pub struct Foto {
    rt: tokio::runtime::Runtime,
    core: Arc<core::Core>,
}

#[uniffi::export]
impl Foto {
    #[uniffi::constructor]
    pub fn open(config: Config, platform: Arc<dyn Platform>) -> R<Arc<Self>> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("foto")
            .build()
            .map_err(|e| FotoError::Failed(e.to_string()))?;
        let core = rt.block_on(core::Core::start(config, platform))?;
        Ok(Arc::new(Foto { rt, core }))
    }

    pub fn subscribe(&self, sink: Arc<dyn EventSink>) {
        *self.core.sink.lock().unwrap() = Some(sink);
    }

    pub fn device_id(&self) -> String {
        self.core.me.clone()
    }

    pub fn device_name(&self) -> String {
        self.core.device_name()
    }

    pub fn set_device_name(&self, name: String) -> R<()> {
        Ok(self.rt.block_on(self.core.set_device_name(name))?)
    }

    pub fn policy(&self) -> Policy {
        self.core.policy()
    }

    pub fn set_policy(&self, policy: Policy) -> R<()> {
        Ok(self.rt.block_on(self.core.set_policy(policy))?)
    }

    pub fn libraries(&self) -> Vec<LibraryInfo> {
        self.core.libraries()
    }

    pub fn create_library(&self, name: String) -> R<LibraryInfo> {
        Ok(self.rt.block_on(self.core.create_library(name))?)
    }

    pub fn join(&self, ticket: String) -> R<LibraryInfo> {
        Ok(self.rt.block_on(self.core.join(ticket))?)
    }

    pub fn share(&self, library: String, writable: bool) -> R<String> {
        Ok(self.rt.block_on(self.core.share(&library, writable))?)
    }

    pub fn library_dir(&self, library: String) -> String {
        self.core
            .library_dir(&library)
            .to_string_lossy()
            .into_owned()
    }

    pub fn roots(&self) -> Vec<RootInfo> {
        self.core.roots()
    }

    pub fn add_root(&self, library: String, path: String) -> R<()> {
        Ok(self.rt.block_on(self.core.add_root(&library, &path))?)
    }

    pub fn remove_root(&self, library: String, path: String) -> R<()> {
        Ok(self.rt.block_on(self.core.remove_root(&library, &path))?)
    }

    pub fn scan(&self) {
        let core = self.core.clone();
        self.rt.spawn(async move { core.scan_all().await });
    }

    pub fn scan_now(&self) -> R<()> {
        self.rt.block_on(self.core.scan_all());
        Ok(())
    }

    pub fn timeline(&self, library: String, filter: Filter) -> Vec<AssetSummary> {
        self.core.timeline(&library, filter)
    }

    pub fn asset(&self, library: String, hash: String) -> Option<AssetDetail> {
        self.core.detail(&library, &hash)
    }

    pub fn thumbnail(&self, library: String, hash: String, large: bool) -> Option<Vec<u8>> {
        self.rt
            .block_on(self.core.thumbnail(&library, &hash, large))
    }

    pub fn original(&self, library: String, hash: String) -> Option<String> {
        self.core.original(&library, &hash)
    }

    pub fn save_original(&self, library: String, hash: String) -> R<Option<String>> {
        Ok(self.rt.block_on(self.core.save_original(&library, &hash))?)
    }

    pub fn set_favorite(&self, library: String, hash: String, favorite: bool) -> R<()> {
        Ok(self
            .rt
            .block_on(self.core.set_favorite(&library, &hash, favorite))?)
    }

    pub fn delete(&self, library: String, hashes: Vec<String>) -> R<()> {
        Ok(self
            .rt
            .block_on(self.core.set_deleted(&library, &hashes, true))?)
    }

    pub fn restore(&self, library: String, hashes: Vec<String>) -> R<()> {
        Ok(self
            .rt
            .block_on(self.core.set_deleted(&library, &hashes, false))?)
    }

    pub fn devices(&self, library: String) -> Vec<DeviceInfo> {
        self.core.devices(&library)
    }

    pub fn status(&self) -> Status {
        self.core.status()
    }

    pub fn pending_trash(&self) -> Vec<String> {
        self.core.pending_trash()
    }

    pub fn trash_done(&self, paths: Vec<String>, confirmed: bool) -> R<()> {
        Ok(self.rt.block_on(self.core.trash_done(paths, confirmed))?)
    }

    pub fn network_changed(&self) {
        let core = self.core.clone();
        self.rt.spawn(async move { core.redial().await });
    }

    pub fn shutdown(&self) {
        self.rt.block_on(self.core.shutdown());
    }
}
