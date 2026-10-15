//! On-disk JSON cache under `.revlens/cache/`. A key covers everything the cached value depends
//! on, so a hit is always valid; a missing, unreadable or corrupt entry is a miss, never an error.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::hash::Fnv;

pub const DIR: &str = ".revlens/cache";

/// `<kind>-<16 hex>` of the hashed parts plus the crate version.
pub fn key(kind: &str, parts: &[&[u8]]) -> String {
    let mut h = Fnv::default();
    h.field(env!("CARGO_PKG_VERSION").as_bytes());
    for p in parts {
        h.field(p);
    }
    format!("{kind}-{:016x}", h.finish())
}

fn path(root: &Path, key: &str) -> PathBuf {
    root.join(DIR).join(format!("{key}.json"))
}

pub fn get<T: DeserializeOwned>(root: &Path, key: &str) -> Option<T> {
    let bytes = std::fs::read(path(root, key)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Atomic write (tmp file + rename).
pub fn put<T: Serialize>(root: &Path, key: &str, value: &T) -> Result<()> {
    let p = path(root, key);
    let dir = p.parent().context("cache path has no parent")?;
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let tmp = p.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(
        &tmp,
        serde_json::to_vec(value).context("serializing cache entry")?,
    )
    .with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, &p).with_context(|| format!("renaming to {}", p.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_miss_and_corruption() {
        let dir = tempfile::tempdir().expect("tmp");
        let k = key("t", &[b"a", b"b"]);
        assert_ne!(k, key("t", &[b"ab"]));
        assert!(k.starts_with("t-") && k.len() == 18);
        assert_eq!(get::<Vec<u32>>(dir.path(), &k), None);
        put(dir.path(), &k, &vec![1u32, 2]).expect("put");
        assert_eq!(get::<Vec<u32>>(dir.path(), &k), Some(vec![1, 2]));
        assert_eq!(get::<String>(dir.path(), &k), None, "wrong type is a miss");
        std::fs::write(path(dir.path(), &k), b"{").expect("corrupt");
        assert_eq!(get::<Vec<u32>>(dir.path(), &k), None);
    }
}
