#![allow(dead_code)]
//! Fixture helpers: build a throwaway git repo with a base and a head commit.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use revlens::analyze;
use revlens::config::Config;
use revlens::git::Repo;
use revlens::model::{Analysis, Hunk, Rule};
use serde_json::{Value, json};
use tempfile::TempDir;

pub struct Fixture {
    pub dir: TempDir,
    pub base: String,
    pub head: String,
}

/// Run git in `dir` with a fixed identity; returns trimmed stdout.
pub fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .current_dir(dir)
        .args([
            "-c",
            "user.name=Lens Test",
            "-c",
            "user.email=lens@example.invalid",
        ])
        .args(["-c", "commit.gpgsign=false", "-c", "core.autocrlf=false"])
        .args(args)
        .output()
        .with_context(|| format!("running git {args:?}"))?;
    if !out.status.success() {
        bail!("git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn write_tree(dir: &Path, files: &[(&str, &str)]) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.file_name().is_some_and(|n| n == ".git") {
            continue;
        }
        if path.is_dir() {
            std::fs::remove_dir_all(&path)?;
        } else {
            std::fs::remove_file(&path)?;
        }
    }
    for (rel, text) in files {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, text)?;
    }
    Ok(())
}

/// A repo whose first commit holds `base` and whose second holds exactly `head`.
pub fn fixture(base: &[(&str, &str)], head: &[(&str, &str)]) -> Result<Fixture> {
    let dir = tempfile::tempdir()?;
    git(dir.path(), &["init", "-q", "-b", "main"])?;
    let mut shas = Vec::new();
    for (i, files) in [base, head].into_iter().enumerate() {
        write_tree(dir.path(), files)?;
        git(dir.path(), &["add", "-A"])?;
        git(
            dir.path(),
            &["commit", "-q", "--allow-empty", "-m", &format!("c{i}")],
        )?;
        shas.push(git(dir.path(), &["rev-parse", "HEAD"])?);
    }
    let head = shas.pop().context("head sha")?;
    let base = shas.pop().context("base sha")?;
    Ok(Fixture { dir, base, head })
}

pub fn analyze(fx: &Fixture) -> Result<Analysis> {
    analyze_with(fx, &Config::default())
}

pub fn analyze_with(fx: &Fixture, cfg: &Config) -> Result<Analysis> {
    let repo = Repo::discover(fx.dir.path())?;
    analyze::run(&repo, &fx.base, &fx.head, cfg)
}

/// Shortcut: build the fixture and analyze it.
pub fn run(base: &[(&str, &str)], head: &[(&str, &str)]) -> Analysis {
    let fx = fixture(base, head).expect("fixture");
    analyze(&fx).expect("analyze")
}

pub fn has(h: &Hunk, rule: &Rule) -> bool {
    h.signals.iter().any(|s| s.rule == *rule)
}

pub fn in_file<'a>(a: &'a Analysis, path: &str) -> Vec<&'a Hunk> {
    a.hunks.iter().filter(|h| h.path == path).collect()
}

/// Stable projection (no shas) for snapshots.
pub fn summary(a: &Analysis) -> Value {
    let hunks: Vec<Value> = a
        .hunks
        .iter()
        .map(|h| {
            json!({
                "path": h.path,
                "new_start": h.new_start,
                "kind": h.kind,
                "score": h.score,
                "signals": h.signals.iter().map(|s| json!([s.rule, s.points, s.reason])).collect::<Vec<_>>(),
            })
        })
        .collect();
    let symbols: Vec<Value> = a
        .symbols
        .iter()
        .map(|s| json!({"name": s.name, "path": s.path, "change": s.change, "public": s.public}))
        .collect();
    json!({"hunks": hunks, "symbols": symbols})
}
