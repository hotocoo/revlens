//! Structural fingerprints of syntax nodes plus line-based move / format-only detection.

use std::collections::HashMap;

use tree_sitter::Node;

use crate::hash::Fnv;
use crate::model::{Hunk, HunkId, LineKind, Loc};

/// Node kinds whose subtree is collapsed to the kind name in `shape`.
const LITERAL_KINDS: &[&str] = &[
    "string",
    "template_string",
    "number",
    "integer",
    "float",
    "regex",
    "concatenated_string",
];

/// Node kinds treated as renamable identifiers in `shape`.
const IDENT_KINDS: &[&str] = &[
    "identifier",
    "property_identifier",
    "type_identifier",
    "shorthand_property_identifier",
    "shorthand_property_identifier_pattern",
    "statement_identifier",
    "private_property_identifier",
];

fn is_comment(n: Node) -> bool {
    n.kind().contains("comment")
}

fn text<'a>(n: Node, src: &'a [u8]) -> &'a [u8] {
    src.get(n.start_byte()..n.end_byte()).unwrap_or_default()
}

/// Hash of leaf token kinds and texts, comments skipped.
pub fn exact(node: Node, src: &[u8]) -> u64 {
    let mut h = Fnv::default();
    exact_walk(node, src, &mut h);
    h.finish()
}

fn exact_walk(n: Node, src: &[u8], h: &mut Fnv) {
    if is_comment(n) {
        return;
    }
    if n.child_count() == 0 {
        h.field(n.kind().as_bytes()).field(text(n, src));
        return;
    }
    let mut c = n.walk();
    for ch in n.children(&mut c) {
        exact_walk(ch, src, h);
    }
}

/// Like `exact`, but identifiers spelled `name` (the definition's own name, recursive calls
/// included) hash as one placeholder: equal values mean the same code up to renaming `name`.
pub fn exact_unnamed(node: Node, src: &[u8], name: &str) -> u64 {
    let mut h = Fnv::default();
    unnamed_walk(node, src, name.as_bytes(), &mut h);
    h.finish()
}

fn unnamed_walk(n: Node, src: &[u8], name: &[u8], h: &mut Fnv) {
    if is_comment(n) {
        return;
    }
    if n.child_count() == 0 {
        let t = text(n, src);
        let t = if IDENT_KINDS.contains(&n.kind()) && t == name {
            b"$self".as_slice()
        } else {
            t
        };
        h.field(n.kind().as_bytes()).field(t);
        return;
    }
    let mut c = n.walk();
    for ch in n.children(&mut c) {
        unnamed_walk(ch, src, name, h);
    }
}

/// Rename-, whitespace- and comment-insensitive structure hash, with the node count.
pub fn shape(node: Node, src: &[u8]) -> (u64, usize) {
    let mut st = Shape {
        h: Fnv::default(),
        names: HashMap::new(),
        count: 0,
    };
    st.walk(node, src);
    (st.h.finish(), st.count)
}

struct Shape {
    h: Fnv,
    names: HashMap<Vec<u8>, usize>,
    count: usize,
}

impl Shape {
    fn walk(&mut self, n: Node, src: &[u8]) {
        if is_comment(n) {
            return;
        }
        self.count += 1;
        let kind = n.kind();
        if LITERAL_KINDS.contains(&kind) {
            self.h.field(kind.as_bytes());
        } else if IDENT_KINDS.contains(&kind) {
            let next = self.names.len();
            let id = *self.names.entry(text(n, src).to_vec()).or_insert(next);
            self.h.field(b"$").field(id.to_string().as_bytes());
        } else if n.child_count() == 0 {
            self.h.field(kind.as_bytes()).field(text(n, src));
        } else {
            self.h.field(kind.as_bytes()).write(b"(");
            let mut c = n.walk();
            for ch in n.children(&mut c) {
                self.walk(ch, src);
            }
            self.h.write(b")");
        }
    }
}

