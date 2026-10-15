//! Carry review state across a force-push or rebase.
//!
//! Sessions are keyed by commits, so a rewritten branch opens an empty session. Next to every
//! session file Lens writes `<id>.meta.json`: the review's source (range text or PR number plus
//! the criteria) and each hunk's anchor, a hash of its path and changed lines without line
//! numbers. When a new session has no file yet, the newest earlier session with the same source
//! is migrated: criterion status carries over as is, and marks, mappings and claims carry over
//! for hunks whose anchor still exists. Everything else is dropped and counted.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::hash::Fnv;
use crate::live::Session;
use crate::model::{Analysis, Hunk, HunkId, LineKind, SessionState};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Meta {
    /// Hash of the range text (or `pr:N`) and the criteria texts.
    pub source: String,
    /// Hunk id -> content anchor.
    pub anchors: HashMap<HunkId, String>,
    /// Session file this one was migrated from, if any.
    #[serde(default)]
    pub migrated_from: Option<String>,
}

/// Position-free identity of a hunk: path plus changed lines (trailing whitespace trimmed).
pub fn anchor(h: &Hunk) -> String {
    let mut f = Fnv::default();
    f.field(h.path.as_bytes());
    for l in &h.lines {
        let m = match l.kind {
            LineKind::Context => continue,
            LineKind::Added => b'+',
            LineKind::Removed => b'-',
        };
        f.field(&[m]).field(l.text.trim_end().as_bytes());
    }
    format!("a_{:016x}", f.finish())
}

/// What the user asked to review, independent of the commits it resolves to.
pub fn source_key(range: Option<&str>, pr: Option<u32>, a: &Analysis) -> String {
    let mut f = Fnv::default();
    match pr {
        Some(n) => f.field(format!("pr:{n}").as_bytes()),
        None => f.field(range.unwrap_or_default().as_bytes()),
    };
    for c in &a.criteria {
        f.field(c.text.as_bytes());
    }
    format!("{:016x}", f.finish())
}

pub fn meta_path(session: &Path) -> PathBuf {
    session.with_extension("meta.json")
}

fn meta_for(source: &str, a: &Analysis, from: Option<String>) -> Meta {
    Meta {
        source: source.to_string(),
        anchors: a.hunks.iter().map(|h| (h.id.clone(), anchor(h))).collect(),
        migrated_from: from,
    }
}

fn write_meta(session: &Path, meta: &Meta) -> Result<()> {
    let p = meta_path(session);
    let text = serde_json::to_string(meta).context("serializing session meta")?;
    std::fs::write(&p, text).with_context(|| format!("writing {}", p.display()))
}

fn read_meta(path: &Path) -> Option<Meta> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

/// Outcome of a migration, for the message shown to the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub from: PathBuf,
    /// Hunks of the old review that had any state, and how many of those were found again.
    pub carried: usize,
    pub had_state: usize,
}

/// Make sure `session` has a meta file; when the session itself does not exist yet, migrate
/// the newest earlier session of the same source into it.
pub fn prepare(session: &Path, source: &str, a: &Analysis) -> Result<Option<Report>> {
    if session.exists() {
        if read_meta(&meta_path(session)).is_none() {
            write_meta(session, &meta_for(source, a, None))?;
        }
        return Ok(None);
    }
    let Some((old_path, old_meta)) = latest_sibling(session, source) else {
        // `export` without a session directory leaves the tree untouched.
        if session.parent().is_some_and(Path::exists) {
            write_meta(session, &meta_for(source, a, None))?;
        }
        return Ok(None);
    };
    let old = Session::open(&old_path)?.snapshot()?;
    let (state, report) = migrate(&old, &old_meta, a, &old_path);
    let s = Session::open(session)?;
    s.import(&state);
    s.persist()?;
    let name = old_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned());
    write_meta(session, &meta_for(source, a, name))?;
    Ok(Some(report))
}

