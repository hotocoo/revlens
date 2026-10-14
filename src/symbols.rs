//! Symbol-level diff: pairs definitions across file versions and classifies what happened to
//! each (added, removed, edited, renamed, moved), then marks hunks that are pure moves.

use std::collections::HashMap;

use crate::config::Limits;
use crate::fingerprint;
use crate::lang::Def;
use crate::model::{Change, FileDiff, Hunk, HunkKind, LineKind, Loc, SymbolChange};
use crate::risk;

/// Where a definition lives: the real path, the path hunks of that file use, and its line span.
struct Span {
    path: String,
    hunk_path: String,
    start: u32,
    end: u32,
}

impl Span {
    fn loc(&self) -> Loc {
        Loc {
            path: self.path.clone(),
            line: self.start,
        }
    }

    fn holds(&self, hunk: &Hunk, line: u32) -> bool {
        hunk.path == self.hunk_path && (self.start..=self.end).contains(&line)
    }
}

struct Entry {
    sym: SymbolChange,
    old: Option<Span>,
    new: Option<Span>,
}

#[derive(Clone, Copy)]
struct Cand<'a> {
    pair: usize,
    path: &'a str,
    hunk_path: &'a str,
    def: &'a Def,
}

impl Cand<'_> {
    fn span(&self) -> Span {
        Span {
            path: self.path.to_string(),
            hunk_path: self.hunk_path.to_string(),
            start: self.def.start_line,
            end: self.def.end_line,
        }
    }
}

/// `base` supplies name, kind and visibility: the new side when there is one.
fn entry(base: Cand, old: Option<Cand>, new: Option<Cand>, change: Change) -> Entry {
    Entry {
        sym: SymbolChange {
            name: base.def.qualified.clone(),
            kind: base.def.kind,
            path: base.path.to_string(),
            public: base.def.public,
            change,
            old_sig: old.map(|c| c.def.signature.clone()),
            new_sig: new.map(|c| c.def.signature.clone()),
            hunks: Vec::new(),
        },
        old: old.map(|c| c.span()),
        new: new.map(|c| c.span()),
    }
}

/// Classify symbol changes between `old` (keyed by old path) and `new` (keyed by new path), link
/// them to hunks (`hunk.symbols` indexes the returned vec), and refine `hunk.kind` to `Moved` or
/// `FormatOnly` where it applies. Hunks are never removed.
pub fn diff(
    files: &[FileDiff],
    old: &HashMap<String, Vec<Def>>,
    new: &HashMap<String, Vec<Def>>,
    hunks: &mut [Hunk],
    limits: &Limits,
    indent_sensitive: &[String],
) -> Vec<SymbolChange> {
    let mut entries = Vec::new();
    let (mut removed, mut added) = (Vec::new(), Vec::new());
    for (pair, f) in files.iter().enumerate() {
        let (o, n) = (side(&f.old_path, old), side(&f.new_path, new));
        pair_file(pair, f.path(), o, n, &mut entries, &mut removed, &mut added);
    }
    cross_file(&removed, &added, &mut entries, limits.rename_min_nodes);
    link_hunks(&mut entries, hunks);
    mark_pure_moves(&entries, hunks);
    mark_line_moves(hunks, limits, indent_sensitive);
    entries.into_iter().map(|e| e.sym).collect()
}

type Side<'a> = Option<(&'a str, &'a [Def])>;

fn side<'a>(path: &'a Option<String>, defs: &'a HashMap<String, Vec<Def>>) -> Side<'a> {
    let p = path.as_deref()?;
    defs.get(p).map(|d| (p, d.as_slice()))
}