/// Removed and added lines are identical once whitespace outside string literals is removed.
/// With `indent_sensitive`, leading indentation of each line must also match.
pub fn is_format_only(hunk: &Hunk, indent_sensitive: bool) -> bool {
    let squash = |kind| -> (bool, String) {
        let mut any = false;
        let mut out = String::new();
        for l in hunk.lines.iter().filter(|l| l.kind == kind) {
            any = true;
            if indent_sensitive {
                let body = l.text.trim_start();
                out.push_str(&l.text[..l.text.len() - body.len()]);
            }
            squash_line(&l.text, &mut out);
            out.push('\n');
        }
        (any, out)
    };
    let (has_rm, rm) = squash(LineKind::Removed);
    let (has_add, add) = squash(LineKind::Added);
    has_rm && has_add && rm == add
}

/// Append `line` minus whitespace, keeping whitespace inside quotes. An unbalanced quote
/// (an apostrophe in a comment) keeps the rest, which only makes the result stricter.
fn squash_line(line: &str, out: &mut String) {
    let (mut quote, mut escaped) = (None::<char>, false);
    for c in line.chars() {
        match quote {
            Some(q) => {
                if !escaped && c == q {
                    quote = None;
                }
                escaped = !escaped && c == '\\';
            }
            None if matches!(c, '"' | '\'' | '`') => quote = Some(c),
            None if c.is_whitespace() => continue,
            None => {}
        }
        out.push(c);
    }
}

/// Consecutive same-kind changed lines (trimmed, non-empty) with their line numbers.
struct Run {
    kind: LineKind,
    lines: Vec<(String, u32)>,
}

fn runs(h: &Hunk) -> Vec<Run> {
    let mut out: Vec<Run> = Vec::new();
    let (mut old, mut new) = (h.old_start, h.new_start);
    let mut prev: Option<LineKind> = None;
    for l in &h.lines {
        let no = match l.kind {
            LineKind::Removed => old,
            _ => new,
        };
        if l.kind != LineKind::Added {
            old += 1;
        }
        if l.kind != LineKind::Removed {
            new += 1;
        }
        if l.kind == LineKind::Context {
            prev = None;
            continue;
        }
        if prev != Some(l.kind) {
            out.push(Run {
                kind: l.kind,
                lines: Vec::new(),
            });
            prev = Some(l.kind);
        }
        let t = l.text.trim();
        if !t.is_empty()
            && let Some(r) = out.last_mut()
        {
            r.lines.push((t.to_string(), no));
        }
    }
    out
}

type Index<'a> = HashMap<&'a str, Vec<(usize, usize, usize)>>;

/// Longest match of `run[i..]` against runs of the opposite kind in other hunks:
/// (length, counterpart hunk index, counterpart line).
fn best_match(
    all: &[Vec<Run>],
    idx: &Index,
    hi: usize,
    run: &Run,
    i: usize,
) -> Option<(usize, usize, u32)> {
    let mut best: Option<(usize, usize, u32)> = None;
    for &(oh, or, op) in idx.get(run.lines[i].0.as_str())? {
        let other = &all[oh][or];
        if oh == hi || other.kind == run.kind {
            continue;
        }
        let len = run.lines[i..]
            .iter()
            .zip(&other.lines[op..])
            .take_while(|(a, b)| a.0 == b.0)
            .count();
        if best.is_none_or(|b| len > b.0) {
            best = Some((len, oh, other.lines[op].1));
        }
    }
    best
}

