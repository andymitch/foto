use iroh_blobs::Hash;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, uniffi::Enum)]
pub enum Kind {
    Photo,
    Video,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Meta {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Asset {
    pub kind: Kind,
    pub taken_at: i64,
    pub mime: String,
    pub w: u32,
    pub h: u32,
    pub duration_ms: u64,
    pub size: u64,
    pub name: String,
    pub camera: Option<String>,
    pub thumb_s: Hash,
    pub thumb_l: Hash,
    pub pair: Option<Hash>,
    pub motion: bool,
    pub origin: String,
    pub added_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Favorite {
    pub favorite: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Tombstone {
    pub deleted_at: i64,
    pub restored: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Device {
    pub name: String,
    pub platform: String,
    pub originals_all: bool,
    pub archive: bool,
    pub last_seen: i64,
}

pub fn encode<T: Serialize>(v: &T) -> Vec<u8> {
    postcard::to_stdvec(v).expect("postcard encode")
}

pub fn decode<T: DeserializeOwned>(b: &[u8]) -> Option<T> {
    postcard::from_bytes(b).ok()
}

pub fn hash_from(s: &str) -> Option<Hash> {
    (s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| s.parse().ok())
        .flatten()
}

pub const META: &[u8] = b"meta";
pub const HOLDING: &[u8] = &[1];

#[derive(Debug, Clone, PartialEq)]
pub enum Key {
    Meta,
    Asset(Hash),
    Favorite(Hash),
    Tombstone(Hash),
    Holding(String, Hash),
    Device(String),
}

impl Key {
    pub fn bytes(&self) -> Vec<u8> {
        match self {
            Key::Meta => META.to_vec(),
            Key::Asset(h) => format!("a/{h}").into_bytes(),
            Key::Favorite(h) => format!("f/{h}").into_bytes(),
            Key::Tombstone(h) => format!("x/{h}").into_bytes(),
            Key::Holding(d, h) => format!("h/{d}/{h}").into_bytes(),
            Key::Device(d) => format!("d/{d}").into_bytes(),
        }
    }

    pub fn parse(b: &[u8]) -> Option<Key> {
        if b == META {
            return Some(Key::Meta);
        }
        let s = std::str::from_utf8(b).ok()?;
        let (kind, rest) = s.split_once('/')?;
        let hash = hash_from;
        Some(match kind {
            "a" => Key::Asset(hash(rest)?),
            "f" => Key::Favorite(hash(rest)?),
            "x" => Key::Tombstone(hash(rest)?),
            "h" => {
                let (d, h) = rest.split_once('/')?;
                Key::Holding(d.to_string(), hash(h)?)
            }
            "d" => Key::Device(rest.to_string()),
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_roundtrip() {
        let h = Hash::new(b"x");
        for k in [
            Key::Meta,
            Key::Asset(h),
            Key::Favorite(h),
            Key::Tombstone(h),
            Key::Holding("dev".into(), h),
            Key::Device("dev".into()),
        ] {
            assert_eq!(Key::parse(&k.bytes()), Some(k));
        }
        assert_eq!(Key::parse(b"zz/1"), None);
        assert_eq!(Key::parse(b"a/nothash"), None);
    }
}