/// Match defs of one file pair by qualified name; leftovers become removal/addition candidates.
fn pair_file<'a>(
    pair: usize,
    hunk_path: &'a str,
    old: Side<'a>,
    new: Side<'a>,
    entries: &mut Vec<Entry>,
    removed: &mut Vec<Cand<'a>>,
    added: &mut Vec<Cand<'a>>,
) {
    let (op, od) = old.unwrap_or(("", &[]));
    let (np, nd) = new.unwrap_or(("", &[]));
    let cand = |path, def| Cand {
        pair,
        path,
        hunk_path,
        def,
    };
    let mut slots: HashMap<&str, Vec<usize>> = HashMap::new();
    for (j, d) in nd.iter().enumerate().rev() {
        slots.entry(d.qualified.as_str()).or_default().push(j);
    }
    let mut used = vec![false; nd.len()];
    for o in od {
        let hit = slots.get_mut(o.qualified.as_str()).and_then(Vec::pop);
        let Some(j) = hit else {
            removed.push(cand(op, o));
            continue;
        };
        used[j] = true;
        let n = &nd[j];
        if o.exact == n.exact {
            continue;
        }
        let change = if o.signature == n.signature {
            Change::BodyChanged
        } else {
            Change::SignatureChanged
        };
        entries.push(entry(
            cand(np, n),
            Some(cand(op, o)),
            Some(cand(np, n)),
            change,
        ));
    }
    for (n, _) in nd.iter().zip(&used).filter(|(_, u)| !**u) {
        added.push(cand(np, n));
    }
}

/// Take the first unused candidate in `pool` accepted by `ok`.
fn take<'a>(
    pool: &[Cand<'a>],
    used: &mut [bool],
    ok: impl Fn(&Cand<'a>) -> bool,
) -> Option<Cand<'a>> {
    let i = (0..pool.len()).find(|&i| !used[i] && ok(&pool[i]))?;
    used[i] = true;
    Some(pool[i])
}

fn unused<'a, 'b>(
    pool: &'b [Cand<'a>],
    used: &'b [bool],
) -> impl Iterator<Item = &'b Cand<'a>> + 'b {
    pool.iter().zip(used).filter(|(_, u)| !**u).map(|(c, _)| c)
}

