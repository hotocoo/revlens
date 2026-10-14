//! Parser for the unified diff produced by `git diff` (see `git::Repo::diff_patch`).

use anyhow::{Context, Result, bail};

use crate::hash::Fnv;
use crate::model::{Hunk, HunkId, HunkKind, Line, LineKind};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct FilePatch {
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    pub hunks: Vec<Hunk>,
    pub binary: bool,
}

/// Stable id: `"h_"` + 12 hex of FNV over path, starts and the changed lines
/// (trailing whitespace trimmed, context excluded).
pub fn hunk_id(path: &str, old_start: u32, new_start: u32, lines: &[Line]) -> HunkId {
    let mut h = Fnv::default();
    h.field(path.as_bytes())
        .field(&old_start.to_le_bytes())
        .field(&new_start.to_le_bytes());
    for l in lines {
        let marker = match l.kind {
            LineKind::Context => continue,
            LineKind::Added => b'+',
            LineKind::Removed => b'-',
        };
        h.field(&[marker]).field(l.text.trim_end().as_bytes());
    }
    let v = h.finish();
    format!("h_{:012x}", (v ^ (v >> 48)) & 0xffff_ffff_ffff)
}

pub fn parse(patch: &str) -> Result<Vec<FilePatch>> {
    let text = patch.strip_suffix('\n').unwrap_or(patch);
    let mut p = Parser::default();
    if !text.is_empty() {
        for (i, line) in text.split('\n').enumerate() {
            p.feed(line)
                .with_context(|| format!("patch line {}", i + 1))?;
        }
    }
    p.finish()
}

struct Open {
    hunk: Hunk,
    old_left: u32,
    new_left: u32,
}

#[derive(Default)]
struct Parser {
    files: Vec<FilePatch>,
    cur: Option<FilePatch>,
    open: Option<Open>,
}

impl Parser {
    fn feed(&mut self, line: &str) -> Result<()> {
        if line.starts_with('\\') {
            return Ok(()); // "\ No newline at end of file"
        }
        if self.open.is_some() {
            return self.body(line);
        }
        if let Some(rest) = line.strip_prefix("diff --git ") {
            self.finish_file()?;
            let (old_path, new_path) = parse_header(rest);
            self.cur = Some(FilePatch {
                old_path,
                new_path,
                ..FilePatch::default()
            });
            return Ok(());
        }
        if line.starts_with("@@ ") {
            self.open = Some(open_hunk(line)?);
            return self.close_if_done();
        }
        if let Some(f) = self.cur.as_mut() {
            file_header_line(f, line);
        }
        Ok(())
    }

    fn body(&mut self, line: &str) -> Result<()> {
        let open = self.open.as_mut().context("not in a hunk")?;
        let (kind, text) = match line.as_bytes().first() {
            Some(b' ') => (LineKind::Context, &line[1..]),
            Some(b'+') => (LineKind::Added, &line[1..]),
            Some(b'-') => (LineKind::Removed, &line[1..]),
            None => (LineKind::Context, ""), // blank context line with its space stripped
            _ => bail!("unexpected line in hunk: {line:?}"),
        };
        let (dec_old, dec_new) = match kind {
            LineKind::Context => (true, true),
            LineKind::Removed => (true, false),
            LineKind::Added => (false, true),
        };
        if dec_old {
            open.old_left = open
                .old_left
                .checked_sub(1)
                .context("hunk longer than header")?;
        }
        if dec_new {
            open.new_left = open
                .new_left
                .checked_sub(1)
                .context("hunk longer than header")?;
        }
        open.hunk.lines.push(Line {
            kind,
            text: text.to_string(),
        });
        self.close_if_done()
    }

    fn close_if_done(&mut self) -> Result<()> {
        let done = matches!(&self.open, Some(o) if o.old_left == 0 && o.new_left == 0);
        if done && let Some(o) = self.open.take() {
            let f = self.cur.as_mut().context("hunk outside of a file")?;
            f.hunks.push(o.hunk);
        }
        Ok(())
    }

    fn finish_file(&mut self) -> Result<()> {
        let Some(mut f) = self.cur.take() else {
            return Ok(());
        };
        let path = f
            .new_path
            .clone()
            .or_else(|| f.old_path.clone())
            .context("file entry without a path")?;
        for h in &mut f.hunks {
            h.id = hunk_id(&path, h.old_start, h.new_start, &h.lines);
            h.path = path.clone();
        }
        self.files.push(f);
        Ok(())
    }