/// Hunks whose changed lines ALL sit in blocks of `min_block`+ lines that reappear (opposite
/// kind) in a different hunk; a hunk with any unmatched (edited) line is never reported. Returns the first counterpart per hunk.
pub fn moved_blocks(hunks: &[Hunk], min_block: usize) -> Vec<(HunkId, Loc)> {
    let all: Vec<Vec<Run>> = hunks.iter().map(runs).collect();
    let mut idx: Index = HashMap::new();
    for (hi, rs) in all.iter().enumerate() {
        for (ri, r) in rs.iter().enumerate() {
            for (p, (t, _)) in r.lines.iter().enumerate() {
                idx.entry(t.as_str()).or_default().push((hi, ri, p));
            }
        }
    }
    let mut out = Vec::new();
    for (hi, rs) in all.iter().enumerate() {
        let total: usize = rs.iter().map(|r| r.lines.len()).sum();
        let (mut moved, mut first) = (0usize, None);
        for r in rs {
            let mut i = 0;
            while i < r.lines.len() {
                match best_match(&all, &idx, hi, r, i) {
                    Some((len, oh, line)) if len >= min_block => {
                        moved += len;
                        first.get_or_insert((oh, line));
                        i += len;
                    }
                    _ => i += 1,
                }
            }
        }
        if let Some((oh, line)) = first
            && moved == total
        {
            out.push((
                hunks[hi].id.clone(),
                Loc {
                    path: hunks[oh].path.clone(),
                    line,
                },
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{HunkKind, Line};
    use tree_sitter::Parser;

    fn parse_ts(s: &str) -> tree_sitter::Tree {
        let mut p = Parser::new();
        p.set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into())
            .expect("lang");
        p.parse(s, None).expect("parse")
    }
    fn parse_py(s: &str) -> tree_sitter::Tree {
        let mut p = Parser::new();
        p.set_language(&tree_sitter_python::LANGUAGE.into())
            .expect("lang");
        p.parse(s, None).expect("parse")
    }
    fn sh(s: &str) -> (u64, usize) {
        shape(parse_ts(s).root_node(), s.as_bytes())
    }
    fn ex(s: &str) -> u64 {
        exact(parse_ts(s).root_node(), s.as_bytes())
    }

    #[test]
    fn shape_invariant_under_rename_ws_comments_literals() {
        let a = "function f(a, b) { return a + b * 2; } // c";
        let b = "/* x */ function  g(x,\n y) {\n return x + y * 9;\n}";
        assert_eq!(sh(a).0, sh(b).0);
        assert_eq!(sh(a).1, sh(b).1);
        assert_eq!(sh("let s = 'a';").0, sh("let s = \"zz\";").0);
    }

    #[test]
    fn shape_differs_on_structure_and_inconsistent_rename() {
        assert_ne!(
            sh("function f(a, b) { return a + b; }").0,
            sh("function f(a, b) { return a - b; }").0
        );
        assert_ne!(
            sh("function f(a, b) { return a + b; }").0,
            sh("function f(a, b) { return a + a; }").0
        );
    }

    #[test]
    fn exact_ignores_ws_comments_but_not_rename() {
        assert_eq!(ex("let a = 1; // hi"), ex("let   a =\n1;"));
        assert_ne!(ex("let a = 1;"), ex("let b = 1;"));
        assert_ne!(ex("let a = 1;"), ex("let a = 2;"));
    }

    #[test]
    fn python_shape() {
        let f = |s: &str| shape(parse_py(s).root_node(), s.as_bytes()).0;
        assert_eq!(
            f("def f(a):\n    return a + 1  # c\n"),
            f("def g(z):\n    return z + 7\n")
        );
        assert_ne!(
            f("def f(a):\n    return a + 1\n"),
            f("def f(a):\n    return a * 1\n")
        );
    }

    #[test]
    fn unicode_ok() {
        assert_eq!(sh("const é = 'ü';").0, sh("const x = 'y';").0);
    }

    fn hunk(id: &str, path: &str, old: u32, new: u32, lines: &[(LineKind, &str)]) -> Hunk {
        Hunk {
            id: id.into(),
            path: path.into(),
            old_start: old,
            old_len: 0,
            new_start: new,
            new_len: 0,
            lines: lines
                .iter()
                .map(|(k, t)| Line {
                    kind: *k,
                    text: (*t).into(),
                })
                .collect(),
            kind: HunkKind::Changed,
            symbols: vec![],
            signals: vec![],
            score: 0,
        }
    }
    use LineKind::{Added as A, Context as C, Removed as R};

    #[test]
    fn format_only() {
        let h = hunk("a", "x", 1, 1, &[(R, "a( b,c )"), (A, "a(b, c)")]);
        assert!(is_format_only(&h, false));
        assert!(!is_format_only(&hunk("a", "x", 1, 1, &[(R, "a")]), false));
        assert!(!is_format_only(
            &hunk("a", "x", 1, 1, &[(R, "a"), (A, "b")]),
            false
        ));
    }

    #[test]
    fn indentation_change_is_not_format_only_when_sensitive() {
        let h = hunk("a", "x", 1, 1, &[(R, "    do()"), (A, "        do()")]);
        assert!(is_format_only(&h, false));
        assert!(!is_format_only(&h, true));
        let same = hunk("a", "x", 1, 1, &[(R, "  x=1"), (A, "  x = 1")]);
        assert!(is_format_only(&same, true));
    }

    #[test]
    fn whitespace_inside_string_literal_is_not_format_only() {
        let h = hunk("a", "x", 1, 1, &[(R, "s = \"a b\""), (A, "s = \"a  b\"")]);
        assert!(!is_format_only(&h, false));
        let esc = hunk("a", "x", 1, 1, &[(R, "s='a\\' b'"), (A, "s = 'a\\' b'")]);
        assert!(is_format_only(&esc, false));
    }

    #[test]
    fn exact_unnamed_ignores_own_name_only() {
        let f = |s: &str, n: &str| exact_unnamed(parse_py(s).root_node(), s.as_bytes(), n);
        let a = f("def f(n):\n    return f(n - 1)\n", "f");
        assert_eq!(a, f("def g(n):\n    return g(n - 1)\n", "g"));
        assert_ne!(a, f("def g(n):\n    return h(n - 1)\n", "g"));
        assert_ne!(a, f("def g(n):\n    return g(n - 2)\n", "g"));
    }

    #[test]
    fn detects_moved_block_both_sides() {
        let body = [(R, "  one();"), (R, "  two();"), (R, "  three();")];
        let h1 = hunk("h1", "a.ts", 10, 10, &body);
        let h2 = hunk(
            "h2",
            "b.ts",
            1,
            5,
            &[(C, "ctx"), (A, "one();"), (A, "two();"), (A, "three();")],
        );
        let got = moved_blocks(&[h1, h2], 3);
        assert_eq!(
            got,
            vec![
                (
                    "h1".to_string(),
                    Loc {
                        path: "b.ts".into(),
                        line: 6
                    }
                ),
                (
                    "h2".to_string(),
                    Loc {
                        path: "a.ts".into(),
                        line: 10
                    }
                ),
            ]
        );
    }

    #[test]
    fn short_blocks_partial_and_same_hunk_not_moved() {
        let h1 = hunk("h1", "a", 1, 1, &[(R, "x"), (R, "y")]);
        let h2 = hunk("h2", "b", 1, 1, &[(A, "x"), (A, "y")]);
        assert!(moved_blocks(&[h1, h2], 3).is_empty());
        // same hunk replaced in place
        let h = hunk(
            "h",
            "a",
            1,
            1,
            &[(R, "p"), (R, "q"), (R, "r"), (A, "p"), (A, "q"), (A, "r")],
        );
        assert!(moved_blocks(&[h], 3).is_empty());
        // below pct
        let h1 = hunk(
            "h1",
            "a",
            1,
            1,
            &[
                (R, "p"),
                (R, "q"),
                (R, "r"),
                (A, "n1"),
                (A, "n2"),
                (A, "n3"),
                (A, "n4"),
            ],
        );
        let h2 = hunk("h2", "b", 1, 1, &[(A, "p"), (A, "q"), (A, "r")]);
        let got = moved_blocks(&[h1, h2], 3);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, "h2");
    }

    #[test]
    fn hunk_with_edited_lines_is_not_moved() {
        let texts: Vec<String> = (0..10).map(|i| format!("x{i}();")).collect();
        let lines = |k| texts.iter().map(|t| (k, t.as_str())).collect::<Vec<_>>();
        let h1 = hunk("h1", "a", 1, 1, &lines(R));
        let mut added = lines(A);
        added.push((A, "eval(x);"));
        let h2 = hunk("h2", "b", 1, 1, &added);
        let got = moved_blocks(&[h1, h2], 3);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, "h1");
    }
}