fn cross_file(removed: &[Cand], added: &[Cand], entries: &mut Vec<Entry>, min_nodes: usize) {
    let (mut ur, mut ua) = (vec![false; removed.len()], vec![false; added.len()]);
    // Moved: same name, other file, identical text.
    for (i, r) in removed.iter().enumerate() {
        let hit = take(added, &mut ua, |a| {
            a.path != r.path && a.def.name == r.def.name && a.def.exact == r.def.exact
        });
        if let Some(a) = hit {
            ur[i] = true;
            let from = r.span().loc();
            entries.push(entry(a, Some(*r), Some(a), Change::Moved { from }));
        }
    }
    // Renamed: identical code under a new name (literals and callees included); same file pair,
    // else the only such pair anywhere, and only for non-trivial bodies.
    for (i, r) in removed.iter().enumerate() {
        if ur[i] {
            continue;
        }
        let shaped = |a: &&Cand| a.def.unnamed == r.def.unnamed && a.def.name != r.def.name;
        let twin = |c: &&Cand| c.def.unnamed == r.def.unnamed;
        let same_pair = |c: &&Cand| c.pair == r.pair;
        let in_pair = unused(added, &ua).filter(shaped).filter(same_pair).count() == 1
            && unused(removed, &ur).filter(twin).filter(same_pair).count() == 1;
        let global = r.def.nodes >= min_nodes
            && unused(added, &ua).filter(shaped).count() == 1
            && unused(removed, &ur).filter(twin).count() == 1;
        if !in_pair && !global {
            continue;
        }
        let hit = take(added, &mut ua, |a| {
            a.def.unnamed == r.def.unnamed
                && a.def.name != r.def.name
                && (!in_pair || a.pair == r.pair)
        });
        if let Some(a) = hit {
            ur[i] = true;
            let from = r.def.qualified.clone();
            entries.push(entry(a, Some(*r), Some(a), Change::Renamed { from }));
        }
    }
    // MovedAndChanged: same name in another file, edited, when the name is unambiguous.
    for (i, r) in removed.iter().enumerate() {
        if ur[i] {
            continue;
        }
        let same_name = |c: &&Cand| c.def.name == r.def.name;
        let one_removed = unused(removed, &ur).filter(same_name).count() == 1;
        let one_added = unused(added, &ua).filter(same_name).count() == 1;
        if !(one_removed && one_added) {
            continue;
        }
        let hit = take(added, &mut ua, |a| {
            a.path != r.path && a.def.name == r.def.name
        });
        if let Some(a) = hit {
            ur[i] = true;
            let from = r.span().loc();
            entries.push(entry(
                a,
                Some(*r),
                Some(a),
                Change::MovedAndChanged { from },
            ));
        }
    }
    // MovedAndChanged, renamed variant: another file, new name, same identifier-insensitive
    // shape (e.g. body now calls a renamed helper). Non-trivial and unique on both sides only;
    // never demoted, so the reviewer still sees the edit.
    for (i, r) in removed.iter().enumerate() {
        if ur[i] || r.def.nodes < min_nodes {
            continue;
        }
        let shaped = |c: &&Cand| c.path != r.path && c.def.shape == r.def.shape;
        let twin = |c: &&Cand| c.def.shape == r.def.shape;
        if unused(added, &ua).filter(shaped).count() != 1
            || unused(removed, &ur).filter(twin).count() != 1
        {
            continue;
        }
        if let Some(a) = take(added, &mut ua, |a| shaped(&a)) {
            ur[i] = true;
            let from = r.span().loc();
            entries.push(entry(
                a,
                Some(*r),
                Some(a),
                Change::MovedAndChanged { from },
            ));
        }
    }
    for (r, _) in removed.iter().zip(&ur).filter(|(_, u)| !**u) {
        entries.push(entry(*r, Some(*r), None, Change::Removed));
    }
    for (a, _) in added.iter().zip(&ua).filter(|(_, u)| !**u) {
        entries.push(entry(*a, None, Some(*a), Change::Added));
    }
}

/// Changed lines of a hunk: (kind, line number on that side).
fn changed_lines(h: &Hunk) -> Vec<(LineKind, u32)> {
    h.changed().into_iter().map(|(k, n, _)| (k, n)).collect()
}

fn link_hunks(entries: &mut [Entry], hunks: &mut [Hunk]) {
    let lines: Vec<_> = hunks.iter().map(changed_lines).collect();
    for (ei, e) in entries.iter_mut().enumerate() {
        for (hi, h) in hunks.iter_mut().enumerate() {
            let hits = |span: &Option<Span>, kind| {
                span.as_ref()
                    .is_some_and(|s| lines[hi].iter().any(|&(k, n)| k == kind && s.holds(h, n)))
            };
            if hits(&e.old, LineKind::Removed) || hits(&e.new, LineKind::Added) {
                e.sym.hunks.push(h.id.clone());
                h.symbols.push(ei);
            }
        }
    }
}

/// A hunk whose every changed line sits inside a Moved def is a pure move. A rename changes the name callers
/// use, so it stays an ordinary changed hunk.
fn mark_pure_moves(entries: &[Entry], hunks: &mut [Hunk]) {
    for h in hunks.iter_mut() {
        let movers: Vec<&Entry> = h
            .symbols
            .iter()
            .map(|&i| &entries[i])
            .filter(|e| matches!(e.sym.change, Change::Moved { .. }))
            .collect();
        let lines: Vec<_> = h
            .changed()
            .into_iter()
            .filter(|(_, _, text)| !text.trim().is_empty())
            .map(|(k, n, _)| (k, n))
            .collect();
        let covering = |kind: LineKind, n: u32| {
            movers.iter().find(|e| {
                let span = if kind == LineKind::Removed {
                    &e.old
                } else {
                    &e.new
                };
                span.as_ref().is_some_and(|s| s.holds(h, n))
            })
        };
        let covers: Vec<_> = lines.iter().map(|&(k, n)| (k, covering(k, n))).collect();
        if covers.is_empty() || covers.iter().any(|(_, e)| e.is_none()) {
            continue;
        }
        if let Some((kind, Some(e))) = covers.first() {
            let counterpart = if *kind == LineKind::Removed {
                &e.new
            } else {
                &e.old
            };
            if let Some(s) = counterpart {
                h.kind = HunkKind::Moved { from: s.loc() };
            }
        }
    }
}

