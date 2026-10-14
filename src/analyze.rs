//! End-to-end analysis of `merge_base(base, head)..head`.

use std::collections::HashMap;

use anyhow::{Context, Result};

use crate::config::Config;
use crate::git::Repo;
use crate::lang::{self, Def};
use crate::model::{Analysis, FileDiff, FileStatus, Hunk, HunkKind, Line, LineKind, Range};
use crate::{patch, risk, symbols};

pub fn run(repo: &Repo, base: &str, head: &str, cfg: &Config) -> Result<Analysis> {
    let base = repo
        .rev_parse(base)
        .with_context(|| format!("resolving '{base}'"))?;
    let head = repo
        .rev_parse(head)
        .with_context(|| format!("resolving '{head}'"))?;
    let merge_base = repo
        .merge_base(&base, &head)
        .context("finding merge base")?;
    let sim = cfg.limits.rename_similarity;

    let mut files = repo.diff_files(&merge_base, &head, sim)?;
    for f in &mut files {
        f.lang = lang::detect(f.path());
        f.generated = !risk::matching(f.path(), &cfg.patterns.generated_paths).is_empty();
    }
    let mut hunks = parse_hunks(&repo.diff_patch(&merge_base, &head, sim)?)?;
    copies_as_added(repo, &head, &mut files, &mut hunks)?;

    let old = defs_at(repo, &merge_base, &files, |f| f.old_path.as_deref())?;
    let new = defs_at(repo, &head, &files, |f| f.new_path.as_deref())?;
    let symbols = symbols::diff(
        &files,
        &old,
        &new,
        &mut hunks,
        &cfg.limits,
        &cfg.patterns.indent_sensitive,
    );
    risk::score(&files, &mut hunks, &symbols, cfg);
    risk::rank(&mut hunks);

    Ok(Analysis {
        range: Range {
            base,
            head,
            merge_base,
        },
        files,
        hunks,
        symbols,
        criteria: Vec::new(),
        assumptions: Vec::new(),
    })
}

fn parse_hunks(patch_text: &str) -> Result<Vec<Hunk>> {
    let mut hunks = Vec::new();
    for fp in patch::parse(patch_text).context("parsing diff")? {
        hunks.extend(fp.hunks);
    }
    Ok(hunks)
}

/// A copied file is reviewed as a new file: git's copy diff is relative to the source file and
/// would hide the copied content (and show deletions that never happened to this file).
fn copies_as_added(
    repo: &Repo,
    head: &str,
    files: &mut [FileDiff],
    hunks: &mut Vec<Hunk>,
) -> Result<()> {
    let mut copied = Vec::new();
    for f in files
        .iter_mut()
        .filter(|f| matches!(f.status, FileStatus::Copied { .. }))
    {
        f.status = FileStatus::Added;
        f.old_path = None;
        copied.push(f.path().to_string());
    }
    if copied.is_empty() {
        return Ok(());
    }
    hunks.retain(|h| !copied.contains(&h.path));
    for (path, text) in repo.read_blobs(head, &copied)? {
        hunks.extend(text.and_then(|t| added_hunk(&path, &t)));
    }
    Ok(())
}

fn added_hunk(path: &str, text: &str) -> Option<Hunk> {
    let lines: Vec<Line> = text
        .lines()
        .map(|t| Line {
            kind: LineKind::Added,
            text: t.to_string(),
        })
        .collect();
    let len = u32::try_from(lines.len()).ok().filter(|n| *n > 0)?;
    Some(Hunk {
        id: patch::hunk_id(path, 0, 1, &lines),
        path: path.to_string(),
        old_start: 0,
        old_len: 0,
        new_start: 1,
        new_len: len,
        lines,
        kind: HunkKind::New,
        symbols: Vec::new(),
        signals: Vec::new(),
        score: 0,
    })
}

/// Definitions of every changed, non-generated source file at `rev`, keyed by path.
/// A file that fails to parse is skipped with a warning.
fn defs_at(
    repo: &Repo,
    rev: &str,
    files: &[FileDiff],
    path_of: impl Fn(&FileDiff) -> Option<&str>,
) -> Result<HashMap<String, Vec<Def>>> {
    let wanted: Vec<(String, crate::model::Lang)> = files
        .iter()
        .filter(|f| !f.generated)
        .filter_map(|f| Some((path_of(f)?.to_string(), f.lang?)))
        .collect();
    let paths: Vec<String> = wanted.iter().map(|(p, _)| p.clone()).collect();
    let langs: HashMap<&str, crate::model::Lang> =
        wanted.iter().map(|(p, l)| (p.as_str(), *l)).collect();
    let mut out = HashMap::new();
    for (path, text) in repo.read_blobs(rev, &paths)? {
        let (Some(text), Some(&lang)) = (text, langs.get(path.as_str())) else {
            continue;
        };
        match lang::defs(lang, &text) {
            Ok(defs) => {
                out.insert(path, defs);
            }
            Err(e) => eprintln!("warning: skipping definitions of {path}: {e:#}"),
        }
    }
    Ok(out)
}