    fn finish(mut self) -> Result<Vec<FilePatch>> {
        if self.open.is_some() {
            bail!("truncated hunk at end of patch");
        }
        self.finish_file()?;
        Ok(self.files)
    }
}

fn file_header_line(f: &mut FilePatch, line: &str) {
    if let Some(p) = line.strip_prefix("--- ") {
        f.old_path = side(p, "a/");
    } else if let Some(p) = line.strip_prefix("+++ ") {
        f.new_path = side(p, "b/");
    } else if line.starts_with("new file mode") {
        f.old_path = None;
    } else if line.starts_with("deleted file mode") {
        f.new_path = None;
    } else if let Some(p) = line
        .strip_prefix("rename from ")
        .or_else(|| line.strip_prefix("copy from "))
    {
        f.old_path = Some(unquote_any(p));
    } else if let Some(p) = line
        .strip_prefix("rename to ")
        .or_else(|| line.strip_prefix("copy to "))
    {
        f.new_path = Some(unquote_any(p));
    } else if line.starts_with("Binary files ") {
        f.binary = true;
    }
}

fn open_hunk(line: &str) -> Result<Open> {
    let rest = line.strip_prefix("@@ -").context("bad hunk header")?;
    let (old, rest) = rest.split_once(" +").context("bad hunk header")?;
    let (new, _) = rest.split_once(" @@").context("bad hunk header")?;
    let (old_start, old_len) = range(old)?;
    let (new_start, new_len) = range(new)?;
    let kind = if old_len == 0 {
        HunkKind::New
    } else if new_len == 0 {
        HunkKind::Deleted
    } else {
        HunkKind::Changed
    };
    Ok(Open {
        hunk: Hunk {
            id: String::new(),
            path: String::new(),
            old_start,
            old_len,
            new_start,
            new_len,
            lines: Vec::new(),
            kind,
            symbols: Vec::new(),
            signals: Vec::new(),
            score: 0,
        },
        old_left: old_len,
        new_left: new_len,
    })
}

/// `"12,3"` or `"12"` (length omitted means 1).
fn range(s: &str) -> Result<(u32, u32)> {
    let num = |t: &str| {
        t.parse::<u32>()
            .with_context(|| format!("bad number {t:?}"))
    };
    match s.split_once(',') {
        Some((a, b)) => Ok((num(a)?, num(b)?)),
        None => Ok((num(s)?, 1)),
    }
}

/// Path from a `---`/`+++` line: `/dev/null` is `None`; prefix and trailing tab are stripped.
fn side(p: &str, prefix: &str) -> Option<String> {
    let p = p.strip_suffix('\t').unwrap_or(p);
    if p == "/dev/null" {
        return None;
    }
    let path = unquote_any(p);
    Some(match path.strip_prefix(prefix) {
        Some(stripped) => stripped.to_string(),
        None => path,
    })
}

/// Best-effort paths from `diff --git a/X b/Y`; `---`/`+++`/rename lines override them.
fn parse_header(rest: &str) -> (Option<String>, Option<String>) {
    let strip = |s: String, p: &str| s.strip_prefix(p).map(str::to_string).unwrap_or(s);
    match split_header(rest) {
        Some((a, b)) => (Some(strip(a, "a/")), Some(strip(b, "b/"))),
        None => (None, None),
    }
}

fn split_header(rest: &str) -> Option<(String, String)> {
    if rest.starts_with('"') {
        let (a, tail) = take_quoted(rest)?;
        return Some((a, unquote_any(tail.strip_prefix(' ')?)));
    }
    if let Some(i) = rest.find(" \"") {
        return Some((rest[..i].to_string(), unquote_any(&rest[i + 1..])));
    }
    // Same path on both sides: "a/P b/P" has length 2*len(P)+5, which survives spaces in P.
    let n = rest.len();
    if n % 2 == 1
        && rest.as_bytes()[n / 2] == b' '
        && let (Some(l), Some(r)) = (rest.get(..n / 2), rest.get(n / 2 + 1..))
        && l.get(2..).is_some()
        && l.get(2..) == r.get(2..)
    {
        return Some((l.to_string(), r.to_string()));
    }
    let i = rest.find(" b/")?;
    Some((rest[..i].to_string(), rest[i + 1..].to_string()))
}

fn unquote_any(s: &str) -> String {
    match take_quoted(s) {
        Some((u, _)) => u,
        None => s.to_string(),
    }
}

