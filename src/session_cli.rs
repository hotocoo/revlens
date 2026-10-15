//! Shared plumbing for `revlens review` and `revlens export`: build the analysis with intent
//! and assumptions attached, and locate the session file.

use std::net::{IpAddr, SocketAddr, ToSocketAddrs, UdpSocket};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::config::Config;
use crate::git::{Repo, parse_range};
use crate::live::session_id;
use crate::model::Analysis;
use crate::{analyze, intent, llm_intent};

/// Where the review comes from. `range` may be omitted when `pr` is given.
pub struct Source<'a> {
    pub repo: &'a Path,
    pub range: Option<&'a str>,
    pub intent: Option<&'a Path>,
    pub pr: Option<u32>,
    pub assumptions: Option<&'a Path>,
    /// Skip the duplicate-code scan (`--no-reuse`).
    pub no_reuse: bool,
    /// Recompute instead of reading the analysis cache (`--no-cache`).
    pub no_cache: bool,
    /// Extract criteria with `intent.llm_command` instead of the list parser (`--llm-intent`).
    pub llm_intent: bool,
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
    let mut cfg = Config::load(repo.root())?;
    cfg.limits.reuse_enabled &= !src.no_reuse;
    cfg.limits.cache_enabled &= !src.no_cache;
    let pr = src.pr.map(|n| fetch_pr(&repo, n)).transpose()?;
    let (base, head) = choose_range(src.range, pr.as_ref())?;
    let mut analysis = analyze::run(&repo, &base, &head, &cfg)?;
    analysis.criteria = match (src.intent, &pr) {
        (Some(path), _) => {
            let doc = std::fs::read_to_string(path)
                .with_context(|| format!("reading intent file {}", path.display()))?;
            if src.llm_intent {
                llm_intent::criteria(&doc, &cfg.intent, repo.root())?
            } else {
                intent::parse_criteria(&doc, &cfg)
            }
        }
        (None, Some(p)) if src.llm_intent => {
            llm_intent::criteria(&p.body, &cfg.intent, repo.root())?
        }
        (None, Some(p)) => {
            let c = intent::intent_from_pr_body(&p.body, &cfg);
            if c.is_empty() {
                eprintln!(
                    "note: PR body has no acceptance-criteria section (headings: {}); pass --intent <FILE> to review against criteria",
                    cfg.intent.headings.join(", ")
                );
            }
            c
        }
        (None, None) if src.llm_intent => bail!("--llm-intent needs --intent or --pr"),
        (None, None) => Vec::new(),
    };
    let default_assumptions = repo.root().join(".revlens/assumptions.jsonl");
    analysis.assumptions =
        intent::load_assumptions(src.assumptions.unwrap_or(&default_assumptions))?;
    Ok((repo, analysis))
}

