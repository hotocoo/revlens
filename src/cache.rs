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

/// Atomic write (tmp file + rename), then drop the least recently written entries beyond
/// `max_entries` (0 keeps everything).
pub fn put<T: Serialize>(root: &Path, key: &str, value: &T, max_entries: usize) -> Result<()> {
    let p = path(root, key);
    let dir = p.parent().context("cache path has no parent")?;
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let tmp = p.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(
        &tmp,
        serde_json::to_vec(value).context("serializing cache entry")?,
    )
    .with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, &p).with_context(|| format!("renaming to {}", p.display()))?;
    // Only entries of the same kind compete: `intent-*` answers must outlive analysis churn.
    let kind = key.split_once('-').map_or(key, |(k, _)| k);
    prune(dir, &format!("{kind}-"), max_entries);
    Ok(())
}

/// Best effort: a file that cannot be read or removed stays.
fn prune(dir: &Path, prefix: &str, max_entries: usize) {
    if max_entries == 0 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<(std::time::SystemTime, PathBuf)> = rd
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter(|e| e.file_name().to_string_lossy().starts_with(prefix))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    if entries.len() <= max_entries {
        return;
    }
    entries.sort();
    for (_, p) in &entries[..entries.len() - max_entries] {
        let _ = std::fs::remove_file(p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_keeps_only_the_newest_entries() {
        let dir = tempfile::tempdir().expect("tmp");
        for i in 0..5u32 {
            put(dir.path(), &format!("t-{i}"), &i, 3).expect("put");
            // Distinct mtimes on coarse filesystems.
            std::thread::sleep(std::time::Duration::from_millis(15));
        }
        let left: Vec<u32> = (0..5)
            .filter_map(|i| get(dir.path(), &format!("t-{i}")))
            .collect();
        assert_eq!(left, [2, 3, 4]);
        put(dir.path(), "other-0", &9u32, 0).expect("put");
        put(dir.path(), "t-9", &9u32, 1).expect("put");
        assert_eq!(
            get::<u32>(dir.path(), "other-0"),
            Some(9),
            "other kinds are never pruned"
        );
        assert_eq!(get::<u32>(dir.path(), "t-4"), None);
    }

    #[test]
    fn round_trip_miss_and_corruption() {
        let dir = tempfile::tempdir().expect("tmp");
        let k = key("t", &[b"a", b"b"]);
        assert_ne!(k, key("t", &[b"ab"]));
        assert!(k.starts_with("t-") && k.len() == 18);
        assert_eq!(get::<Vec<u32>>(dir.path(), &k), None);
        put(dir.path(), &k, &vec![1u32, 2], 0).expect("put");
        assert_eq!(get::<Vec<u32>>(dir.path(), &k), Some(vec![1, 2]));
        assert_eq!(get::<String>(dir.path(), &k), None, "wrong type is a miss");
        std::fs::write(path(dir.path(), &k), b"{").expect("corrupt");
        assert_eq!(get::<Vec<u32>>(dir.path(), &k), None);
    }
}
