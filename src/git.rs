//! Thin wrapper over the `git` CLI. User git config must not change what we parse, so the
//! settings that affect diff output are pinned on every invocation.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

use crate::model::{FileDiff, FileStatus};

/// Pinned via `-c` so user/system config cannot change diff text or hunk boundaries.
const PINNED_CONFIG: &[&str] = &[
    "core.quotepath=off",
    "diff.noprefix=false",
    "diff.mnemonicPrefix=false",
    "diff.algorithm=myers",
    "diff.renames=true",
    "diff.relative=false",
    "diff.submodule=short",
    "color.ui=never",
];

const SCRUBBED_ENV: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_EXTERNAL_DIFF",
];

#[derive(Debug, Clone)]
pub struct Repo {
    root: PathBuf,
}

impl Repo {
    pub fn discover(path: &Path) -> Result<Repo> {
        let out = run(
            git_cmd(path).args(["rev-parse", "--show-toplevel"]),
            "rev-parse --show-toplevel",
        )?;
        let top = String::from_utf8_lossy(&out);
        let top = top.trim_end_matches('\n');
        if top.is_empty() {
            bail!("{} is not inside a git work tree", path.display());
        }
        Ok(Repo {
            root: PathBuf::from(top),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Full commit sha for `rev`.
    pub fn rev_parse(&self, rev: &str) -> Result<String> {
        check_rev(rev)?;
        let arg = format!("{rev}^{{commit}}");
        self.text(&["rev-parse", "--verify", &arg])
    }

    pub fn merge_base(&self, a: &str, b: &str) -> Result<String> {
        check_rev(a)?;
        check_rev(b)?;
        self.text(&["merge-base", a, b])
    }

    pub fn diff_patch(&self, base: &str, head: &str, similarity: u32) -> Result<String> {
        check_rev(base)?;
        check_rev(head)?;
        let (m, c) = (format!("-M{similarity}%"), format!("-C{similarity}%"));
        let out = self.git(&[
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            &m,
            &c,
            "-U3",
            "--src-prefix=a/",
            "--dst-prefix=b/",
            base,
            head,
            "--",
        ])?;
        Ok(String::from_utf8_lossy(&out).into_owned())
    }

    /// File list with statuses; `lang` and `generated` are left for the caller to fill.
    pub fn diff_files(&self, base: &str, head: &str, similarity: u32) -> Result<Vec<FileDiff>> {
        check_rev(base)?;
        check_rev(head)?;
        let (m, c) = (format!("-M{similarity}%"), format!("-C{similarity}%"));
        let out = self.git(&[
            "diff",
            "--raw",
            "-z",
            "--no-abbrev",
            "--no-ext-diff",
            &m,
            &c,
            base,
            head,
            "--",
        ])?;
        parse_raw(&String::from_utf8_lossy(&out))
    }

    pub fn ls_tree(&self, rev: &str) -> Result<Vec<String>> {
        check_rev(rev)?;
        let out = self.git(&["ls-tree", "-r", "-z", "--name-only", rev])?;
        Ok(String::from_utf8_lossy(&out)
            .split('\0')
            .filter(|p| !p.is_empty())
            .map(str::to_string)
            .collect())
    }

    /// Contents of `paths` at `rev` through one `git cat-file --batch` process.
    /// `None` for missing paths, non-UTF-8 content, and non-blobs.
    pub fn read_blobs(&self, rev: &str, paths: &[String]) -> Result<Vec<(String, Option<String>)>> {
        check_rev(rev)?;
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        let mut cmd = git_cmd(&self.root);
        cmd.args(["cat-file", "--batch"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().context("spawning git cat-file")?;
        let mut stdin = child.stdin.take().context("git cat-file has no stdin")?;
        let stdout = child.stdout.take().context("git cat-file has no stdout")?;
        let mut stdout = BufReader::new(stdout);

        let mut input = Vec::new();
        for p in paths.iter().filter(|p| !p.contains('\n')) {
            input.extend_from_slice(format!("{rev}:{p}\n").as_bytes());
        }
        let result = std::thread::scope(|s| {
            // Writer runs alongside the reader so neither pipe can fill and deadlock.
            s.spawn(move || {
                let _ = stdin.write_all(&input);
            });
            let r = read_all(&mut stdout, paths);
            if r.is_err() {
                let _ = child.kill(); // unblocks the writer
            }
            r
        });
        let mut stderr = String::new();
        if let Some(mut e) = child.stderr.take() {
            let _ = e.read_to_string(&mut stderr);
        }
        let _ = child.wait();
        result.with_context(|| format!("git cat-file --batch: {}", stderr.trim()))
    }

    fn git(&self, args: &[&str]) -> Result<Vec<u8>> {
        run(git_cmd(&self.root).args(args), &args.join(" "))
    }

    fn text(&self, args: &[&str]) -> Result<String> {
        Ok(String::from_utf8_lossy(&self.git(args)?).trim().to_string())
    }
}

/// `a...b` / `a..b` -> (a, b); bare `a` -> (a, "HEAD").
pub fn parse_range(spec: &str) -> Result<(String, String)> {
    let (a, b) = match spec.split_once("...").or_else(|| spec.split_once("..")) {
        Some((a, b)) => (a, if b.is_empty() { "HEAD" } else { b }),
        None => (spec, "HEAD"),
    };
    if a.is_empty() {
        bail!("invalid range {spec:?}: missing base");
    }
    check_rev(a)?;
    check_rev(b)?;
    Ok((a.to_string(), b.to_string()))
}

/// Revs reach git as arguments (or stdin lines), so reject anything option- or line-like.
fn check_rev(rev: &str) -> Result<()> {
    if rev.is_empty() || rev.starts_with('-') || rev.contains(['\n', '\0']) {
        bail!("invalid revision {rev:?}");
    }
    Ok(())
}

fn git_cmd(dir: &Path) -> Command {
    let mut c = Command::new("git");
    c.arg("-C").arg(dir);
    for kv in PINNED_CONFIG {
        c.arg("-c").arg(kv);
    }
    for v in SCRUBBED_ENV {
        c.env_remove(v);
    }
    c.env("GIT_OPTIONAL_LOCKS", "0");
    c
}

fn run(cmd: &mut Command, what: &str) -> Result<Vec<u8>> {
    let out = cmd
        .output()
        .with_context(|| format!("spawning git {what}"))?;
    if !out.status.success() {
        bail!(
            "git {what} failed ({}): {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(out.stdout)
}

/// `git diff --raw -z`: `:modes shas STATUS\0path\0[path2\0]` repeated.
fn parse_raw(text: &str) -> Result<Vec<FileDiff>> {
    let mut it = text.split('\0').filter(|s| !s.is_empty());
    let mut files = Vec::new();
    while let Some(meta) = it.next() {
        let status = meta
            .strip_prefix(':')
            .and_then(|m| m.split_whitespace().last())
            .with_context(|| format!("bad raw diff entry {meta:?}"))?;
        let mut path = || {
            it.next()
                .map(str::to_string)
                .context("missing path in raw diff")
        };
        files.push(raw_entry(status, &mut path)?);
    }
    Ok(files)
}

fn raw_entry(status: &str, path: &mut dyn FnMut() -> Result<String>) -> Result<FileDiff> {
    let code = status.chars().next().context("empty status")?;
    let num = &status[code.len_utf8()..];
    let entry = |status, old_path, new_path| FileDiff {
        old_path,
        new_path,
        status,
        lang: None,
        generated: false,
    };
    Ok(match code {
        'A' => entry(FileStatus::Added, None, Some(path()?)),
        'D' => entry(FileStatus::Deleted, Some(path()?), None),
        'M' | 'T' => {
            let p = path()?;
            entry(FileStatus::Modified, Some(p.clone()), Some(p))
        }
        'R' | 'C' => {
            let (old, new) = (path()?, path()?);
            let similarity = num
                .parse::<u8>()
                .with_context(|| format!("bad similarity in status {status:?}"))?;
            let st = if code == 'R' {
                FileStatus::Renamed { similarity }
            } else {
                FileStatus::Copied { similarity }
            };
            entry(st, Some(old), Some(new))
        }
        _ => bail!("unsupported diff status {status:?}"),
    })
}

fn read_all(r: &mut impl BufRead, paths: &[String]) -> Result<Vec<(String, Option<String>)>> {
    paths
        .iter()
        .map(|p| {
            let content = if p.contains('\n') {
                None
            } else {
                read_record(r)?
            };
            Ok((p.clone(), content))
        })
        .collect()
}

/// One `cat-file --batch` record: `<oid> <type> <size>\n<content>\n` or `<name> missing\n`.
fn read_record(r: &mut impl BufRead) -> Result<Option<String>> {
    let mut head = Vec::new();
    if r.read_until(b'\n', &mut head)? == 0 {
        bail!("unexpected end of cat-file output");
    }
    head.pop();
    if head.ends_with(b" missing") || head.ends_with(b" ambiguous") {
        return Ok(None);
    }
    let head = String::from_utf8_lossy(&head);
    let mut f = head.rsplitn(3, ' ');
    let size: usize = f
        .next()
        .and_then(|s| s.parse().ok())
        .with_context(|| format!("bad cat-file header {head:?}"))?;
    let kind = f
        .next()
        .with_context(|| format!("bad cat-file header {head:?}"))?;
    let mut body = vec![0u8; size + 1]; // content + trailing newline
    r.read_exact(&mut body)
        .context("truncated cat-file content")?;
    body.pop();
    Ok(if kind == "blob" {
        String::from_utf8(body).ok()
    } else {
        None
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::patch;
    use tempfile::TempDir;

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .output()
            .expect("spawn git");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn init() -> TempDir {
        let d = tempfile::tempdir().expect("tempdir");
        git(d.path(), &["init", "-q"]);
        d
    }

    fn write(dir: &Path, name: &str, content: &[u8]) {
        let p = dir.join(name);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(p, content).expect("write");
    }

    fn commit(dir: &Path) -> String {
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-q", "-m", "c"]);
        git(dir, &["rev-parse", "HEAD"])
    }

    fn lines(prefix: &str, n: usize) -> String {
        (0..n).map(|i| format!("{prefix} line {i}\n")).collect()
    }

    /// base commit + head commit exercising A/M/D/R/C/T.
    fn fixture() -> (TempDir, String, String) {
        let d = init();
        let p = d.path();
        write(p, "m.txt", lines("m", 10).as_bytes());
        write(p, "d.txt", lines("d", 10).as_bytes());
        write(p, "r.txt", lines("r", 20).as_bytes());
        write(p, "src.txt", lines("src", 20).as_bytes());
        write(p, "t.txt", b"target\n");
        let base = commit(p);
        write(p, "new.txt", b"a\nb\n");
        write(p, "m.txt", lines("m2", 10).as_bytes());
        std::fs::remove_file(p.join("d.txt")).expect("rm");
        std::fs::rename(p.join("r.txt"), p.join("sub dir/é.txt").tap_mkdir()).expect("mv");
        write(
            p,
            "src.txt",
            format!("{}extra\n", lines("src", 20)).as_bytes(),
        );
        write(p, "copy.txt", lines("src", 20).as_bytes());
        std::fs::remove_file(p.join("t.txt")).expect("rm");
        std::os::unix::fs::symlink("m.txt", p.join("t.txt")).expect("symlink");
        let head = commit(p);
        (d, base, head)
    }

    trait TapMkdir {
        fn tap_mkdir(self) -> Self;
    }
    impl TapMkdir for PathBuf {
        fn tap_mkdir(self) -> Self {
            std::fs::create_dir_all(self.parent().expect("parent")).expect("mkdir");
            self
        }
    }

    #[test]
    fn discover_finds_root_and_rejects_non_repo() {
        let d = init();
        write(d.path(), "sub/x", b"x");
        let repo = Repo::discover(&d.path().join("sub")).expect("discover");
        assert_eq!(
            repo.root().canonicalize().expect("canon"),
            d.path().canonicalize().expect("canon")
        );
        let plain = tempfile::tempdir().expect("tempdir");
        let err = Repo::discover(plain.path()).expect_err("not a repo");
        assert!(err.to_string().contains("not a git repository"), "{err}");
    }

    #[test]
    fn rev_parse_and_errors() {
        let d = init();
        write(d.path(), "a", b"a");
        let sha = commit(d.path());
        let repo = Repo::discover(d.path()).expect("discover");
        assert_eq!(repo.rev_parse("HEAD").expect("rev"), sha);
        assert_eq!(repo.rev_parse(&sha[..8]).expect("short"), sha);
        let err = repo.rev_parse("nope").expect_err("bad rev");
        assert!(
            err.to_string().contains("fatal") || err.to_string().contains("Needed"),
            "{err}"
        );
        assert!(repo.rev_parse("--help").is_err());
        assert!(repo.rev_parse("").is_err());
        assert!(repo.rev_parse("a\nb").is_err());
        assert!(repo.merge_base("-x", "HEAD").is_err());
        assert!(repo.diff_patch("HEAD", "--output=x", 50).is_err());
        assert!(repo.diff_files("-x", "HEAD", 50).is_err());
        assert!(repo.ls_tree("-x").is_err());
        assert!(repo.read_blobs("-x", &["a".into()]).is_err());
    }

    #[test]
    fn merge_base_of_diverged_branches() {
        let d = init();
        write(d.path(), "a", b"a");
        let root = commit(d.path());
        git(d.path(), &["checkout", "-q", "-b", "feat"]);
        write(d.path(), "b", b"b");
        let feat = commit(d.path());
        git(d.path(), &["checkout", "-q", "-"]);
        write(d.path(), "c", b"c");
        let main = commit(d.path());
        let repo = Repo::discover(d.path()).expect("discover");
        assert_eq!(repo.merge_base(&main, &feat).expect("mb"), root);
        let err = repo.merge_base(&main, "nope").expect_err("bad");
        assert!(err.to_string().contains("nope"), "{err}");
    }

    #[test]
    fn diff_files_classifies_every_status() {
        let (d, base, head) = fixture();
        let repo = Repo::discover(d.path()).expect("discover");
        let files = repo.diff_files(&base, &head, 50).expect("files");
        let by = |p: &str| {
            files
                .iter()
                .find(|f| f.path() == p)
                .unwrap_or_else(|| panic!("{p} in {files:?}"))
        };
        assert_eq!(by("new.txt").status, FileStatus::Added);
        assert_eq!(by("new.txt").old_path, None);
        assert_eq!(by("m.txt").status, FileStatus::Modified);
        assert_eq!(by("d.txt").status, FileStatus::Deleted);
        assert_eq!(by("d.txt").new_path, None);
        assert_eq!(by("t.txt").status, FileStatus::Modified); // type change
        let r = by("sub dir/é.txt");
        assert_eq!(r.status, FileStatus::Renamed { similarity: 100 });
        assert_eq!(r.old_path.as_deref(), Some("r.txt"));
        let c = by("copy.txt");
        assert_eq!(c.status, FileStatus::Copied { similarity: 100 });
        assert_eq!(c.old_path.as_deref(), Some("src.txt"));
        assert!(files.iter().all(|f| f.lang.is_none() && !f.generated));
    }

    #[test]
    fn diff_patch_is_deterministic_under_hostile_user_config() {
        let (d, base, head) = fixture();
        git(d.path(), &["config", "diff.noprefix", "true"]);
        git(d.path(), &["config", "core.quotepath", "true"]);
        git(d.path(), &["config", "diff.mnemonicPrefix", "true"]);
        git(d.path(), &["config", "diff.algorithm", "histogram"]);
        let repo = Repo::discover(d.path()).expect("discover");
        let text = repo.diff_patch(&base, &head, 50).expect("patch");
        assert!(text.contains("diff --git a/new.txt b/new.txt"), "{text}");
        assert!(text.contains("+++ b/new.txt"));
        assert!(text.contains("rename to sub dir/é.txt"), "{text}");
        let files = patch::parse(&text).expect("parse real patch");
        let paths: Vec<_> = files.iter().filter_map(|f| f.new_path.as_deref()).collect();
        assert!(
            paths.contains(&"sub dir/é.txt") && paths.contains(&"m.txt"),
            "{paths:?}"
        );
        let m = files
            .iter()
            .find(|f| f.new_path.as_deref() == Some("m.txt"))
            .expect("m");
        assert_eq!(m.hunks.len(), 1);
        let files2 = repo.diff_files(&base, &head, 50).expect("files");
        assert_eq!(files2.len(), 7); // patch splits the type change into delete + add
    }

    #[test]
    fn diff_patch_reports_bad_rev_with_stderr() {
        let (d, base, _) = fixture();
        let repo = Repo::discover(d.path()).expect("discover");
        let err = repo.diff_patch(&base, "missingrev", 50).expect_err("bad");
        assert!(err.to_string().contains("missingrev"), "{err}");
    }

    #[test]
    fn ls_tree_lists_all_paths() {
        let (d, base, head) = fixture();
        let repo = Repo::discover(d.path()).expect("discover");
        let mut b = repo.ls_tree(&base).expect("base");
        b.sort();
        assert_eq!(b, ["d.txt", "m.txt", "r.txt", "src.txt", "t.txt"]);
        let h = repo.ls_tree(&head).expect("head");
        assert!(h.contains(&"sub dir/é.txt".to_string()));
        assert!(repo.ls_tree("nope").is_err());
    }

    #[test]
    fn read_blobs_handles_missing_binary_and_odd_paths() {
        let d = init();
        write(d.path(), "ok.txt", b"hello\nworld");
        write(d.path(), "bin", &[0xff, 0xfe, 0x00]);
        write(d.path(), "my file é.txt", b"");
        write(d.path(), "dir/inner", b"x");
        let sha = commit(d.path());
        let repo = Repo::discover(d.path()).expect("discover");
        let want: Vec<String> = [
            "ok.txt",
            "nope.txt",
            "bin",
            "my file é.txt",
            "dir",
            "bad\nname",
            "ok.txt",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let got = repo.read_blobs(&sha, &want).expect("blobs");
        let vals: Vec<Option<&str>> = got.iter().map(|(_, c)| c.as_deref()).collect();
        assert_eq!(
            vals,
            [
                Some("hello\nworld"),
                None,
                None,
                Some(""),
                None,
                None,
                Some("hello\nworld")
            ]
        );
        assert_eq!(got[1].0, "nope.txt");
        assert!(repo.read_blobs(&sha, &[]).expect("empty").is_empty());
    }

    #[test]
    fn read_blobs_bad_rev_yields_missing_not_hang() {
        let d = init();
        write(d.path(), "a", b"a");
        commit(d.path());
        let repo = Repo::discover(d.path()).expect("discover");
        let got = repo.read_blobs("nosuchrev", &["a".into()]).expect("blobs");
        assert_eq!(got, [("a".to_string(), None)]);
    }

    #[test]
    fn read_blobs_large_batch_does_not_deadlock() {
        let d = init();
        let big = "x".repeat(200_000);
        let mut paths = Vec::new();
        for i in 0..40 {
            write(d.path(), &format!("f{i}"), big.as_bytes());
            paths.push(format!("f{i}"));
        }
        let sha = commit(d.path());
        let repo = Repo::discover(d.path()).expect("discover");
        let got = repo.read_blobs(&sha, &paths).expect("blobs");
        assert!(got.iter().all(|(_, c)| c.as_deref() == Some(big.as_str())));
    }

    #[test]
    fn record_parser_rejects_garbage() {
        assert!(read_record(&mut &b""[..]).is_err());
        assert!(read_record(&mut &b"garbage\n"[..]).is_err());
        assert!(read_record(&mut &b"abc blob 10\nshort\n"[..]).is_err());
        assert_eq!(
            read_record(&mut &b"abc tree 2\nhi\n"[..]).expect("tree"),
            None
        );
    }

    #[test]
    fn raw_parser_rejects_garbage() {
        assert!(parse_raw("junk\0").is_err());
        assert!(parse_raw(":100644 100644 a b X\0p\0").is_err());
        assert!(parse_raw(":100644 100644 a b R\0o\0n\0").is_err());
        assert!(parse_raw(":100644 100644 a b M\0").is_err());
        assert!(parse_raw("").expect("empty").is_empty());
    }

    #[test]
    fn range_specs() {
        let r = |s| parse_range(s).expect("range");
        assert_eq!(r("a...b"), ("a".into(), "b".into()));
        assert_eq!(r("a..b"), ("a".into(), "b".into()));
        assert_eq!(r("main"), ("main".into(), "HEAD".into()));
        assert_eq!(r("main.."), ("main".into(), "HEAD".into()));
        assert_eq!(
            r("origin/main...feat/x"),
            ("origin/main".into(), "feat/x".into())
        );
        assert!(parse_range("").is_err());
        assert!(parse_range("..b").is_err());
        assert!(parse_range("-x..b").is_err());
        assert!(parse_range("a..-b").is_err());
    }
}
