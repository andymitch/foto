use std::path::Path;

use anyhow::Result;
use iroh_blobs::Hash;
use rusqlite::{Connection, OptionalExtension, params};

use crate::model::{Asset, Device, Kind};

const SCHEMA_VERSION: i64 = 1;

const SCHEMA: &str = "
CREATE TABLE settings(k TEXT PRIMARY KEY, v TEXT NOT NULL);
CREATE TABLE libraries(lib TEXT PRIMARY KEY, name TEXT);
CREATE TABLE entries(lib TEXT, key BLOB, hash TEXT, PRIMARY KEY(lib, key));
CREATE TABLE assets(
  lib TEXT, hash TEXT, kind INTEGER, taken_at INTEGER, w INTEGER, h INTEGER, duration_ms INTEGER,
  size INTEGER, name TEXT, mime TEXT, camera TEXT, thumb_s TEXT, thumb_l TEXT, pair TEXT,
  motion INTEGER, origin TEXT, favorite INTEGER DEFAULT 0, deleted_at INTEGER,
  has_thumb_s INTEGER DEFAULT 0, has_thumb_l INTEGER DEFAULT 0,
  PRIMARY KEY(lib, hash));
CREATE INDEX assets_time ON assets(lib, taken_at DESC);
CREATE TABLE tombstones(lib TEXT, hash TEXT, deleted_at INTEGER, restored INTEGER, PRIMARY KEY(lib, hash));
CREATE TABLE favorites(lib TEXT, hash TEXT, favorite INTEGER, PRIMARY KEY(lib, hash));
CREATE TABLE roots(lib TEXT, path TEXT, PRIMARY KEY(lib, path));
CREATE TABLE paths(path TEXT PRIMARY KEY, lib TEXT, hash TEXT, size INTEGER, mtime INTEGER);
CREATE INDEX paths_hash ON paths(lib, hash);
CREATE TABLE cache(lib TEXT, hash TEXT, path TEXT, size INTEGER, used INTEGER, PRIMARY KEY(lib, hash));
CREATE TABLE holdings(lib TEXT, device TEXT, hash TEXT, PRIMARY KEY(lib, device, hash));
CREATE TABLE devices(lib TEXT, id TEXT, name TEXT, platform TEXT, originals_all INTEGER, archive INTEGER, last_seen INTEGER, PRIMARY KEY(lib, id));
CREATE TABLE peers(lib TEXT, id TEXT, PRIMARY KEY(lib, id));
CREATE TABLE trashed(path TEXT PRIMARY KEY, lib TEXT, hash TEXT, trash_path TEXT, at INTEGER);
CREATE TABLE pending_trash(path TEXT PRIMARY KEY, lib TEXT, hash TEXT);
";