/// Line-based fallback for blocks that are not whole definitions, then whitespace-only edits.
fn mark_line_moves(hunks: &mut [Hunk], limits: &Limits, indent_sensitive: &[String]) {
    let open = |h: &Hunk| {
        matches!(
            h.kind,
            HunkKind::Changed | HunkKind::New | HunkKind::Deleted
        )
    };
    let pool: Vec<Hunk> = hunks.iter().filter(|h| open(h)).cloned().collect();
    let moved: HashMap<_, _> = fingerprint::moved_blocks(&pool, limits.moved_min_block)
        .into_iter()
        .collect();
    for h in hunks.iter_mut().filter(|h| open(h)) {
        if let Some(from) = moved.get(&h.id) {
            h.kind = HunkKind::Moved { from: from.clone() };
        } else if h.kind == HunkKind::Changed
            && fingerprint::is_format_only(h, !risk::matching(&h.path, indent_sensitive).is_empty())
        {
            h.kind = HunkKind::FormatOnly;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lang;
    use crate::model::{FileStatus, Lang, Line};

    fn defs(src: &str) -> Vec<Def> {
        lang::defs(Lang::Python, src).expect("parse")
    }

    fn file(old: Option<&str>, new: Option<&str>) -> FileDiff {
        let status = match (old, new) {
            (None, _) => FileStatus::Added,
            (_, None) => FileStatus::Deleted,
            _ => FileStatus::Modified,
        };
        FileDiff {
            old_path: old.map(str::to_string),
            new_path: new.map(str::to_string),
            status,
            lang: Some(Lang::Python),
            generated: false,
        }
    }

    fn hunk(id: &str, path: &str, old_start: u32, new_start: u32, spec: &[&str]) -> Hunk {
        let lines: Vec<Line> = spec
            .iter()
            .map(|s| {
                let (k, t) = s.split_at(1);
                let kind = match k {
                    "+" => LineKind::Added,
                    "-" => LineKind::Removed,
                    _ => LineKind::Context,
                };
                Line {
                    kind,
                    text: t.to_string(),
                }
            })
            .collect();
        let count = |f: fn(&LineKind) -> bool| lines.iter().filter(|l| f(&l.kind)).count() as u32;
        let old_len = count(|k| *k != LineKind::Added);
        let new_len = count(|k| *k != LineKind::Removed);
        let kind = match (old_len, new_len) {
            (0, _) => HunkKind::New,
            (_, 0) => HunkKind::Deleted,
            _ => HunkKind::Changed,
        };
        Hunk {
            id: id.into(),
            path: path.into(),
            old_start,
            old_len,
            new_start,
            new_len,
            lines,
            kind,
            symbols: vec![],
            signals: vec![],
            score: 0,
        }
    }

    type Defs = HashMap<String, Vec<Def>>;

    fn run(
        files: &[FileDiff],
        old: &[(&str, &str)],
        new: &[(&str, &str)],
        hunks: &mut [Hunk],
    ) -> Vec<SymbolChange> {
        let m = |xs: &[(&str, &str)]| -> Defs {
            xs.iter().map(|(p, s)| (p.to_string(), defs(s))).collect()
        };
        let cfg = crate::config::Config::default();
        diff(
            files,
            &m(old),
            &m(new),
            hunks,
            &cfg.limits,
            &cfg.patterns.indent_sensitive,
        )
    }

    const F: &str = "def f(x):\n    return x + 1\n";

    #[test]
    fn pure_move_between_files() {
        let mut hunks = [
            hunk("h1", "a.py", 1, 1, &["-def f(x):", "-    return x + 1"]),
            hunk("h2", "b.py", 0, 1, &["+def f(x):", "+    return x + 1"]),
        ];
        let files = [file(Some("a.py"), Some("a.py")), file(None, Some("b.py"))];
        let s = run(
            &files,
            &[("a.py", F)],
            &[("a.py", ""), ("b.py", F)],
            &mut hunks,
        );
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].name, "f");
        assert_eq!(s[0].path, "b.py");
        assert_eq!(
            s[0].change,
            Change::Moved {
                from: Loc {
                    path: "a.py".into(),
                    line: 1
                }
            }
        );
        assert_eq!(s[0].hunks, ["h1", "h2"]);
        assert_eq!(hunks[0].symbols, [0]);
        assert_eq!(
            hunks[0].kind,
            HunkKind::Moved {
                from: Loc {
                    path: "b.py".into(),
                    line: 1
                }
            }
        );
        assert_eq!(
            hunks[1].kind,
            HunkKind::Moved {
                from: Loc {
                    path: "a.py".into(),
                    line: 1
                }
            }
        );
    }

    #[test]
    fn move_with_edit_is_moved_and_changed_and_not_pure() {
        let g = "def f(x):\n    return x + 2\n";
        let mut hunks = [
            hunk("h1", "a.py", 1, 1, &["-def f(x):", "-    return x + 1"]),
            hunk("h2", "b.py", 0, 1, &["+def f(x):", "+    return x + 2"]),
        ];
        let files = [file(Some("a.py"), Some("a.py")), file(None, Some("b.py"))];
        let s = run(
            &files,
            &[("a.py", F)],
            &[("a.py", ""), ("b.py", g)],
            &mut hunks,
        );
        assert!(matches!(s[0].change, Change::MovedAndChanged { .. }));
        assert_eq!(hunks[0].kind, HunkKind::Deleted);
        assert_eq!(hunks[1].kind, HunkKind::New);
    }

    #[test]
    fn ambiguous_name_is_not_moved_and_changed() {
        let g = "def f(x):\n    return x * 2\n";
        let files = [
            file(Some("a.py"), Some("a.py")),
            file(None, Some("b.py")),
            file(None, Some("c.py")),
        ];
        let mut hunks: [Hunk; 0] = [];
        let s = run(
            &files,
            &[("a.py", F)],
            &[
                ("a.py", ""),
                ("b.py", g),
                ("c.py", "def f(x):\n    return 7\n"),
            ],
            &mut hunks,
        );
        let kinds: Vec<_> = s.iter().map(|c| c.change.clone()).collect();
        assert_eq!(kinds, [Change::Removed, Change::Added, Change::Added]);
    }

    #[test]
    fn rename_in_place_is_not_a_pure_move() {
        let g = "def g(x):\n    return x + 1\n";
        let mut hunks = [hunk(
            "h1",
            "a.py",
            1,
            1,
            &["-def f(x):", "+def g(x):", "     return x + 1"],
        )];
        let files = [file(Some("a.py"), Some("a.py"))];
        let s = run(&files, &[("a.py", F)], &[("a.py", g)], &mut hunks);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].change, Change::Renamed { from: "f".into() });
        assert_eq!(s[0].name, "g");
        assert_eq!(hunks[0].kind, HunkKind::Changed);
    }

    #[test]
    fn signature_and_body_changes_attach_hunks() {
        let old = "def f(a):\n    return a\n\n\ndef h(a):\n    return a\n";
        let new = "def f(a, b):\n    return a\n\n\ndef h(a):\n    return a + 1\n";
        let mut hunks = [
            hunk(
                "h1",
                "a.py",
                1,
                1,
                &["-def f(a):", "+def f(a, b):", "     return a"],
            ),
            hunk(
                "h2",
                "a.py",
                5,
                5,
                &[" def h(a):", "-    return a", "+    return a + 1"],
            ),
        ];
        let files = [file(Some("a.py"), Some("a.py"))];
        let s = run(&files, &[("a.py", old)], &[("a.py", new)], &mut hunks);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].change, Change::SignatureChanged);
        assert_eq!(s[0].old_sig.as_deref(), Some("(a)"));
        assert_eq!(s[0].new_sig.as_deref(), Some("(a, b)"));
        assert_eq!(s[0].hunks, ["h1"]);
        assert_eq!(s[1].change, Change::BodyChanged);
        assert_eq!(s[1].hunks, ["h2"]);
        assert_eq!(hunks[0].kind, HunkKind::Changed);
        assert_eq!(hunks[1].symbols, [1]);
    }

    #[test]
    fn added_and_removed_symbols() {
        let mut hunks: [Hunk; 0] = [];
        let files = [file(Some("a.py"), Some("a.py"))];
        let s = run(
            &files,
            &[("a.py", F)],
            &[("a.py", "def other(y, z):\n    return y * z\n")],
            &mut hunks,
        );
        let kinds: Vec<_> = s.iter().map(|c| c.change.clone()).collect();
        assert_eq!(kinds, [Change::Removed, Change::Added]);
        assert!(s[0].new_sig.is_none() && s[1].old_sig.is_none());
    }

    #[test]
    fn renamed_file_pairs_defs_by_old_path() {
        let mut hunks = [hunk(
            "h1",
            "n.py",
            1,
            1,
            &["-def f(x):", "+def f(x, y):", "     return x + 1"],
        )];
        let files = [file(Some("o.py"), Some("n.py"))];
        let new = "def f(x, y):\n    return x + 1\n";
        let s = run(&files, &[("o.py", F)], &[("n.py", new)], &mut hunks);
        assert_eq!(s[0].change, Change::SignatureChanged);
        assert_eq!(s[0].path, "n.py");
        assert_eq!(s[0].hunks, ["h1"]);
    }

    #[test]
    fn whitespace_only_hunk_is_format_only_and_blocks_are_moved() {
        let mut hunks = [
            hunk("h1", "a.txt", 1, 1, &["-x = 1", "+x  =  1"]),
            hunk(
                "h2",
                "b.txt",
                1,
                1,
                &["-alpha one", "-beta two", "-gamma three", " keep"],
            ),
            hunk(
                "h3",
                "c.txt",
                0,
                1,
                &["+alpha one", "+beta two", "+gamma three"],
            ),
        ];
        let s = run(&[], &[], &[], &mut hunks);
        assert!(s.is_empty());
        assert_eq!(hunks[0].kind, HunkKind::FormatOnly);
        assert!(matches!(hunks[1].kind, HunkKind::Moved { .. }));
        assert!(matches!(hunks[2].kind, HunkKind::Moved { .. }));
    }

    #[test]
    fn rename_with_literal_change_is_not_a_pure_move() {
        let old = "def limit():\n    return 30\n";
        let new = "def max_limit():\n    return 3000\n";
        let mut hunks = [hunk(
            "h1",
            "a.py",
            1,
            1,
            &[
                "-def limit():",
                "-    return 30",
                "+def max_limit():",
                "+    return 3000",
            ],
        )];
        let files = [file(Some("a.py"), Some("a.py"))];
        let s = run(&files, &[("a.py", old)], &[("a.py", new)], &mut hunks);
        assert!(!s.iter().any(|c| matches!(c.change, Change::Renamed { .. })));
        assert_eq!(hunks[0].kind, HunkKind::Changed);
    }

    #[test]
    fn rename_with_different_callee_is_not_a_pure_move() {
        let old = "def is_allowed(u):\n    return check_admin(u)\n";
        let new = "def is_permitted(u):\n    return check_guest(u)\n";
        let mut hunks = [hunk(
            "h1",
            "a.py",
            1,
            1,
            &[
                "-def is_allowed(u):",
                "-    return check_admin(u)",
                "+def is_permitted(u):",
                "+    return check_guest(u)",
            ],
        )];
        let files = [file(Some("a.py"), Some("a.py"))];
        let s = run(&files, &[("a.py", old)], &[("a.py", new)], &mut hunks);
        assert!(!s.iter().any(|c| matches!(c.change, Change::Renamed { .. })));
        assert_eq!(hunks[0].kind, HunkKind::Changed);
    }

    #[test]
    fn trivial_stubs_in_different_files_are_not_a_global_rename() {
        let a = "def is_dropped(self):\n    raise NotImplementedError()\n";
        let b = "def get_version(self):\n    raise NotImplementedError()\n";
        let files = [
            file(Some("a.py"), Some("a.py")),
            file(Some("b.py"), Some("b.py")),
        ];
        let mut hunks: [Hunk; 0] = [];
        let s = run(
            &files,
            &[("a.py", a), ("b.py", "")],
            &[("a.py", ""), ("b.py", b)],
            &mut hunks,
        );
        let kinds: Vec<_> = s.iter().map(|c| c.change.clone()).collect();
        assert_eq!(kinds, [Change::Removed, Change::Added]);
    }

    #[test]
    fn nontrivial_global_rename_still_detected() {
        let body = "    a = [i * 2 for i in range(10) if i % 3 == 0]\n    b = {k: v + 1 for k, v in zip(a, a)}\n    return sum(b.values()) + len(a)\n";
        let (o, n) = (
            format!("def old_name(x):\n{body}"),
            format!("def new_name(x):\n{body}"),
        );
        let files = [
            file(Some("a.py"), Some("a.py")),
            file(Some("b.py"), Some("b.py")),
        ];
        let mut hunks: [Hunk; 0] = [];
        let s = run(
            &files,
            &[("a.py", &o), ("b.py", "")],
            &[("a.py", ""), ("b.py", &n)],
            &mut hunks,
        );
        assert_eq!(s.len(), 1);
        assert_eq!(
            s[0].change,
            Change::Renamed {
                from: "old_name".into()
            }
        );
    }

    #[test]
    fn python_indent_change_is_not_format_only() {
        let mut hunks = [hunk(
            "h1",
            "a.py",
            1,
            1,
            &["-    do_cleanup()", "+        do_cleanup()"],
        )];
        run(&[], &[], &[], &mut hunks);
        assert_eq!(hunks[0].kind, HunkKind::Changed);
        let mut other = [hunk("h2", "a.ts", 1, 1, &["-    do()", "+        do()"])];
        run(&[], &[], &[], &mut other);
        assert_eq!(other[0].kind, HunkKind::FormatOnly);
    }

    #[test]
    fn cross_file_rename_with_renamed_callee_is_moved_and_changed() {
        let body = "    a = [i * 2 for i in range(10) if i % 3 == 0]\n    b = {k: v + 1 for k, v in zip(a, a)}\n";
        let o = format!("def same_origin(x):\n{body}    return port_or_default(b)\n");
        let n = format!("def _same_origin(x):\n{body}    return _port_or_default(b)\n");
        let files = [
            file(Some("a.py"), Some("a.py")),
            file(Some("b.py"), Some("b.py")),
        ];
        let mut hunks: [Hunk; 0] = [];
        let s = run(
            &files,
            &[("a.py", &o), ("b.py", "")],
            &[("a.py", ""), ("b.py", &n)],
            &mut hunks,
        );
        assert_eq!(s.len(), 1);
        assert!(matches!(&s[0].change, Change::MovedAndChanged { from } if from.path == "a.py"));
        assert_eq!(s[0].name, "_same_origin");
    }
}
