#![deny(clippy::unwrap_used)]

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use revlens::analyze;
use revlens::config::Config;
use revlens::git::{Repo, parse_range};
use revlens::model::{Analysis, Hunk, HunkKind};

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
    },
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
        Cmd::Analyze { range, repo, json } => {
            let repo = Repo::discover(&repo)?;
            let cfg = Config::load(repo.root())?;
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
            out.push_str(&format!("      moved from {}:{}\n", from.path, from.line));
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