/// The session file for this review, re-anchoring state from an earlier review of the same
/// source (after a force-push or rebase) when the file does not exist yet.
pub fn prepare_session(repo: &Repo, a: &Analysis, src: &Source) -> Result<PathBuf> {
    let path = session_path(repo, a);
    let source = crate::reanchor::source_key(src.range, src.pr, a);
    if let Some(r) = crate::reanchor::prepare(&path, &source, a)? {
        eprintln!(
            "re-anchored review from {}: {} of {} reviewed hunks found again; the rest changed since",
            r.from.display(),
            r.carried,
            r.had_state
        );
    }
    Ok(path)
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

/// Hidden first line of a posted record; reruns edit the comment carrying it instead of stacking.
pub const COMMENT_MARKER: &str = "<!-- revlens:conformance -->";

/// Post `body` on PR `n`, replacing the previous Lens comment if there is one.
pub fn post_pr_comment(repo: &Repo, n: u32, body: &str) -> Result<()> {
    let body = format!("{COMMENT_MARKER}\n{body}");
    let jq = format!(
        ".[] | select(.body | startswith({})) | .id",
        serde_json::to_string(COMMENT_MARKER).context("quoting marker")?
    );
    let list = gh(
        repo,
        &[
            "api",
            "--paginate",
            &format!("repos/{{owner}}/{{repo}}/issues/{n}/comments"),
            "--jq",
            &jq,
        ],
        None,
    )?;
    match list.lines().rev().find(|l| !l.trim().is_empty()) {
        Some(id) => {
            let payload = serde_json::json!({ "body": body }).to_string();
            gh(
                repo,
                &[
                    "api",
                    "-X",
                    "PATCH",
                    &format!("repos/{{owner}}/{{repo}}/issues/comments/{}", id.trim()),
                    "--input",
                    "-",
                ],
                Some(&payload),
            )?;
        }
        None => {
            gh(
                repo,
                &["pr", "comment", &n.to_string(), "--body-file", "-"],
                Some(&body),
            )?;
        }
    }
    Ok(())
}

/// Run `gh` in the repo, optionally feeding stdin; returns stdout.
fn gh(repo: &Repo, args: &[&str], stdin: Option<&str>) -> Result<String> {
    use std::io::Write;
    use std::process::Stdio;
    let mut child = Command::new("gh")
        .current_dir(repo.root())
        .args(args)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("running `gh` (is the GitHub CLI installed?)")?;
    if let (Some(text), Some(mut pipe)) = (stdin, child.stdin.take()) {
        pipe.write_all(text.as_bytes())
            .context("writing to `gh` stdin")?;
    }
    let out = child.wait_with_output().context("waiting for `gh`")?;
    if !out.status.success() {
        bail!(
            "gh {}: {}",
            args.first().copied().unwrap_or_default(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
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

/// Local address of the interface that routes to `probe`. A connected UDP socket only selects a
/// route; nothing is sent, so `probe` need not be reachable (or even exist).
pub fn local_addr_toward(probe: &str) -> Result<IpAddr> {
    let target = probe
        .to_socket_addrs()
        .with_context(|| format!("server.lan_probe '{probe}' is not a socket address"))?
        .next()
        .with_context(|| format!("server.lan_probe '{probe}' resolves to nothing"))?;
    let any: SocketAddr = if target.is_ipv4() {
        ([0, 0, 0, 0], 0).into()
    } else {
        ([0u16; 8], 0).into()
    };
    let sock = UdpSocket::bind(any).context("opening a UDP socket")?;
    sock.connect(target)
        .with_context(|| format!("no route to {probe}"))?;
    Ok(sock.local_addr().context("reading local address")?.ip())
}

/// Non-loopback addresses teammates could use, one per probe that yields one.
pub fn lan_addrs(probes: &[&str]) -> Vec<IpAddr> {
    let mut out: Vec<IpAddr> = Vec::new();
    for p in probes {
        match local_addr_toward(p) {
            Ok(ip) if !ip.is_loopback() && !ip.is_unspecified() && !out.contains(&ip) => {
                out.push(ip)
            }
            Ok(_) => {}
            Err(e) => eprintln!("warning: {e:#}"),
        }
    }
    out
}

/// `local_url` (host 127.0.0.1) re-addressed to each of `addrs`.
pub fn share_urls(local_url: &str, addrs: &[IpAddr]) -> Vec<String> {
    addrs
        .iter()
        .map(|ip| {
            let host = match ip {
                IpAddr::V4(v) => v.to_string(),
                IpAddr::V6(v) => format!("[{v}]"),
            };
            local_url.replacen("127.0.0.1", &host, 1)
        })
        .collect()
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
    fn local_addr_toward_loopback_is_loopback_and_sends_nothing() {
        let ip = local_addr_toward("127.0.0.1:9").expect("route");
        assert!(ip.is_loopback());
        assert!(local_addr_toward("not an address").is_err());
    }

    #[test]
    fn lan_addrs_drops_loopback_and_bad_probes() {
        assert!(lan_addrs(&["127.0.0.1:9", "nonsense"]).is_empty());
    }

    #[test]
    fn share_urls_readdress_every_candidate() {
        let addrs: Vec<IpAddr> = vec![
            "192.168.1.7".parse().expect("ip"),
            "fd00::7".parse().expect("ip"),
        ];
        assert_eq!(
            share_urls("http://127.0.0.1:4100/?t=abc", &addrs),
            [
                "http://192.168.1.7:4100/?t=abc",
                "http://[fd00::7]:4100/?t=abc"
            ]
        );
        assert!(share_urls("http://127.0.0.1:1/", &[]).is_empty());
    }

    fn git(dir: &std::path::Path, args: &[&str]) {
        let st = Command::new("git")
            .current_dir(dir)
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@example.invalid",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .status()
            .expect("git");
        assert!(st.success(), "git {args:?}");
    }

    fn init_repo(dir: &std::path::Path) -> Repo {
        git(dir, &["init", "-q"]);
        std::fs::write(dir.join("a.txt"), "x\n").expect("write");
        git(dir, &["add", "."]);
        git(dir, &["commit", "-qm", "c"]);
        Repo::discover(dir).expect("repo")
    }

    #[test]
    fn git_fetch_reports_missing_origin_and_succeeds_with_one() {
        let origin = tempfile::tempdir().expect("tmp");
        init_repo(origin.path());
        let work = tempfile::tempdir().expect("tmp");
        let repo = init_repo(work.path());
        let err = git_fetch(&repo, "HEAD").expect_err("no origin");
        assert!(format!("{err:#}").contains("fetch"), "{err:#}");
        git(
            work.path(),
            &[
                "remote",
                "add",
                "origin",
                &origin.path().display().to_string(),
            ],
        );
        git_fetch(&repo, "HEAD").expect("fetch");
    }

    #[test]
    fn session_path_is_keyed_by_criteria_set() {
        let dir = tempfile::tempdir().expect("tmp");
        let repo = init_repo(dir.path());
        let mut a = Analysis {
            range: crate::model::Range {
                base: "b".into(),
                head: "h".into(),
                merge_base: "m".into(),
            },
            files: vec![],
            hunks: vec![],
            symbols: vec![],
            criteria: vec![],
            assumptions: vec![],
        };
        let bare = session_path(&repo, &a);
        assert!(bare.starts_with(repo.root().join(".revlens/sessions")));
        assert_eq!(bare.extension().and_then(|e| e.to_str()), Some("ydoc"));
        a.criteria = intent::parse_criteria("- one\n- two\n", &Config::default());
        let with = session_path(&repo, &a);
        assert_ne!(bare, with);
        a.criteria = intent::parse_criteria("- one\n- three\n", &Config::default());
        assert_ne!(with, session_path(&repo, &a));
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
