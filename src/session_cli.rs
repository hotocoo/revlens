//! Shared plumbing for `revlens review` and `revlens export`: build the analysis with intent
//! and assumptions attached, and locate the session file.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::config::Config;
use crate::git::{Repo, parse_range};
use crate::live::session_id;
use crate::model::Analysis;
use crate::{analyze, intent};

/// Where the review comes from. `range` may be omitted when `pr` is given.
pub struct Source<'a> {
    pub repo: &'a Path,
    pub range: Option<&'a str>,
    pub intent: Option<&'a Path>,
    pub pr: Option<u32>,
    pub assumptions: Option<&'a Path>,
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PrInfo {
    pub body: String,
    pub base_ref_oid: String,
    pub head_ref_oid: String,
}

pub fn parse_pr_info(json: &str) -> Result<PrInfo> {
    serde_json::from_str(json).context("parsing `gh pr view` output")
}

/// An explicit range wins; otherwise the PR's base and head commits.
pub fn choose_range(range: Option<&str>, pr: Option<&PrInfo>) -> Result<(String, String)> {
    match (range, pr) {
        (Some(r), _) => parse_range(r),
        (None, Some(p)) => Ok((p.base_ref_oid.clone(), p.head_ref_oid.clone())),
        (None, None) => bail!("a range or --pr is required"),
    }
}

pub fn load(src: &Source) -> Result<(Repo, Analysis)> {
    let repo = Repo::discover(src.repo)?;
    let cfg = Config::load(repo.root())?;
    let pr = src.pr.map(|n| fetch_pr(&repo, n)).transpose()?;
    let (base, head) = choose_range(src.range, pr.as_ref())?;
    let mut analysis = analyze::run(&repo, &base, &head, &cfg)?;
    analysis.criteria = match (src.intent, &pr) {
        (Some(path), _) => {
            let doc = std::fs::read_to_string(path)
                .with_context(|| format!("reading intent file {}", path.display()))?;
            intent::parse_criteria(&doc, &cfg)
        }
        (None, Some(p)) => intent::intent_from_pr_body(&p.body, &cfg),
        (None, None) => Vec::new(),
    };
    let default_assumptions = repo.root().join(".revlens/assumptions.jsonl");
    analysis.assumptions =
        intent::load_assumptions(src.assumptions.unwrap_or(&default_assumptions))?;
    Ok((repo, analysis))
}

/// `.revlens/sessions/<session_id>.ydoc` under the repo root.
pub fn session_path(repo: &Repo, a: &Analysis) -> PathBuf {
    // Status and mappings are keyed by positional criterion ids, so a different criteria set
    // must not reuse another set's session.
    let head = if a.criteria.is_empty() {
        a.range.head.clone()
    } else {
        let texts = a.criteria.iter().map(|c| c.text.as_str());
        let joined = texts.collect::<Vec<_>>().join("\0");
        format!(
            "{}\0{:x}",
            a.range.head,
            crate::hash::fnv1a64(joined.as_bytes())
        )
    };
    let id = session_id(&a.range.merge_base, &head);
    repo.root()
        .join(".revlens/sessions")
        .join(format!("{id}.ydoc"))
}

fn fetch_pr(repo: &Repo, n: u32) -> Result<PrInfo> {
    let out = Command::new("gh")
        .current_dir(repo.root())
        .args([
            "pr",
            "view",
            &n.to_string(),
            "--json",
            "body,baseRefOid,headRefOid",
        ])
        .output()
        .context("running `gh` (is the GitHub CLI installed?)")?;
    if !out.status.success() {
        bail!(
            "gh pr view {n}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let info = parse_pr_info(&String::from_utf8_lossy(&out.stdout))?;
    if repo.rev_parse(&info.head_ref_oid).is_err() {
        git_fetch(repo, &format!("pull/{n}/head"))?;
    }
    if repo.rev_parse(&info.base_ref_oid).is_err() {
        git_fetch(repo, &info.base_ref_oid)?;
    }
    Ok(info)
}

fn git_fetch(repo: &Repo, refspec: &str) -> Result<()> {
    let out = Command::new("git")
        .current_dir(repo.root())
        .args(["fetch", "--quiet", "origin", refspec])
        .output()
        .context("running git fetch")?;
    if !out.status.success() {
        bail!(
            "git fetch origin {refspec}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

/// Best effort: a missing opener is not an error.
pub fn open_browser(url: &str) {
    let (prog, args): (&str, &[&str]) = if cfg!(target_os = "macos") {
        ("open", &[])
    } else if cfg!(target_os = "windows") {
        ("cmd", &["/c", "start", ""])
    } else {
        ("xdg-open", &[])
    };
    if let Err(e) = Command::new(prog).args(args).arg(url).spawn() {
        eprintln!("could not open a browser ({e}); open the URL above manually");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr() -> PrInfo {
        parse_pr_info(r#"{"body":"- a","baseRefOid":"b1","headRefOid":"h1"}"#).expect("json")
    }

    #[test]
    fn pr_json_parses() {
        assert_eq!(pr().head_ref_oid, "h1");
        assert!(parse_pr_info("{}").is_err());
    }

    #[test]
    fn range_precedence() {
        assert_eq!(
            choose_range(Some("a..b"), Some(&pr())).expect("r"),
            ("a".into(), "b".into())
        );
        assert_eq!(
            choose_range(None, Some(&pr())).expect("r"),
            ("b1".into(), "h1".into())
        );
        assert!(choose_range(None, None).is_err());
        assert!(choose_range(Some("-x..y"), None).is_err());
    }
}
