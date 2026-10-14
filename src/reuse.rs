//! Reuse-miss: an added function whose shape (structure with names and literals abstracted)
//! equals a function that already existed at the merge base, i.e. code that could have been
//! called instead of rewritten.
//!
//! The base tree is streamed in batches and only hits are kept, so memory is O(added functions).

use std::collections::{HashMap, HashSet};

use anyhow::Result;

use crate::config::{Config, Limits};
use crate::git::Repo;
use crate::lang::{self, Def};
use crate::model::{Change, FileDiff, FileStatus, HunkId, Loc, SymKind, SymbolChange};
use crate::risk;

/// Files read per `git cat-file --batch` call; bounds peak memory to batch x `reuse_max_file_bytes`.
const BATCH_FILES: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Miss {
    /// Qualified name of the added function.
    pub name: String,
    /// Qualified name of the existing function with the same shape.
    pub existing: String,
    pub at: Loc,
}

impl Miss {
    pub fn reason(&self) -> String {
        format!(
            "new '{}' duplicates existing '{}' at {}:{}",
            self.name, self.existing, self.at.path, self.at.line
        )
    }
}

fn is_fn(kind: SymKind) -> bool {
    matches!(kind, SymKind::Function | SymKind::Method)
}

fn eligible(d: &Def, limits: &Limits) -> bool {
    is_fn(d.kind) && !d.is_test && d.nodes >= limits.reuse_min_nodes
}

struct Added {
    hunk: HunkId,
    name: String,
    shape: u64,
}

/// Added (not moved, not renamed) functions of `head`, keyed by shape, with the hunk to flag.
fn added_defs(
    symbols: &[SymbolChange],
    new: &HashMap<String, Vec<Def>>,
    limits: &Limits,
) -> Vec<Added> {
    symbols
        .iter()
        .filter(|s| s.change == Change::Added && is_fn(s.kind))
        .filter_map(|s| {
            let def = new
                .get(&s.path)?
                .iter()
                .find(|d| d.qualified == s.name && is_fn(d.kind))?;
            let hunk = s.hunks.first()?.clone();
            eligible(def, limits).then(|| Added {
                hunk,
                name: s.name.clone(),
                shape: def.shape,
            })
        })
        .collect()
}

/// Definitions the PR itself touched: not candidates for "already existed".
#[derive(Default)]
struct Touched {
    names: HashSet<(String, String)>,
    lines: HashSet<(String, u32)>,
}

impl Touched {
    fn of(symbols: &[SymbolChange]) -> Self {
        let mut t = Self::default();
        for s in symbols {
            t.names.insert((s.path.clone(), s.name.clone()));
            match &s.change {
                Change::Renamed { from } => {
                    t.names.insert((s.path.clone(), from.clone()));
                }
                Change::Moved { from } | Change::MovedAndChanged { from } => {
                    t.lines.insert((from.path.clone(), from.line));
                }
                _ => {}
            }
        }
        t
    }

    fn holds(&self, paths: [&str; 2], d: &Def) -> bool {
        paths.iter().any(|p| {
            self.names.contains(&(p.to_string(), d.qualified.clone()))
                || self.lines.contains(&(p.to_string(), d.start_line))
        })
    }
}

fn skip_file(text: &str, limits: &Limits) -> bool {
    let lines = text.lines().count().max(1);
    text.len() > limits.reuse_max_file_bytes || text.len() / lines > limits.minified_avg_line
}

/// Find, for each added function, an equal-shaped function at `merge_base` that the PR did not
/// touch. `new` holds the head definitions of changed files, keyed by path.
pub fn scan(
    repo: &Repo,
    merge_base: &str,
    files: &[FileDiff],
    new: &HashMap<String, Vec<Def>>,
    symbols: &[SymbolChange],
    cfg: &Config,
) -> Result<HashMap<HunkId, Vec<Miss>>> {
    let limits = &cfg.limits;
    let added = added_defs(symbols, new, limits);
    if !limits.reuse_enabled || added.is_empty() {
        return Ok(HashMap::new());
    }
    let mut by_shape: HashMap<u64, Vec<usize>> = HashMap::new();
    for (i, a) in added.iter().enumerate() {
        by_shape.entry(a.shape).or_default().push(i);
    }
    let touched = Touched::of(symbols);
    let renamed: HashMap<&str, &str> = files
        .iter()
        .filter_map(|f| Some((f.old_path.as_deref()?, f.new_path.as_deref()?)))
        .collect();
    let deleted: HashSet<&str> = files
        .iter()
        .filter(|f| f.status == FileStatus::Deleted)
        .map(FileDiff::path)
        .collect();
    let paths: Vec<String> = repo
        .ls_tree(merge_base)?
        .into_iter()
        .filter(|p| lang::detect(p).is_some() && !deleted.contains(p.as_str()))
        .filter(|p| risk::matching(p, &cfg.patterns.generated_paths).is_empty())
        .collect();

    let mut hits: Vec<Option<Miss>> = vec![None; added.len()];
    for chunk in paths.chunks(BATCH_FILES) {
        for (path, text) in repo.read_blobs(merge_base, chunk)? {
            let (Some(text), Some(lang)) = (text, lang::detect(&path)) else {
                continue;
            };
            if skip_file(&text, limits) {
                continue;
            }
            let Ok(defs) = lang::defs(lang, &text) else {
                continue;
            };
            let key = renamed.get(path.as_str()).copied().unwrap_or(&path);
            for d in defs.iter().filter(|d| eligible(d, limits)) {
                let Some(idxs) = by_shape.get(&d.shape) else {
                    continue;
                };
                if touched.holds([&path, key], d) {
                    continue;
                }
                for &i in idxs {
                    hits[i].get_or_insert_with(|| Miss {
                        name: added[i].name.clone(),
                        existing: d.qualified.clone(),
                        at: Loc {
                            path: path.clone(),
                            line: d.start_line,
                        },
                    });
                }
            }
        }
    }
    let mut out: HashMap<HunkId, Vec<Miss>> = HashMap::new();
    for (a, miss) in added.iter().zip(hits) {
        if let Some(m) = miss {
            out.entry(a.hunk.clone()).or_default().push(m);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reason_names_both_functions_and_location() {
        let m = Miss {
            name: "foo".into(),
            existing: "bar".into(),
            at: Loc {
                path: "a/b.py".into(),
                line: 12,
            },
        };
        assert_eq!(
            m.reason(),
            "new 'foo' duplicates existing 'bar' at a/b.py:12"
        );
    }

    #[test]
    fn minified_and_oversized_files_are_skipped() {
        let limits = Limits::default();
        assert!(!skip_file("a = 1\nb = 2\n", &limits));
        assert!(skip_file(
            &"x".repeat(limits.minified_avg_line + 1),
            &limits
        ));
        assert!(skip_file(
            &"x\n".repeat(limits.reuse_max_file_bytes),
            &limits
        ));
    }
}
