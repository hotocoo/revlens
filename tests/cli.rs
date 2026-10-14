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