/// Newest `*.ydoc` in the same directory whose meta has the same source.
fn latest_sibling(session: &Path, source: &str) -> Option<(PathBuf, Meta)> {
    let dir = session.parent()?;
    let mut best: Option<(SystemTime, PathBuf, Meta)> = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let p = entry.path();
        if p == session || p.extension().and_then(|e| e.to_str()) != Some("ydoc") {
            continue;
        }
        let Some(meta) = read_meta(&meta_path(&p)).filter(|m| m.source == source) else {
            continue;
        };
        let t = entry
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        if best.as_ref().is_none_or(|(bt, _, _)| t > *bt) {
            best = Some((t, p, meta));
        }
    }
    best.map(|(_, p, m)| (p, m))
}

/// Old state re-keyed onto the new analysis' hunk ids.
pub fn migrate(
    old: &SessionState,
    old_meta: &Meta,
    a: &Analysis,
    from: &Path,
) -> (SessionState, Report) {
    let by_anchor: HashMap<String, &HunkId> = a.hunks.iter().map(|h| (anchor(h), &h.id)).collect();
    let remap = |old_id: &str| -> Option<HunkId> {
        let key = old_meta.anchors.get(old_id)?;
        by_anchor.get(key).map(|id| (*id).clone())
    };
    let mut state = SessionState {
        status: old.status.clone(),
        ..SessionState::default()
    };
    let mut touched: Vec<&str> = Vec::new();
    for m in &old.mappings {
        touched.push(&m.hunk);
        if let Some(h) = remap(&m.hunk) {
            let mut m2 = m.clone();
            m2.hunk = h;
            state.mappings.push(m2);
        }
    }
    for (h, user) in &old.reviewed {
        touched.push(h);
        if let Some(n) = remap(h) {
            state.reviewed.insert(n, user.clone());
        }
    }
    for (h, c) in &old.claims {
        touched.push(h);
        if let Some(n) = remap(h) {
            state.claims.insert(n, c.clone());
        }
    }
    state.mappings.sort();
    touched.sort_unstable();
    touched.dedup();
    let carried = touched.iter().filter(|h| remap(h).is_some()).count();
    let report = Report {
        from: from.to_path_buf(),
        carried,
        had_state: touched.len(),
    };
    (state, report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Claim, CritStatus, Criterion, Line, Mapping, Range};
    use std::collections::BTreeMap;

    fn hunk(id: &str, path: &str, start: u32, added: &str) -> Hunk {
        Hunk {
            id: id.into(),
            path: path.into(),
            old_start: start,
            old_len: 0,
            new_start: start,
            new_len: 1,
            lines: vec![Line {
                kind: LineKind::Added,
                text: added.into(),
            }],
            kind: crate::model::HunkKind::New,
            symbols: vec![],
            signals: vec![],
            score: 1,
        }
    }

    fn analysis(hunks: Vec<Hunk>) -> Analysis {
        Analysis {
            range: Range {
                base: "b".into(),
                head: "h".into(),
                merge_base: "m".into(),
            },
            files: vec![],
            hunks,
            symbols: vec![],
            criteria: vec![Criterion {
                id: "c1".into(),
                text: "Fee".into(),
                source_line: 1,
            }],
            assumptions: vec![],
        }
    }

    #[test]
    fn anchor_ignores_position_but_not_content() {
        assert_eq!(
            anchor(&hunk("a", "x.py", 1, "y")),
            anchor(&hunk("b", "x.py", 40, "y "))
        );
        assert_ne!(
            anchor(&hunk("a", "x.py", 1, "y")),
            anchor(&hunk("a", "x.py", 1, "z"))
        );
        assert_ne!(
            anchor(&hunk("a", "x.py", 1, "y")),
            anchor(&hunk("a", "w.py", 1, "y"))
        );
    }

    #[test]
    fn source_key_depends_on_range_or_pr_and_criteria() {
        let a = analysis(vec![]);
        let k = source_key(Some("main..feat"), None, &a);
        assert_eq!(k, source_key(Some("main..feat"), None, &a));
        assert_ne!(k, source_key(Some("main..other"), None, &a));
        assert_ne!(k, source_key(None, Some(3), &a));
        let mut b = a.clone();
        b.criteria[0].text = "Other".into();
        assert_ne!(k, source_key(Some("main..feat"), None, &b));
    }

    #[test]
    fn migrate_carries_matching_hunks_and_drops_the_rest() {
        let before = analysis(vec![
            hunk("h_old1", "a.py", 3, "fee = 1"),
            hunk("h_old2", "b.py", 1, "gone"),
        ]);
        let meta = meta_for("s", &before, None);
        let old = SessionState {
            status: BTreeMap::from([("c1".into(), CritStatus::Met)]),
            mappings: vec![
                Mapping {
                    criterion: "c1".into(),
                    hunk: "h_old1".into(),
                    by: "ann".into(),
                },
                Mapping {
                    criterion: "c1".into(),
                    hunk: "h_old2".into(),
                    by: "ann".into(),
                },
            ],
            reviewed: BTreeMap::from([("h_old1".into(), "bob".into())]),
            claims: BTreeMap::from([(
                "h_old2".into(),
                Claim {
                    user: "cy".into(),
                    at_ms: 1,
                },
            )]),
        };
        // After a rebase the fee hunk moved down; the other one changed.
        let after = analysis(vec![
            hunk("h_new1", "a.py", 9, "fee = 1"),
            hunk("h_new2", "b.py", 1, "edited"),
        ]);
        let (state, rep) = migrate(&old, &meta, &after, Path::new("old.ydoc"));
        assert_eq!(state.status["c1"], CritStatus::Met);
        assert_eq!(state.mappings.len(), 1);
        assert_eq!(state.mappings[0].hunk, "h_new1");
        assert_eq!(
            state.reviewed.get("h_new1").map(String::as_str),
            Some("bob")
        );
        assert!(state.claims.is_empty());
        assert_eq!((rep.carried, rep.had_state), (1, 2));
    }

    #[test]
    fn prepare_migrates_from_the_newest_matching_sibling() {
        let dir = tempfile::tempdir().expect("tmp");
        let before = analysis(vec![hunk("h_old1", "a.py", 3, "fee = 1")]);
        let old_path = dir.path().join("s_old.ydoc");
        assert_eq!(prepare(&old_path, "src", &before).expect("first"), None);
        assert!(
            meta_path(&old_path).exists(),
            "meta written for a new session"
        );
        let s = Session::open(&old_path).expect("open");
        s.import(&SessionState {
            reviewed: BTreeMap::from([("h_old1".into(), "bob".into())]),
            ..SessionState::default()
        });
        s.persist().expect("persist");
        // A session of another source is never a migration source.
        let other = dir.path().join("s_other.ydoc");
        prepare(&other, "different", &before).expect("other");
        Session::open(&other).expect("o").persist().expect("p");

        let after = analysis(vec![hunk("h_new1", "a.py", 7, "fee = 1")]);
        let new_path = dir.path().join("s_new.ydoc");
        let rep = prepare(&new_path, "src", &after)
            .expect("migrate")
            .expect("report");
        assert_eq!(rep.from, old_path);
        assert_eq!((rep.carried, rep.had_state), (1, 1));
        let got = Session::open(&new_path)
            .expect("reopen")
            .snapshot()
            .expect("snap");
        assert_eq!(got.reviewed.get("h_new1").map(String::as_str), Some("bob"));
        let meta = read_meta(&meta_path(&new_path)).expect("meta");
        assert_eq!(meta.migrated_from.as_deref(), Some("s_old.ydoc"));
        // Existing session: nothing to migrate.
        assert_eq!(prepare(&new_path, "src", &after).expect("again"), None);
    }
}
