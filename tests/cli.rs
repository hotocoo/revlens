//! Runs the built `revlens` binary against throwaway repos.

mod common;

use std::collections::BTreeMap;
use std::path::Path;
use std::process::{Command, Output};

use anyhow::Result;
use common::{Fixture, fixture};
use revlens::analyze;
use revlens::config::Config;
use revlens::git::Repo;
use revlens::intent::parse_criteria;
use revlens::live::apply_state;
use revlens::model::{CritStatus, Mapping, SessionState};
use revlens::session_cli::session_path;
use yrs::{Doc, ReadTxn, StateVector, Transact};

const BASE: &str = "def pay(x):\n    return x\n";
const HEAD: &str = "def pay(x, fee):\n    return x + fee\n";
const INTENT: &str = "## Acceptance criteria\n- Payments include a fee\n- Refunds still work\n";

fn repo() -> Result<Fixture> {
    fixture(&[("pay.py", BASE)], &[("pay.py", HEAD)])
}

fn run(args: &[&str], dir: &Path) -> Result<Output> {
    Ok(Command::new(env!("CARGO_BIN_EXE_revlens"))
        .current_dir(dir)
        .args(args)
        .output()?)
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

fn range(f: &Fixture) -> String {
    format!("{}..{}", f.base, f.head)
}

#[test]
fn analyze_prints_ranked_hunks() -> Result<()> {
    let f = repo()?;
    let out = run(&["analyze", &range(&f)], f.dir.path())?;
    assert!(out.status.success());
    let s = text(&out.stdout);
    assert!(s.contains("1 hunks"), "{s}");
    assert!(s.contains("pay.py:1"), "{s}");
    Ok(())
}

#[test]
fn analyze_json_is_valid() -> Result<()> {
    let f = repo()?;
    let out = run(&["analyze", &range(&f), "--json"], f.dir.path())?;
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout)?;
    assert_eq!(v["hunks"].as_array().map(Vec::len), Some(1));
    Ok(())
}

fn write_session(f: &Fixture, intent: &str) -> Result<()> {
    let repo = Repo::discover(f.dir.path())?;
    let mut a = analyze::run(&repo, &f.base, &f.head, &Config::default())?;
    a.criteria = parse_criteria(intent, &Config::default());
    let hunk = a.hunks[0].id.clone();
    let state = SessionState {
        status: BTreeMap::from([("c1".to_string(), CritStatus::Met)]),
        mappings: vec![Mapping {
            criterion: "c1".into(),
            hunk: hunk.clone(),
            by: "ann".into(),
        }],
        reviewed: BTreeMap::from([(hunk, "ann".to_string())]),
        claims: BTreeMap::new(),
    };
    let doc = Doc::new();
    apply_state(&doc, &state);
    let bytes = doc
        .transact()
        .encode_state_as_update_v1(&StateVector::default());
    let path = session_path(&repo, &a);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, bytes)?;
    Ok(())
}

#[test]
fn export_reads_session_file() -> Result<()> {
    let f = repo()?;
    std::fs::write(f.dir.path().join("intent.md"), INTENT)?;
    write_session(&f, INTENT)?;
    let r = range(&f);
    let md = run(&["export", &r, "--intent", "intent.md"], f.dir.path())?;
    assert!(md.status.success(), "{}", text(&md.stderr));
    let s = text(&md.stdout);
    assert!(s.contains("Payments include a fee"), "{s}");
    assert!(s.contains("ann"), "{s}");

    let js = run(
        &["export", &r, "--intent", "intent.md", "--format", "json"],
        f.dir.path(),
    )?;
    let v: serde_json::Value = serde_json::from_slice(&js.stdout)?;
    assert_eq!(v["criteria"][0]["status"], "Met");
    assert_eq!(v["missing_work"], serde_json::json!(["c2"]));
    assert_eq!(v["scope_creep"], serde_json::json!([]));
    Ok(())
}

