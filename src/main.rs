#![deny(clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use revlens::analyze;
use revlens::config::Config;
use revlens::git::{Repo, parse_range};
use revlens::model::{Analysis, Hunk, HunkKind};
use revlens::session_cli::{self, Source};
use revlens::{conformance, live, server, time};

#[derive(Parser)]
#[command(
    name = "revlens",
    version,
    about = "Intent-aware, risk-ranked code review"
)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Rank the hunks of a change by review risk.
    Analyze {
        /// `base..head`, `base...head`, or a single ref compared against HEAD.
        range: String,
        /// Repository path (default: current directory).
        #[arg(long, default_value = ".")]
        repo: PathBuf,
        /// Print the full analysis as JSON.
        #[arg(long)]
        json: bool,
        /// Skip the scan for functions that duplicate existing code.
        #[arg(long)]
        no_reuse: bool,
    },
    /// Serve the live review UI for a change.
    Review {
        /// `base..head`; optional with --pr.
        #[arg(required_unless_present = "pr")]
        range: Option<String>,
        #[arg(long, default_value = ".")]
        repo: PathBuf,
        /// Intent document whose list items are the acceptance criteria.
        #[arg(long, conflicts_with = "pr")]
        intent: Option<PathBuf>,
        /// Take the range and criteria from this GitHub PR (needs `gh`).
        #[arg(long)]
        pr: Option<u32>,
        /// Agent assumptions JSONL (default: .revlens/assumptions.jsonl).
        #[arg(long)]
        assumptions: Option<PathBuf>,
        /// Skip the scan for functions that duplicate existing code.
        #[arg(long)]
        no_reuse: bool,
        /// Listen on all interfaces so teammates can join.
        #[arg(long)]
        share: bool,
        #[arg(long)]
        port: Option<u16>,
        /// Do not open a browser.
        #[arg(long)]
        no_open: bool,
    },
    /// Print the conformance record of a review session.
    Export {
        /// `base..head`; optional with --pr.
        #[arg(required_unless_present = "pr")]
        range: Option<String>,
        #[arg(long, default_value = ".")]
        repo: PathBuf,
        #[arg(long, conflicts_with = "pr")]
        intent: Option<PathBuf>,
        #[arg(long)]
        pr: Option<u32>,
        #[arg(long)]
        assumptions: Option<PathBuf>,
        /// Skip the scan for functions that duplicate existing code.
        #[arg(long)]
        no_reuse: bool,
        #[arg(long, value_enum, default_value = "md")]
        format: Format,
        /// Write to this file instead of stdout.
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum Format {
    Md,
    Json,
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Cmd::Analyze {
            range,
            repo,
            json,
            no_reuse,
        } => {
            let repo = Repo::discover(&repo)?;
            let mut cfg = Config::load(repo.root())?;
            cfg.limits.reuse_enabled &= !no_reuse;
            let (base, head) = parse_range(&range)?;
            let analysis = analyze::run(&repo, &base, &head, &cfg)?;
            if json {
                let out =
                    serde_json::to_string_pretty(&analysis).context("serializing analysis")?;
                println!("{out}");
            } else {
                print!("{}", render(&range, &analysis));
            }
            Ok(())
        }
        Cmd::Review {
            range,
            repo,
            intent,
            pr,
            assumptions,
            no_reuse,
            share,
            port,
            no_open,
        } => {
            let src = Source {
                repo: &repo,
                range: range.as_deref(),
                intent: intent.as_deref(),
                pr,
                assumptions: assumptions.as_deref(),
                no_reuse,
            };
            review(&src, share, port, no_open)
        }
        Cmd::Export {
            range,
            repo,
            intent,
            pr,
            assumptions,
            no_reuse,
            format,
            out,
        } => {
            let src = Source {
                repo: &repo,
                range: range.as_deref(),
                intent: intent.as_deref(),
                pr,
                assumptions: assumptions.as_deref(),
                no_reuse,
            };
            export(&src, format, out.as_deref())
        }
    }
}

fn review(src: &Source, share: bool, port: Option<u16>, no_open: bool) -> Result<()> {
    let (repo, analysis) = session_cli::load(src)?;
    let cfg = Config::load(repo.root())?;
    let path = session_cli::session_path(&repo, &analysis);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    println!("Session file: {}", path.display());
    println!("Hint: add .revlens/sessions/ to .gitignore.");
    let rt = tokio::runtime::Runtime::new().context("starting async runtime")?;
    rt.block_on(server::serve(analysis, path, &cfg, share, port, |url| {
        if share {
            print_share_urls(url, &cfg.server.lan_probe);
        }
        if !no_open {
            session_cli::open_browser(url);
        }
    }))
}

