//! Risk scoring: each hunk gets signals (rule, points, reason) from config-driven rules.
//! Moved, format-only and generated hunks are demoted to 0 but stay listed.

use std::collections::{HashMap, HashSet};

use crate::config::{Config, Patterns};
use crate::deps::{self, Scan, Scans};
use crate::model::{
    Change, FileDiff, FileStatus, Hunk, HunkId, HunkKind, LineKind, RiskSignal, Rule, SymbolChange,
};
use crate::reuse::Miss;

/// Findings that need more than the diff: parsed manifests and the base-tree duplicate scan.
#[derive(Debug, Default)]
pub struct Findings {
    pub deps: Scans,
    pub reuse: HashMap<HunkId, Vec<Miss>>,
}

/// Patterns (case-insensitive substrings) that match `path`, in config order.
/// The path is matched with a leading `/` so `"/tests/"` also matches `tests/a.py`.
pub fn matching<'a>(path: &str, pats: &'a [String]) -> Vec<&'a str> {
    let p = format!("/{}", path.to_lowercase());
    pats.iter()
        .filter(|x| p.contains(&x.to_lowercase()))
        .map(String::as_str)
        .collect()
}

fn is_test_path(path: &str, pat: &Patterns) -> bool {
    !matching(path, &pat.test_paths).is_empty()
}

fn stem(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.rsplit_once('.')
        .map_or(name, |(s, _)| s)
        .to_lowercase()
}

/// Whole-token cover: every alphanumeric token of source stem `name` is a token of `test_stem`
/// (`parse` is covered by `test_parse` and `parse.spec`, but `io` is not by `test_ratio`).
fn stem_covers(test_stem: &str, name: &str) -> bool {
    let tokens = |s: &str| -> Vec<String> {
        s.split(|c: char| !c.is_alphanumeric())
            .filter(|t| !t.is_empty())
            .map(str::to_string)
            .collect()
    };
    let have = tokens(test_stem);
    tokens(name).iter().all(|t| have.contains(t))
}

fn signal(rule: Rule, points: u32, reason: String) -> Option<RiskSignal> {
    Some(RiskSignal {
        rule,
        points,
        reason,
    })
}

fn quote_list(items: &[String]) -> String {
    items
        .iter()
        .map(|i| format!("'{i}'"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Assign `signals` and `score` to every hunk.
pub fn score(
    files: &[FileDiff],
    hunks: &mut [Hunk],
    symbols: &[SymbolChange],
    found: &Findings,
    cfg: &Config,
) {
    let by_path: HashMap<&str, &FileDiff> = files.iter().map(|f| (f.path(), f)).collect();
    let test_stems: Vec<String> = files
        .iter()
        .filter(|f| is_test_path(f.path(), &cfg.patterns))
        .map(|f| stem(f.path()))
        .collect();
    for h in hunks.iter_mut() {
        let file = by_path.get(h.path.as_str()).copied();
        let ctx = Ctx {
            file,
            symbols,
            test_stems: &test_stems,
            found,
            cfg,
        };
        let signals = match demotion(h, file) {
            Some(s) => vec![s],
            None => ctx.signals(h),
        };
        h.score = signals.iter().map(|s| s.points).sum();
        h.signals = signals;
    }
}

/// Score descending, then path, then new_start; the id keeps the order total.
pub fn rank(hunks: &mut [Hunk]) {
    hunks.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.path.cmp(&b.path))
            .then_with(|| a.new_start.cmp(&b.new_start))
            .then_with(|| a.id.cmp(&b.id))
    });
}

fn demotion(h: &Hunk, file: Option<&FileDiff>) -> Option<RiskSignal> {
    if file.is_some_and(|f| f.generated) {
        return signal(
            Rule::Generated,
            0,
            format!("generated or lock file: {}", h.path),
        );
    }
    match &h.kind {
        HunkKind::Moved { from } => signal(Rule::Moved, 0, h.move_label(from)),
        HunkKind::FormatOnly => signal(Rule::FormatOnly, 0, "whitespace-only change".into()),
        _ => None,
    }
}

/// Line of the first added line carrying `pat` (case-insensitive) beyond those the removed lines
/// already carried; `None` when the pattern was only kept, re-indented or moved within the hunk.
fn net_added_line(h: &Hunk, pat: &str) -> Option<u32> {
    let pat = pat.to_lowercase();
    let has = |t: &str| t.to_lowercase().contains(&pat);
    let ch = h.changed();
    let removed = ch
        .iter()
        .filter(|(k, _, t)| *k == LineKind::Removed && has(t))
        .count();
    ch.iter()
        .filter(|(k, _, t)| *k == LineKind::Added && has(t))
        .nth(removed)
        .map(|(_, n, _)| *n)
}