#[test]
fn changed_intent_does_not_reuse_old_session() -> Result<()> {
    let f = repo()?;
    write_session(&f, INTENT)?;
    let shifted = format!("{INTENT}- Brand new criterion\n").replace(
        "- Payments include fee",
        "- Brand new first\n- Payments include fee",
    );
    std::fs::write(f.dir.path().join("i2.md"), shifted)?;
    let out = run(
        &[
            "export",
            &range(&f),
            "--intent",
            "i2.md",
            "--format",
            "json",
        ],
        f.dir.path(),
    )?;
    assert!(
        text(&out.stderr).contains("empty state"),
        "{}",
        text(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout)?;
    assert_eq!(v["criteria"][0]["status"], "Open");
    Ok(())
}

#[test]
fn export_warns_about_orphaned_mappings() -> Result<()> {
    let f = repo()?;
    write_session(&f, "")?;
    let out = run(&["export", &range(&f)], f.dir.path())?;
    assert!(out.status.success());
    assert!(
        text(&out.stderr).contains("1 mappings refer to"),
        "{}",
        text(&out.stderr)
    );
    Ok(())
}

#[test]
fn export_without_session_says_so_and_writes_file() -> Result<()> {
    let f = repo()?;
    std::fs::write(f.dir.path().join("intent.md"), INTENT)?;
    let out = run(
        &[
            "export",
            &range(&f),
            "--intent",
            "intent.md",
            "--out",
            "rec.md",
        ],
        f.dir.path(),
    )?;
    assert!(out.status.success());
    assert!(text(&out.stderr).contains("empty state"));
    assert!(out.stdout.is_empty());
    assert!(std::fs::read_to_string(f.dir.path().join("rec.md"))?.contains("Refunds still work"));
    Ok(())
}

#[test]
fn bad_inputs_fail_with_codes() -> Result<()> {
    let f = repo()?;
    let d = f.dir.path();
    let bad = run(&["analyze", "nope..main"], d)?;
    assert_eq!(bad.status.code(), Some(1));
    assert!(text(&bad.stderr).contains("error:"));

    let missing = run(&["export", &range(&f), "--intent", "absent.md"], d)?;
    assert_eq!(missing.status.code(), Some(1));
    assert!(text(&missing.stderr).contains("absent.md"));

    let review = run(
        &["review", &range(&f), "--intent", "absent.md", "--no-open"],
        d,
    )?;
    assert_eq!(review.status.code(), Some(1));

    let usage = run(&["review"], d)?;
    assert_eq!(usage.status.code(), Some(2));
    Ok(())
}

const DUP_A: &str = "def parse_rows(rows):\n    out = []\n    for row in rows:\n        if row.get('ok'):\n            out.append(row['value'] * 2)\n        else:\n            out.append(0)\n    return out\n";
const DUP_B: &str = "def read_items(items):\n    res = []\n    for it in items:\n        if it.get('good'):\n            res.append(it['n'] * 3)\n        else:\n            res.append(1)\n    return res\n";

#[test]
fn no_reuse_flag_turns_off_duplicate_detection() -> Result<()> {
    let f = fixture(&[("a.py", DUP_A)], &[("a.py", DUP_A), ("b.py", DUP_B)])?;
    let with = run(&["analyze", &range(&f)], f.dir.path())?;
    assert!(with.status.success());
    assert!(
        text(&with.stdout).contains("new 'read_items' duplicates existing 'parse_rows' at a.py:1"),
        "{}",
        text(&with.stdout)
    );
    let without = run(&["analyze", &range(&f), "--no-reuse"], f.dir.path())?;
    assert!(without.status.success());
    assert!(!text(&without.stdout).contains("duplicates"));
    let export = run(&["export", &range(&f), "--no-reuse"], f.dir.path())?;
    assert!(export.status.success(), "{}", text(&export.stderr));
    Ok(())
}

#[test]
fn fail_on_conditions_exit_with_status_two() -> Result<()> {
    let f = repo()?;
    std::fs::write(f.dir.path().join("intent.md"), INTENT)?;
    let r = range(&f);
    let base = ["export", r.as_str(), "--intent", "intent.md"];
    let pass = run(
        &[&base[..], &["--fail-on", "reuse-miss"]].concat(),
        f.dir.path(),
    )?;
    assert_eq!(pass.status.code(), Some(0), "{}", text(&pass.stderr));
    let fail = run(
        &[&base[..], &["--fail-on", "missing-work,unreviewed"]].concat(),
        f.dir.path(),
    )?;
    assert_eq!(fail.status.code(), Some(2));
    let err = text(&fail.stderr);
    assert!(err.contains("fail: missing work: c1, c2"), "{err}");
    assert!(err.contains("fail: 1 hunks unreviewed"), "{err}");
    assert!(text(&fail.stdout).contains("# Lens conformance record"));
    let score = run(
        &[&base[..], &["--fail-on-score", "1"]].concat(),
        f.dir.path(),
    )?;
    assert_eq!(score.status.code(), Some(2));
    let post = run(&[&base[..], &["--post"]].concat(), f.dir.path())?;
    assert_eq!(post.status.code(), Some(1));
    assert!(text(&post.stderr).contains("--post needs --pr"));
    Ok(())
}

/// A stand-in `gh` that answers `pr view`, lists `$GH_EXISTING` as the old comment id, and logs
/// every call (args, then stdin) to `$GH_LOG`.
#[cfg(unix)]
fn fake_gh(dir: &Path) -> Result<std::path::PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let bin = dir.join("fakebin");
    std::fs::create_dir_all(&bin)?;
    let script = r###"#!/bin/sh
echo "ARGS $*" >> "$GH_LOG"
case "$1 $2" in
  "pr view") printf '{"body":"## Acceptance criteria\\n- Payments include a fee\\n","baseRefOid":"%s","headRefOid":"%s"}' "$GH_BASE" "$GH_HEAD" ;;
  "api --paginate") [ -n "$GH_EXISTING" ] && echo "$GH_EXISTING" ;;
  *) cat >> "$GH_LOG" ;;