/// Decode a git C-quoted string at the start of `s`; returns it and the remainder.
fn take_quoted(s: &str) -> Option<(String, &str)> {
    let b = s.as_bytes();
    if b.first() != Some(&b'"') {
        return None;
    }
    let mut i = 1;
    while i < b.len() && b[i] != b'"' {
        i += if b[i] == b'\\' { 2 } else { 1 };
    }
    if i >= b.len() {
        return None;
    }
    Some((unescape(&b[1..i]), &s[i + 1..]))
}

fn unescape(b: &[u8]) -> String {
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'\\' {
            out.push(b[i]);
            i += 1;
            continue;
        }
        i += 1;
        let Some(&c) = b.get(i) else { break };
        i += 1;
        out.push(match c {
            b'a' => 7,
            b'b' => 8,
            b'f' => 12,
            b'n' => b'\n',
            b'r' => b'\r',
            b't' => b'\t',
            b'v' => 11,
            b'0'..=b'7' => {
                let mut v = u32::from(c - b'0');
                for _ in 0..2 {
                    match b.get(i) {
                        Some(d @ b'0'..=b'7') => {
                            v = v * 8 + u32::from(d - b'0');
                            i += 1;
                        }
                        _ => break,
                    }
                }
                (v & 0xff) as u8
            }
            other => other,
        });
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn one(patch: &str) -> FilePatch {
        let mut v = parse(patch).expect("parse");
        assert_eq!(v.len(), 1);
        v.remove(0)
    }

    #[test]
    fn modify_with_omitted_lengths_and_no_newline_marker() {
        let f = one(
            "diff --git a/x.rs b/x.rs\nindex 1..2 100644\n--- a/x.rs\n+++ b/x.rs\n@@ -1 +1 @@\n-old\n\\ No newline at end of file\n+new\n\\ No newline at end of file\n",
        );
        assert_eq!(f.old_path.as_deref(), Some("x.rs"));
        assert_eq!(f.new_path.as_deref(), Some("x.rs"));
        let h = &f.hunks[0];
        assert_eq!(
            (h.old_start, h.old_len, h.new_start, h.new_len),
            (1, 1, 1, 1)
        );
        assert_eq!(h.kind, HunkKind::Changed);
        assert_eq!(h.lines.len(), 2);
        assert_eq!(h.path, "x.rs");
        assert_eq!(h.id, hunk_id("x.rs", 1, 1, &h.lines));
        assert!(h.symbols.is_empty() && h.signals.is_empty() && h.score == 0);
    }

    #[test]
    fn new_and_deleted_files() {
        let n = one(
            "diff --git a/n.txt b/n.txt\nnew file mode 100644\nindex 0..1\n--- /dev/null\n+++ b/n.txt\n@@ -0,0 +1,2 @@\n+a\n+b\n",
        );
        assert_eq!((n.old_path, n.new_path.as_deref()), (None, Some("n.txt")));
        assert_eq!(n.hunks[0].kind, HunkKind::New);
        let d = one(
            "diff --git a/d.txt b/d.txt\ndeleted file mode 100644\nindex 1..0\n--- a/d.txt\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-a\n-b\n",
        );
        assert_eq!((d.old_path.as_deref(), d.new_path), (Some("d.txt"), None));
        assert_eq!(d.hunks[0].kind, HunkKind::Deleted);
        assert_eq!(d.hunks[0].path, "d.txt");
    }

    #[test]
    fn empty_new_and_deleted_files_have_no_hunks() {
        let n = one("diff --git a/e b/e\nnew file mode 100644\nindex 0..e69de29\n");
        assert_eq!((n.old_path, n.new_path.as_deref()), (None, Some("e")));
        let d = one("diff --git a/e b/e\ndeleted file mode 100644\nindex e69de29..0\n");
        assert_eq!((d.old_path.as_deref(), d.new_path), (Some("e"), None));
    }

    #[test]
    fn rename_and_copy() {
        let r = one(
            "diff --git a/old name.rs b/new name.rs\nsimilarity index 100%\nrename from old name.rs\nrename to new name.rs\n",
        );
        assert_eq!(r.old_path.as_deref(), Some("old name.rs"));
        assert_eq!(r.new_path.as_deref(), Some("new name.rs"));
        assert!(r.hunks.is_empty());
        let c = one(
            "diff --git a/s.rs b/c.rs\nsimilarity index 90%\ncopy from s.rs\ncopy to c.rs\nindex 1..2 100644\n--- a/s.rs\n+++ b/c.rs\n@@ -1 +1,2 @@\n x\n+y\n",
        );
        assert_eq!(c.old_path.as_deref(), Some("s.rs"));
        assert_eq!(c.new_path.as_deref(), Some("c.rs"));
        assert_eq!(c.hunks[0].path, "c.rs");
    }

    #[test]
    fn binary_files() {
        let f = one(
            "diff --git a/i.png b/i.png\nindex 1..2 100644\nBinary files a/i.png and b/i.png differ\n",
        );
        assert!(f.binary && f.hunks.is_empty());
        let n = one(
            "diff --git a/i.png b/i.png\nnew file mode 100644\nindex 0..2\nBinary files /dev/null and b/i.png differ\n",
        );
        assert!(n.binary);
        assert_eq!((n.old_path, n.new_path.as_deref()), (None, Some("i.png")));
    }

    #[test]
    fn mode_only_change() {
        let f = one("diff --git a/run.sh b/run.sh\nold mode 100644\nnew mode 100755\n");
        assert_eq!(f.old_path.as_deref(), Some("run.sh"));
        assert_eq!(f.new_path.as_deref(), Some("run.sh"));
        assert!(f.hunks.is_empty() && !f.binary);
    }

    #[test]
    fn mode_only_change_with_space_and_unicode_path() {
        let f = one(
            "diff --git a/dir é/my file.sh b/dir é/my file.sh\nold mode 100644\nnew mode 100755\n",
        );
        assert_eq!(f.new_path.as_deref(), Some("dir é/my file.sh"));
        assert_eq!(f.old_path.as_deref(), Some("dir é/my file.sh"));
    }

    #[test]
    fn quoted_paths_with_octal_escapes() {
        let f = one(
            "diff --git \"a/caf\\303\\251 x.txt\" \"b/caf\\303\\251 x.txt\"\nindex 1..2 100644\n--- \"a/caf\\303\\251 x.txt\"\n+++ \"b/caf\\303\\251 x.txt\"\n@@ -1 +1 @@\n-a\n+b\n",
        );
        assert_eq!(f.new_path.as_deref(), Some("café x.txt"));
        assert_eq!(f.old_path.as_deref(), Some("café x.txt"));
        let g = one(
            "diff --git \"a/q\\\"t\\t.txt\" \"b/q\\\"t\\t.txt\"\nold mode 100644\nnew mode 100755\n",
        );
        assert_eq!(g.new_path.as_deref(), Some("q\"t\t.txt"));
    }

    #[test]
    fn unquoted_space_path_with_trailing_tab() {
        let f = one(
            "diff --git a/my file.txt b/my file.txt\nindex 1..2 100644\n--- a/my file.txt\t\n+++ b/my file.txt\t\n@@ -1 +1 @@\n-a\n+b\n",
        );
        assert_eq!(f.new_path.as_deref(), Some("my file.txt"));
        assert_eq!(f.old_path.as_deref(), Some("my file.txt"));
    }

    #[test]
    fn body_lines_that_look_like_headers() {
        let f = one(
            "diff --git a/x b/x\nindex 1..2 100644\n--- a/x\n+++ b/x\n@@ -1 +1 @@\n--- a/fake\n+++ b/fake\n",
        );
        assert_eq!(f.hunks.len(), 1);
        assert_eq!(f.hunks[0].lines.len(), 2);
        assert_eq!(f.hunks[0].lines[0].text, "-- a/fake");
        assert_eq!(f.new_path.as_deref(), Some("x"));
    }

    #[test]
    fn multiple_files_and_hunks() {
        let v = parse("diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-1\n+2\n@@ -5 +5 @@\n-5\n+6\ndiff --git a/b b/b\n--- a/b\n+++ b/b\n@@ -1 +1 @@\n-x\n+y\n").expect("parse");
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].hunks.len(), 2);
        assert_ne!(v[0].hunks[0].id, v[0].hunks[1].id);
        assert_eq!(v[1].hunks[0].path, "b");
    }

    #[test]
    fn empty_and_preamble_only_input() {
        assert!(parse("").expect("empty").is_empty());
        assert!(parse("garbage\n").expect("garbage").is_empty());
    }

    #[test]
    fn errors() {
        let head = "diff --git a/x b/x\n--- a/x\n+++ b/x\n";
        assert!(parse(&format!("{head}@@ -1,3 +1,3 @@\n-a\n")).is_err()); // truncated
        assert!(parse(&format!("{head}@@ -1 +1 @@\n-a\n-b\n+c\n")).is_err()); // too long
        assert!(parse(&format!("{head}@@ nope\n")).is_err());
        assert!(parse(&format!("{head}@@ -x +1 @@\n")).is_err());
        assert!(parse(&format!("{head}@@ -1 +1 @@\n?a\n")).is_err());
        assert!(parse("diff --git x\n").is_err()); // no path
    }

    #[test]
    fn hunk_id_rules() {
        let l = |kind, t: &str| Line {
            kind,
            text: t.into(),
        };
        let a = [l(LineKind::Context, "ctx"), l(LineKind::Added, "x  ")];
        let b = [l(LineKind::Context, "other"), l(LineKind::Added, "x")];
        let id = hunk_id("p", 1, 2, &a);
        assert_eq!(id, hunk_id("p", 1, 2, &b));
        assert!(id.starts_with("h_") && id.len() == 14);
        assert!(
            id[2..]
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        );
        assert_ne!(id, hunk_id("q", 1, 2, &a));
        assert_ne!(id, hunk_id("p", 2, 2, &a));
        assert_ne!(id, hunk_id("p", 1, 3, &a));
        let removed = [l(LineKind::Removed, "x")];
        assert_ne!(
            hunk_id("p", 1, 2, &removed),
            hunk_id("p", 1, 2, &[l(LineKind::Added, "x")])
        );
    }

    type RawHunk = (u32, u32, Vec<Line>);

    fn arb_line() -> impl Strategy<Value = Line> {
        (
            prop_oneof![
                Just(LineKind::Context),
                Just(LineKind::Added),
                Just(LineKind::Removed)
            ],
            "[ -~]{0,12}",
        )
            .prop_map(|(kind, text)| Line { kind, text })
    }

    fn arb_hunk() -> impl Strategy<Value = RawHunk> {
        (
            1u32..5000,
            1u32..5000,
            proptest::collection::vec(arb_line(), 1..12),
        )
    }

    fn counts(lines: &[Line]) -> (u32, u32) {
        let n = |f: fn(&LineKind) -> bool| lines.iter().filter(|l| f(&l.kind)).count() as u32;
        (n(|k| *k != LineKind::Added), n(|k| *k != LineKind::Removed))
    }

    fn render(path: &str, hunks: &[RawHunk]) -> String {
        let mut s = format!("diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n");
        for (os, ns, lines) in hunks {
            let (ol, nl) = counts(lines);
            s += &format!("@@ -{os},{ol} +{ns},{nl} @@\n");
            for l in lines {
                s.push(match l.kind {
                    LineKind::Context => ' ',
                    LineKind::Added => '+',
                    LineKind::Removed => '-',
                });
                s += &l.text;
                s.push('\n');
            }
        }
        s
    }

    fn expected(path: &str, (os, ns, lines): &RawHunk) -> Hunk {
        let (ol, nl) = counts(lines);
        Hunk {
            id: hunk_id(path, *os, *ns, lines),
            path: path.to_string(),
            old_start: *os,
            old_len: ol,
            new_start: *ns,
            new_len: nl,
            lines: lines.clone(),
            kind: if ol == 0 {
                HunkKind::New
            } else if nl == 0 {
                HunkKind::Deleted
            } else {
                HunkKind::Changed
            },
            symbols: vec![],
            signals: vec![],
            score: 0,
        }
    }

    proptest! {
        #[test]
        fn render_then_parse_round_trips(
            path in "[a-z]{1,8}(/[a-z]{1,8}){0,2}\\.rs",
            hunks in proptest::collection::vec(arb_hunk(), 1..5),
        ) {
            let parsed = parse(&render(&path, &hunks)).expect("parse");
            prop_assert_eq!(parsed.len(), 1);
            let want: Vec<Hunk> = hunks.iter().map(|h| expected(&path, h)).collect();
            prop_assert_eq!(&parsed[0].hunks, &want);
            let again = parse(&render(&path, &hunks)).expect("parse");
            prop_assert_eq!(parsed, again);
        }

        #[test]
        fn hunk_id_is_deterministic(
            path in "[a-z]{1,8}",
            (os, ns, lines) in arb_hunk(),
        ) {
            prop_assert_eq!(hunk_id(&path, os, ns, &lines), hunk_id(&path, os, ns, &lines.clone()));
        }
    }
}