struct Ctx<'a> {
    file: Option<&'a FileDiff>,
    symbols: &'a [SymbolChange],
    test_stems: &'a [String],
    found: &'a Findings,
    cfg: &'a Config,
}

impl Ctx<'_> {
    fn signals(&self, h: &Hunk) -> Vec<RiskSignal> {
        [
            self.public_sig(h),
            self.test_removed(h),
            self.new_dependency(h),
            self.sensitive(h),
            self.reuse_miss(h),
            self.suppression(h),
            self.error_handling(h),
            self.infra(h),
            self.no_test_touch(h),
            self.size(h),
        ]
        .into_iter()
        .flatten()
        .collect()
    }

    fn syms<'s>(&'s self, h: &'s Hunk) -> impl Iterator<Item = &'s SymbolChange> + 's {
        h.symbols.iter().filter_map(|&i| self.symbols.get(i))
    }

    fn public_sig(&self, h: &Hunk) -> Option<RiskSignal> {
        let reasons: Vec<String> = self
            .syms(h)
            .filter(|s| s.public && !is_test_path(&s.path, &self.cfg.patterns))
            .filter_map(|s| match (&s.change, &s.old_sig, &s.new_sig) {
                (Change::SignatureChanged, Some(o), Some(n)) => Some(format!(
                    "public signature of '{}' changed: {o} -> {n}",
                    s.name
                )),
                (Change::Renamed { from }, _, _) => {
                    Some(format!("public symbol '{from}' renamed to '{}'", s.name))
                }
                (Change::Removed, _, _) => Some(format!("public symbol '{}' removed", s.name)),
                _ => None,
            })
            .collect();
        signal(
            Rule::PublicSigChanged,
            self.cfg.weights.public_sig_changed,
            reasons.join("; "),
        )
        .filter(|_| !reasons.is_empty())
    }

    fn test_removed(&self, h: &Hunk) -> Option<RiskSignal> {
        let pat = &self.cfg.patterns;
        if !is_test_path(&h.path, pat) {
            return None;
        }
        let w = self.cfg.weights.test_removed;
        if self.file.is_some_and(|f| f.status == FileStatus::Deleted) {
            return signal(
                Rule::TestRemoved,
                w,
                format!("test file deleted: {}", h.path),
            );
        }
        let gone: Vec<String> = self
            .syms(h)
            .filter(|s| s.change == Change::Removed && is_test_name(&s.name, pat))
            .map(|s| format!("'{}'", s.name))
            .collect();
        if !gone.is_empty() {
            return signal(
                Rule::TestRemoved,
                w,
                format!("test removed: {}", gone.join(", ")),
            );
        }
        let count = |kind| {
            h.changed()
                .iter()
                .filter(|(k, _, t)| *k == kind && has_any(t, &pat.assertions))
                .count()
        };
        let (before, after) = (count(LineKind::Removed), count(LineKind::Added));
        (before > after).then(|| RiskSignal {
            rule: Rule::TestRemoved,
            points: w,
            reason: format!("assertions dropped in test file: {before} -> {after}"),
        })
    }

    fn new_dependency(&self, h: &Hunk) -> Option<RiskSignal> {
        let (names, note) = match self.found.deps.get(&h.path) {
            Some(Scan::Parsed(by_hunk)) => (by_hunk.get(&h.id).cloned().unwrap_or_default(), ""),
            unparsed => {
                let names = deps::added(&h.path, &h.lines, &self.cfg.patterns.dependency_sections);
                let note = if unparsed.is_some() {
                    " (line heuristic: manifest could not be parsed)"
                } else {
                    ""
                };
                (names, note)
            }
        };
        let reason = format!("new dependency {} in {}{note}", quote_list(&names), h.path);
        signal(Rule::NewDependency, self.cfg.weights.new_dependency, reason)
            .filter(|_| !names.is_empty())
    }

    fn reuse_miss(&self, h: &Hunk) -> Option<RiskSignal> {
        let misses = self.found.reuse.get(&h.id)?;
        let first = misses.first()?;
        let reasons: Vec<String> = misses.iter().map(Miss::reason).collect();
        signal(
            Rule::ReuseMiss {
                existing: first.at.clone(),
            },
            self.cfg.weights.reuse_miss,
            reasons.join("; "),
        )
    }

    fn sensitive(&self, h: &Hunk) -> Option<RiskSignal> {
        let pats = &self.cfg.patterns.sensitive;
        let in_path = matching(&h.path, pats);
        let in_lines: Vec<&str> = pats
            .iter()
            .filter(|p| net_added_line(h, p).is_some())
            .map(String::as_str)
            .collect();
        let mut parts = Vec::new();
        if !in_path.is_empty() {
            parts.push(format!("path matches {}", quote_list(&owned(&in_path))));
        }
        if !in_lines.is_empty() {
            parts.push(format!(
                "added lines mention {}",
                quote_list(&owned(&in_lines))
            ));
        }
        signal(
            Rule::SensitiveArea,
            self.cfg.weights.sensitive_area,
            format!("sensitive area: {}", parts.join("; ")),
        )
        .filter(|_| !parts.is_empty())
    }

    fn suppression(&self, h: &Hunk) -> Option<RiskSignal> {
        let found: Vec<String> = self
            .cfg
            .patterns
            .suppressions
            .iter()
            .filter_map(|p| net_added_line(h, p).map(|line| format!("'{p}' (line {line})")))
            .collect();
        signal(
            Rule::SuppressionAdded,
            self.cfg.weights.suppression_added,
            format!("suppression added: {}", found.join(", ")),
        )
        .filter(|_| !found.is_empty())
    }

    fn error_handling(&self, h: &Hunk) -> Option<RiskSignal> {
        let pat = &self.cfg.patterns;
        let w = self.cfg.weights.error_handling_weakened;
        let ch = h.changed();
        let count = |kind| {
            ch.iter()
                .filter(|(k, _, t)| *k == kind && starts_handler(t, &pat.error_handling))
                .count()
        };
        let (before, after) = (count(LineKind::Removed), count(LineKind::Added));
        if before > after {
            let reason = format!(
                "error handling removed: {before} try/catch/except lines dropped to {after}"
            );
            return signal(Rule::ErrorHandlingWeakened, w, reason);
        }
        let added: Vec<String> = ch
            .iter()
            .filter(|(k, _, _)| *k == LineKind::Added)
            .map(|(_, _, t)| squash(t))
            .collect();
        let windows = added
            .iter()
            .cloned()
            .chain(added.windows(2).map(|w| w.concat()))
            .map(|w| w.trim_start_matches('}').to_string());
        let hits: HashSet<&String> = windows
            .flat_map(|win| {
                pat.weak_handlers
                    .iter()
                    .filter(move |p| win.starts_with(&squash(p)))
            })
            .collect();
        let mut hits: Vec<String> = hits.into_iter().cloned().collect();
        hits.sort();
        signal(
            Rule::ErrorHandlingWeakened,
            w,
            format!("swallowing error handler added: {}", quote_list(&hits)),
        )
        .filter(|_| !hits.is_empty())
    }

    fn infra(&self, h: &Hunk) -> Option<RiskSignal> {
        let m = matching(&h.path, &self.cfg.patterns.infra_paths);
        let reason = format!(
            "infra/CI config touched: {} (matches {})",
            h.path,
            quote_list(&owned(&m))
        );
        signal(Rule::InfraConfig, self.cfg.weights.infra_config, reason).filter(|_| !m.is_empty())
    }

    fn no_test_touch(&self, h: &Hunk) -> Option<RiskSignal> {
        let f = self.file?;
        let is_source = f.lang.is_some()
            && f.status != FileStatus::Deleted
            && !is_test_path(&h.path, &self.cfg.patterns);
        let name = stem(&h.path);
        let untested =
            is_source && !name.is_empty() && !self.test_stems.iter().any(|t| stem_covers(t, &name));
        signal(
            Rule::NoTestTouch,
            self.cfg.weights.no_test_touch,
            format!(
                "source file {} changed but no test file mentioning '{name}' changed",
                h.path
            ),
        )
        .filter(|_| untested)
    }

    fn size(&self, h: &Hunk) -> Option<RiskSignal> {
        let n = h.changed().len() as u32;
        let points = n
            .checked_ilog2()
            .unwrap_or(0)
            .min(self.cfg.weights.size_cap);
        signal(Rule::Size, points, format!("{n} changed lines")).filter(|_| points > 0)
    }
}

fn owned(xs: &[&str]) -> Vec<String> {
    xs.iter().map(|s| s.to_string()).collect()
}

fn has_any(text: &str, needles: &[String]) -> bool {
    let t = text.to_lowercase();
    needles.iter().any(|n| t.contains(&n.to_lowercase()))
}

fn is_test_name(qualified: &str, pat: &Patterns) -> bool {
    qualified
        .split('.')
        .any(|seg| pat.test_symbols.iter().any(|p| seg.starts_with(p.as_str())))
}

fn squash(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect()
}

/// Line opens with a keyword as a whole word (`try:`, `} catch (e) {`), not `tryParse(`.
fn starts_handler(line: &str, keywords: &[String]) -> bool {
    let t = line.trim_start().trim_start_matches('}').trim_start();
    keywords.iter().any(|k| {
        t.strip_prefix(k.as_str())
            .is_some_and(|rest| !rest.starts_with(|c: char| c.is_alphanumeric() || c == '_'))
    })
}

#[cfg(test)]
mod tests;