pub struct Index {
    db: Connection,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct AssetSummary {
    pub hash: String,
    pub kind: Kind,
    pub taken_at: i64,
    pub w: u32,
    pub h: u32,
    pub duration_ms: u64,
    pub favorite: bool,
    pub live: bool,
    pub local: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum Filter {
    All,
    Favorites,
    Deleted,
}

#[derive(Debug, Clone)]
pub struct Want {
    pub lib: String,
    pub hash: Hash,
    pub size: u64,
}

pub fn h(s: String) -> Hash {
    s.parse().unwrap_or(Hash::EMPTY)
}

impl Index {
    pub fn open(path: &Path) -> Result<Index> {
        let mut db = Connection::open(path)?;
        db.pragma_update(None, "journal_mode", "WAL")?;
        db.pragma_update(None, "synchronous", "NORMAL")?;
        let version: i64 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version != SCHEMA_VERSION {
            let tx = db.transaction()?;
            let tables: Vec<String> = tx
                .prepare("SELECT name FROM sqlite_master WHERE type='table'")?
                .query_map([], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            for t in tables {
                tx.execute_batch(&format!("DROP TABLE \"{t}\""))?;
            }
            tx.execute_batch(SCHEMA)?;
            tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
            tx.commit()?;
        }
        Ok(Index { db })
    }

    pub fn setting(&self, k: &str) -> Option<String> {
        self.db
            .query_row("SELECT v FROM settings WHERE k=?1", [k], |r| r.get(0))
            .optional()
            .ok()
            .flatten()
    }

    pub fn set_setting(&self, k: &str, v: &str) -> Result<()> {
        self.db.execute(
            "INSERT INTO settings(k,v) VALUES(?1,?2) ON CONFLICT(k) DO UPDATE SET v=?2",
            [k, v],
        )?;
        Ok(())
    }

    pub fn set_library(&self, lib: &str, name: Option<&str>) -> Result<()> {
        self.db.execute(
            "INSERT INTO libraries(lib,name) VALUES(?1,?2) ON CONFLICT(lib) DO UPDATE SET name=coalesce(?2,name)",
            params![lib, name],
        )?;
        Ok(())
    }

    pub fn library_name(&self, lib: &str) -> Option<String> {
        self.db
            .query_row("SELECT name FROM libraries WHERE lib=?1", [lib], |r| {
                r.get::<_, Option<String>>(0)
            })
            .optional()
            .ok()
            .flatten()
            .flatten()
    }

    pub fn entry_hashes(&self, lib: &str) -> Result<std::collections::HashMap<Vec<u8>, String>> {
        let mut st = self
            .db
            .prepare("SELECT key, hash FROM entries WHERE lib=?1")?;
        let rows = st.query_map([lib], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn set_entry(&self, lib: &str, key: &[u8], hash: &str) -> Result<()> {
        self.db.execute(
            "INSERT INTO entries(lib,key,hash) VALUES(?1,?2,?3) ON CONFLICT(lib,key) DO UPDATE SET hash=?3",
            params![lib, key, hash],
        )?;
        Ok(())
    }

    pub fn put_asset(&self, lib: &str, hash: &Hash, a: &Asset) -> Result<()> {
        let tomb: Option<i64> = self
            .db
            .query_row(
                "SELECT deleted_at FROM tombstones WHERE lib=?1 AND hash=?2 AND restored=0",
                params![lib, hash.to_string()],
                |r| r.get(0),
            )
            .optional()?;
        let fav: bool = self
            .db
            .query_row(
                "SELECT favorite FROM favorites WHERE lib=?1 AND hash=?2",
                params![lib, hash.to_string()],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(false);
        self.db.execute(
            "INSERT INTO assets(lib,hash,kind,taken_at,w,h,duration_ms,size,name,mime,camera,thumb_s,thumb_l,pair,motion,origin,favorite,deleted_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18)
             ON CONFLICT(lib,hash) DO UPDATE SET kind=?3,taken_at=?4,w=?5,h=?6,duration_ms=?7,size=?8,name=?9,mime=?10,
               camera=?11,thumb_s=?12,thumb_l=?13,pair=?14,motion=?15,origin=?16",
            params![
                lib,
                hash.to_string(),
                a.kind as i64,
                a.taken_at,
                a.w,
                a.h,
                a.duration_ms as i64,
                a.size as i64,
                a.name,
                a.mime,
                a.camera,
                a.thumb_s.to_string(),
                a.thumb_l.to_string(),
                a.pair.map(|p| p.to_string()),
                a.motion,
                a.origin,
                fav,
                tomb,
            ],
        )?;
        Ok(())
    }

    pub fn asset(&self, lib: &str, hash: &Hash) -> Option<Asset> {
        self.db
            .query_row(
                "SELECT kind,taken_at,w,h,duration_ms,size,name,mime,camera,thumb_s,thumb_l,pair,motion,origin
                 FROM assets WHERE lib=?1 AND hash=?2",
                params![lib, hash.to_string()],
                |r| {
                    Ok(Asset {
                        kind: if r.get::<_, i64>(0)? == 1 { Kind::Video } else { Kind::Photo },
                        taken_at: r.get(1)?,
                        w: r.get(2)?,
                        h: r.get(3)?,
                        duration_ms: r.get::<_, i64>(4)? as u64,
                        size: r.get::<_, i64>(5)? as u64,
                        name: r.get(6)?,
                        mime: r.get(7)?,
                        camera: r.get(8)?,
                        thumb_s: h(r.get(9)?),
                        thumb_l: h(r.get(10)?),
                        pair: r.get::<_, Option<String>>(11)?.map(h),
                        motion: r.get(12)?,
                        origin: r.get(13)?,
                        added_at: 0,
                    })
                },
            )
            .optional()
            .ok()
            .flatten()
    }

    pub fn deleted_at(&self, lib: &str, hash: &Hash) -> Option<i64> {
        self.db
            .query_row(
                "SELECT deleted_at FROM assets WHERE lib=?1 AND hash=?2",
                params![lib, hash.to_string()],
                |r| r.get(0),
            )
            .optional()
            .ok()
            .flatten()
            .flatten()
    }

    pub fn set_tombstone(
        &self,
        lib: &str,
        hash: &Hash,
        deleted_at: i64,
        restored: bool,
    ) -> Result<()> {
        let hs = hash.to_string();
        self.db.execute(
            "INSERT INTO tombstones(lib,hash,deleted_at,restored) VALUES(?1,?2,?3,?4)
             ON CONFLICT(lib,hash) DO UPDATE SET deleted_at=?3, restored=?4",
            params![lib, hs, deleted_at, restored],
        )?;
        let at = (!restored).then_some(deleted_at);
        self.db.execute(
            "UPDATE assets SET deleted_at=?3 WHERE lib=?1 AND hash=?2",
            params![lib, hs, at],
        )?;
        Ok(())
    }

    pub fn set_favorite(&self, lib: &str, hash: &Hash, fav: bool) -> Result<()> {
        let hs = hash.to_string();
        self.db.execute(
            "INSERT INTO favorites(lib,hash,favorite) VALUES(?1,?2,?3) ON CONFLICT(lib,hash) DO UPDATE SET favorite=?3",
            params![lib, hs, fav],
        )?;
        self.db.execute(
            "UPDATE assets SET favorite=?3 WHERE lib=?1 AND hash=?2",
            params![lib, hs, fav],
        )?;
        Ok(())
    }

    pub fn set_thumb_flag(&self, lib: &str, hash: &Hash, large: bool) -> Result<()> {
        let col = if large { "has_thumb_l" } else { "has_thumb_s" };
        self.db.execute(
            &format!("UPDATE assets SET {col}=1 WHERE lib=?1 AND hash=?2"),
            params![lib, hash.to_string()],
        )?;
        Ok(())
    }

    pub fn timeline(&self, lib: &str, filter: Filter) -> Result<Vec<AssetSummary>> {
        let cond = match filter {
            Filter::All => "deleted_at IS NULL",
            Filter::Favorites => "deleted_at IS NULL AND favorite=1",
            Filter::Deleted => "deleted_at IS NOT NULL",
        };
        let mut st = self.db.prepare(&format!(
            "SELECT a.hash,kind,taken_at,w,h,duration_ms,favorite,pair IS NOT NULL OR motion,
               EXISTS(SELECT 1 FROM paths p WHERE p.lib=a.lib AND p.hash=a.hash)
               OR EXISTS(SELECT 1 FROM cache c WHERE c.lib=a.lib AND c.hash=a.hash)
             FROM assets a WHERE lib=?1 AND {cond} ORDER BY taken_at DESC, a.hash"
        ))?;
        let rows = st.query_map([lib], |r| {
            Ok(AssetSummary {
                hash: r.get(0)?,
                kind: if r.get::<_, i64>(1)? == 1 {
                    Kind::Video
                } else {
                    Kind::Photo
                },
                taken_at: r.get(2)?,
                w: r.get(3)?,
                h: r.get(4)?,
                duration_ms: r.get::<_, i64>(5)? as u64,
                favorite: r.get(6)?,
                live: r.get(7)?,
                local: r.get(8)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn missing_thumbs(&self, large: bool, limit: usize) -> Result<Vec<(String, Hash, Hash)>> {
        let (col, thumb) = if large {
            ("has_thumb_l", "thumb_l")
        } else {
            ("has_thumb_s", "thumb_s")
        };
        let mut st = self.db.prepare(&format!(
            "SELECT lib, hash, {thumb} FROM assets WHERE {col}=0 AND deleted_at IS NULL
             ORDER BY taken_at DESC LIMIT ?1"
        ))?;
        let rows = st.query_map([limit as i64], |r| {
            Ok((r.get(0)?, h(r.get(1)?), h(r.get(2)?)))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn missing_originals(&self, limit: usize) -> Result<Vec<Want>> {
        let mut st = self.db.prepare(
            "SELECT lib, hash, size FROM assets a WHERE deleted_at IS NULL
               AND NOT EXISTS(SELECT 1 FROM paths p WHERE p.lib=a.lib AND p.hash=a.hash)
               AND NOT EXISTS(SELECT 1 FROM trashed t WHERE t.lib=a.lib AND t.hash=a.hash)
             ORDER BY taken_at DESC LIMIT ?1",
        )?;
        let rows = st.query_map([limit as i64], |r| {
            Ok(Want {
                lib: r.get(0)?,
                hash: h(r.get(1)?),
                size: r.get::<_, i64>(2)? as u64,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn pending_counts(&self) -> Result<(u64, u64)> {
        Ok(self.db.query_row(
            "SELECT
               (SELECT count(*) FROM assets WHERE deleted_at IS NULL AND (has_thumb_s=0 OR has_thumb_l=0)),
               (SELECT count(*) FROM assets a WHERE deleted_at IS NULL
                  AND NOT EXISTS(SELECT 1 FROM paths p WHERE p.lib=a.lib AND p.hash=a.hash))",
            [],
            |r| Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)? as u64)),
        )?)
    }

    pub fn add_root(&self, lib: &str, path: &str) -> Result<()> {
        self.db.execute(
            "INSERT OR IGNORE INTO roots(lib,path) VALUES(?1,?2)",
            [lib, path],
        )?;
        Ok(())
    }

    pub fn remove_root(&self, lib: &str, path: &str) -> Result<()> {
        self.db
            .execute("DELETE FROM roots WHERE lib=?1 AND path=?2", [lib, path])?;
        self.db.execute(
            "DELETE FROM paths WHERE lib=?1 AND (path=?2 OR substr(path,1,length(?2)+1)=?2||'/')",
            [lib, path],
        )?;
        Ok(())
    }

    pub fn roots(&self) -> Result<Vec<(String, String, u64)>> {
        let mut st = self.db.prepare(
            "SELECT r.lib, r.path, (SELECT count(*) FROM paths p WHERE p.lib=r.lib
               AND substr(p.path,1,length(r.path)+1)=r.path||'/') FROM roots r ORDER BY r.path",
        )?;
        let rows = st.query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? as u64))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn path_stat(&self, path: &str) -> Option<(String, Hash, u64, i64)> {
        self.db
            .query_row(
                "SELECT lib, hash, size, mtime FROM paths WHERE path=?1",
                [path],
                |r| {
                    Ok((
                        r.get(0)?,
                        h(r.get(1)?),
                        r.get::<_, i64>(2)? as u64,
                        r.get(3)?,
                    ))
                },
            )
            .optional()
            .ok()
            .flatten()
    }

    pub fn put_path(
        &self,
        path: &str,
        lib: &str,
        hash: &Hash,
        size: u64,
        mtime: i64,
    ) -> Result<()> {
        self.db.execute(
            "INSERT INTO paths(path,lib,hash,size,mtime) VALUES(?1,?2,?3,?4,?5)
             ON CONFLICT(path) DO UPDATE SET lib=?2,hash=?3,size=?4,mtime=?5",
            params![path, lib, hash.to_string(), size as i64, mtime],
        )?;
        Ok(())
    }

    pub fn remove_path(&self, path: &str) -> Result<()> {
        self.db.execute("DELETE FROM paths WHERE path=?1", [path])?;
        Ok(())
    }

    pub fn paths_under(&self, lib: &str, root: &str) -> Result<Vec<String>> {
        let mut st = self.db.prepare(
            "SELECT path FROM paths WHERE lib=?1 AND substr(path,1,length(?2)+1)=?2||'/'",
        )?;
        let rows = st.query_map([lib, root], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn paths_of(&self, lib: &str, hash: &Hash) -> Result<Vec<String>> {
        let mut st = self
            .db
            .prepare("SELECT path FROM paths WHERE lib=?1 AND hash=?2")?;
        let rows = st.query_map(params![lib, hash.to_string()], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn cached(&self, lib: &str, hash: &Hash) -> Option<String> {
        let hs = hash.to_string();
        self.db
            .execute(
                "UPDATE cache SET used=strftime('%s','now') WHERE lib=?1 AND hash=?2",
                [lib, &hs],
            )
            .ok()?;
        self.db
            .query_row(
                "SELECT path FROM cache WHERE lib=?1 AND hash=?2",
                [lib, &hs],
                |r| r.get(0),
            )
            .optional()
            .ok()
            .flatten()
    }

    pub fn put_cache(&self, lib: &str, hash: &Hash, path: &str, size: u64) -> Result<()> {
        self.db.execute(
            "INSERT OR REPLACE INTO cache(lib,hash,path,size,used) VALUES(?1,?2,?3,?4,strftime('%s','now'))",
            params![lib, hash.to_string(), path, size as i64],
        )?;
        Ok(())
    }

    pub fn evict_cache(&self, max_bytes: u64) -> Result<Vec<(String, Hash, String)>> {
        let mut st = self
            .db
            .prepare("SELECT lib, hash, path, size FROM cache ORDER BY used DESC")?;
        let rows: Vec<(String, String, String, i64)> = st
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let mut total = 0u64;
        let mut out = Vec::new();
        for (lib, hash, path, size) in rows {
            total += size as u64;
            if total > max_bytes {
                self.db
                    .execute("DELETE FROM cache WHERE lib=?1 AND hash=?2", [&lib, &hash])?;
                out.push((lib, h(hash), path));
            }
        }
        Ok(out)
    }

    pub fn set_holding(&self, lib: &str, device: &str, hash: &Hash, held: bool) -> Result<()> {
        let sql = if held {
            "INSERT OR IGNORE INTO holdings(lib,device,hash) VALUES(?1,?2,?3)"
        } else {
            "DELETE FROM holdings WHERE lib=?1 AND device=?2 AND hash=?3"
        };
        self.db
            .execute(sql, params![lib, device, hash.to_string()])?;
        Ok(())
    }

    pub fn holders(&self, lib: &str, hash: &Hash) -> Result<Vec<String>> {
        let mut st = self
            .db
            .prepare("SELECT device FROM holdings WHERE lib=?1 AND hash=?2")?;
        let rows = st.query_map(params![lib, hash.to_string()], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn put_device(&self, lib: &str, id: &str, d: &Device) -> Result<()> {
        self.db.execute(
            "INSERT OR REPLACE INTO devices(lib,id,name,platform,originals_all,archive,last_seen) VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![lib, id, d.name, d.platform, d.originals_all, d.archive, d.last_seen],
        )?;
        self.add_peer(lib, id)
    }

    pub fn devices(&self, lib: &str) -> Result<Vec<(String, Device, u64)>> {
        let mut st = self.db.prepare(
            "SELECT id,name,platform,originals_all,archive,last_seen,
               (SELECT count(*) FROM holdings h JOIN assets a ON a.lib=h.lib AND a.hash=h.hash
                  WHERE h.lib=d.lib AND h.device=d.id AND a.deleted_at IS NULL)
             FROM devices d WHERE lib=?1 ORDER BY name",
        )?;
        let rows = st.query_map([lib], |r| {
            Ok((
                r.get(0)?,
                Device {
                    name: r.get(1)?,
                    platform: r.get(2)?,
                    originals_all: r.get(3)?,
                    archive: r.get(4)?,
                    last_seen: r.get(5)?,
                },
                r.get::<_, i64>(6)? as u64,
            ))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn device_name(&self, id: &str) -> Option<String> {
        self.db
            .query_row("SELECT name FROM devices WHERE id=?1 LIMIT 1", [id], |r| {
                r.get(0)
            })
            .optional()
            .ok()
            .flatten()
    }

    pub fn asset_count(&self, lib: &str) -> u64 {
        self.db
            .query_row(
                "SELECT count(*) FROM assets WHERE lib=?1 AND deleted_at IS NULL",
                [lib],
                |r| r.get::<_, i64>(0),
            )
            .unwrap_or(0) as u64
    }

    pub fn add_peer(&self, lib: &str, id: &str) -> Result<()> {
        self.db.execute(
            "INSERT OR IGNORE INTO peers(lib,id) VALUES(?1,?2)",
            [lib, id],
        )?;
        Ok(())
    }

    pub fn peers(&self, lib: &str) -> Result<Vec<String>> {
        let mut st = self.db.prepare("SELECT id FROM peers WHERE lib=?1")?;
        let rows = st.query_map([lib], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn put_trashed(
        &self,
        path: &str,
        lib: &str,
        hash: &Hash,
        trash_path: &str,
        at: i64,
    ) -> Result<()> {
        self.db.execute(
            "INSERT OR REPLACE INTO trashed(path,lib,hash,trash_path,at) VALUES(?1,?2,?3,?4,?5)",
            params![path, lib, hash.to_string(), trash_path, at],
        )?;
        Ok(())
    }

    pub fn trashed_of(&self, lib: &str, hash: &Hash) -> Result<Vec<(String, String)>> {
        let mut st = self
            .db
            .prepare("SELECT path, trash_path FROM trashed WHERE lib=?1 AND hash=?2")?;
        let rows = st.query_map(params![lib, hash.to_string()], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn trashed_before(&self, at: i64) -> Result<Vec<(String, String)>> {
        let mut st = self
            .db
            .prepare("SELECT path, trash_path FROM trashed WHERE at < ?1")?;
        let rows = st.query_map([at], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn remove_trashed(&self, path: &str) -> Result<()> {
        self.db
            .execute("DELETE FROM trashed WHERE path=?1", [path])?;
        Ok(())
    }

    pub fn expired_assets(&self, before: i64) -> Result<Vec<(String, Hash, Hash, Hash)>> {
        let mut st = self.db.prepare(
            "SELECT lib, hash, thumb_s, thumb_l FROM assets WHERE deleted_at IS NOT NULL AND deleted_at < ?1",
        )?;
        let rows = st.query_map([before], |r| {
            Ok((r.get(0)?, h(r.get(1)?), h(r.get(2)?), h(r.get(3)?)))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn set_pending_trash(&self, path: &str, lib: &str, hash: &Hash) -> Result<()> {
        self.db.execute(
            "INSERT OR REPLACE INTO pending_trash(path,lib,hash) VALUES(?1,?2,?3)",
            params![path, lib, hash.to_string()],
        )?;
        Ok(())
    }

    pub fn pending_trash(&self) -> Result<Vec<(String, String, Hash)>> {
        let mut st = self
            .db
            .prepare("SELECT path, lib, hash FROM pending_trash")?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, h(r.get(2)?))))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn remove_pending_trash(&self, path: &str) -> Result<()> {
        self.db
            .execute("DELETE FROM pending_trash WHERE path=?1", [path])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(taken_at: i64) -> Asset {
        Asset {
            kind: Kind::Photo,
            taken_at,
            mime: "image/jpeg".into(),
            w: 4,
            h: 3,
            duration_ms: 0,
            size: 10,
            name: "a.jpg".into(),
            camera: None,
            thumb_s: Hash::new(b"s"),
            thumb_l: Hash::new(b"l"),
            pair: None,
            motion: false,
            origin: "dev".into(),
            added_at: 0,
        }
    }

    #[test]
    fn timeline_filters_and_orders() {
        let dir = std::env::temp_dir().join(format!("foto-index-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ix = Index::open(&dir.join("i.db")).unwrap();
        let (a, b, c) = (Hash::new(b"a"), Hash::new(b"b"), Hash::new(b"c"));
        ix.put_asset("L", &a, &asset(1)).unwrap();
        ix.put_asset("L", &b, &asset(3)).unwrap();
        ix.set_tombstone("L", &c, 5, false).unwrap();
        ix.put_asset("L", &c, &asset(2)).unwrap();
        ix.set_favorite("L", &a, true).unwrap();
        let all: Vec<_> = ix
            .timeline("L", Filter::All)
            .unwrap()
            .into_iter()
            .map(|s| s.hash)
            .collect();
        assert_eq!(all, vec![b.to_string(), a.to_string()]);
        assert_eq!(ix.timeline("L", Filter::Favorites).unwrap().len(), 1);
        assert_eq!(
            ix.timeline("L", Filter::Deleted).unwrap()[0].hash,
            c.to_string()
        );
        ix.set_tombstone("L", &c, 6, true).unwrap();
        assert_eq!(ix.timeline("L", Filter::All).unwrap().len(), 3);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