esac
exit 0
"###;
    let gh = bin.join("gh");
    std::fs::write(&gh, script)?;
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755))?;
    Ok(bin)
}

#[cfg(unix)]
fn run_with_gh(f: &Fixture, existing: &str) -> Result<(Output, String)> {
    let bin = fake_gh(f.dir.path())?;
    let log = f.dir.path().join("gh.log");
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let out = Command::new(env!("CARGO_BIN_EXE_revlens"))
        .current_dir(f.dir.path())
        .env("PATH", path)
        .env("GH_LOG", &log)
        .env("GH_BASE", &f.base)
        .env("GH_HEAD", &f.head)
        .env("GH_EXISTING", existing)
        .args(["export", "--pr", "7", "--post", "--fail-on", "missing-work"])
        .output()?;
    Ok((out, std::fs::read_to_string(log)?))
}

#[cfg(unix)]
#[test]
fn post_creates_a_comment_then_edits_it() -> Result<()> {
    let f = repo()?;
    let (out, log) = run_with_gh(&f, "")?;
    assert_eq!(out.status.code(), Some(2), "{}", text(&out.stderr));
    assert!(log.contains("ARGS pr comment 7 --body-file -"), "{log}");
    assert!(
        log.contains("<!-- revlens:conformance -->\n# Lens conformance record"),
        "{log}"
    );
    assert!(log.contains("Payments include a fee"), "{log}");

    let (out, log) = run_with_gh(&f, "991")?;
    assert_eq!(out.status.code(), Some(2), "{}", text(&out.stderr));
    assert!(
        log.contains("ARGS api -X PATCH repos/{owner}/{repo}/issues/comments/991 --input -"),
        "{log}"
    );
    assert!(
        log.contains(r##"{"body":"<!-- revlens:conformance -->\n# Lens"##),
        "{log}"
    );
    Ok(())
}

#[test]
fn analysis_is_cached_per_commits_and_config() -> Result<()> {
    let f = repo()?;
    let d = f.dir.path();
    let cache = d.join(".revlens/cache");
    let first = run(&["analyze", &range(&f), "--json"], d)?;
    assert!(first.status.success());
    let entries = || std::fs::read_dir(&cache).map(|r| r.count()).unwrap_or(0);
    assert_eq!(entries(), 1);
    let again = run(&["analyze", &range(&f), "--json"], d)?;
    assert_eq!(
        again.stdout, first.stdout,
        "cache hit returns the same analysis"
    );
    assert_eq!(entries(), 1);
    run(&["analyze", &range(&f), "--no-reuse"], d)?;
    assert_eq!(entries(), 2, "a different config is a different entry");
    std::fs::remove_dir_all(&cache)?;
    run(&["analyze", &range(&f), "--no-cache"], d)?;
    assert_eq!(entries(), 0);
    Ok(())
}

#[cfg(unix)]
#[test]
fn llm_intent_uses_the_configured_command_once() -> Result<()> {
    let f = repo()?;
    let d = f.dir.path();
    std::fs::write(
        d.join("ticket.md"),
        "# Ticket\n\nPayments must include a fee.\n",
    )?;
    let calls = d.join("calls");
    std::fs::create_dir_all(d.join(".revlens"))?;
    std::fs::write(
        d.join(".revlens/config.toml"),
        format!(
            "[intent]\nllm_command = [\"sh\", \"-c\", \"cat >/dev/null; echo x >> {}; echo '[\\\"Payments include a fee\\\"]'\"]\n",
            calls.display()
        ),
    )?;
    let args = [
        "export",
        range(&f).leak(),
        "--intent",
        "ticket.md",
        "--llm-intent",
        "--format",
        "json",
    ];
    let out = run(&args, d)?;
    assert!(out.status.success(), "{}", text(&out.stderr));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout)?;
    assert_eq!(
        v["criteria"][0]["criterion"]["text"],
        "Payments include a fee"
    );
    assert_eq!(v["criteria"][0]["criterion"]["source_line"], 3);
    let again = run(&args, d)?;
    assert_eq!(again.stdout, out.stdout);
    assert_eq!(std::fs::read_to_string(&calls)?.lines().count(), 1);

    let none = run(&["export", &range(&f), "--llm-intent"], d)?;
    assert_eq!(none.status.code(), Some(1));
    assert!(text(&none.stderr).contains("--llm-intent needs --intent or --pr"));
    Ok(())
}