/// Teammate-reachable URLs for a server listening on all interfaces.
fn print_share_urls(local_url: &str, probe: &str) {
    let addrs = session_cli::lan_addrs(&[probe]);
    if addrs.is_empty() {
        println!(
            "No LAN address found (offline?); use this machine's address with the port above."
        );
        return;
    }
    println!("Share with teammates on your network:");
    for url in session_cli::share_urls(local_url, &addrs) {
        println!("  {url}");
    }
}

fn export(src: &Source, format: Format, out: Option<&Path>) -> Result<()> {
    let (repo, analysis) = session_cli::load(src)?;
    let path = session_cli::session_path(&repo, &analysis);
    if !path.exists() {
        eprintln!(
            "no session file at {}; exporting with empty state",
            path.display()
        );
    }
    let state = live::Session::open(&path)?.snapshot()?;
    let orphans = conformance::orphaned(&analysis, &state);
    if orphans > 0 {
        eprintln!(
            "warning: {orphans} mappings refer to criteria or hunks not in this analysis (wrong --intent?)"
        );
    }
    let rec = conformance::build(&analysis, &state, time::now_rfc3339());
    let text = match format {
        Format::Md => conformance::render_markdown(&rec),
        Format::Json => conformance::render_json(&rec)?,
    };
    match out {
        Some(p) => std::fs::write(p, text).with_context(|| format!("writing {}", p.display())),
        None => {
            print!("{text}");
            Ok(())
        }
    }
}

fn kind_name(h: &Hunk) -> &'static str {
    match h.kind {
        HunkKind::Changed => "changed",
        HunkKind::New => "new",
        HunkKind::Deleted => "deleted",
        HunkKind::Moved { .. } => "moved",
        HunkKind::FormatOnly => "format",
    }
}

fn render(range: &str, a: &Analysis) -> String {
    let short = |s: &str| s.chars().take(10).collect::<String>();
    let mut out = format!(
        "revlens analyze {range} (merge-base {})\n{} hunks, {} symbol changes\n\n",
        short(&a.range.merge_base),
        a.hunks.len(),
        a.symbols.len()
    );
    for h in &a.hunks {
        out.push_str(&format!(
            "{:>4}  {:<8} {}:{}\n",
            h.score,
            kind_name(h),
            h.path,
            h.new_start
        ));
        if let HunkKind::Moved { from } = &h.kind {
            out.push_str(&format!("      {}\n", h.move_label(from)));
            continue;
        }
        for s in &h.signals {
            out.push_str(&format!("      +{:<3}{}\n", s.points, s.reason));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use revlens::model::{Loc, Range, RiskSignal, Rule};

    fn hunk(path: &str, kind: HunkKind, score: u32, signals: Vec<RiskSignal>) -> Hunk {
        Hunk {
            id: "h".into(),
            path: path.into(),
            old_start: 1,
            old_len: 1,
            new_start: 7,
            new_len: 1,
            lines: vec![],
            kind,
            symbols: vec![],
            signals,
            score,
        }
    }

    #[test]
    fn render_lists_scores_reasons_and_move_origins() {
        let a = Analysis {
            range: Range {
                base: "b".into(),
                head: "h".into(),
                merge_base: "0123456789abcdef".into(),
            },
            files: vec![],
            hunks: vec![
                hunk(
                    "a.py",
                    HunkKind::Changed,
                    41,
                    vec![RiskSignal {
                        rule: Rule::Size,
                        points: 1,
                        reason: "2 changed lines".into(),
                    }],
                ),
                hunk(
                    "b.py",
                    HunkKind::Moved {
                        from: Loc {
                            path: "a.py".into(),
                            line: 3,
                        },
                    },
                    0,
                    vec![],
                ),
            ],
            symbols: vec![],
            criteria: vec![],
            assumptions: vec![],
        };
        let text = render("main..feat", &a);
        assert!(text.starts_with(
            "revlens analyze main..feat (merge-base 0123456789)\n2 hunks, 0 symbol changes"
        ));
        assert!(text.contains("  41  changed  a.py:7\n      +1  2 changed lines\n"));
        assert!(text.contains("   0  moved    b.py:7\n      moved from a.py:3\n"));
    }
}
