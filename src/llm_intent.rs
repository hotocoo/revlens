//! Opt-in criteria extraction by an external model command (`--llm-intent`).
//!
//! Lens holds no API keys and links no SDK: `intent.llm_command` is run with the prompt and the
//! document on stdin and must print a JSON array of criterion strings. Model output varies run to
//! run, while session files are keyed by the criteria texts, so the first answer for a given
//! (document, command, prompt) is cached in `.revlens/cache/` and reused by every later `review`
//! and `export`. Delete the cache file to ask again.

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::cache;
use crate::config::Intent;
use crate::model::Criterion;

/// How often a running command is polled for exit.
const POLL: Duration = Duration::from_millis(50);

pub fn criteria(doc: &str, cfg: &Intent, repo_root: &Path) -> Result<Vec<Criterion>> {
    let Some((prog, args)) = cfg.llm_command.split_first() else {
        bail!(
            "--llm-intent needs intent.llm_command in .revlens/config.toml (for example [\"claude\", \"-p\"])"
        );
    };
    let argv = cfg.llm_command.join("\0");
    let key = cache::key(
        "intent",
        &[doc.as_bytes(), argv.as_bytes(), cfg.llm_prompt.as_bytes()],
    );
    let texts = match cache::get::<Vec<String>>(repo_root, &key) {
        Some(t) => t,
        None => {
            let input = format!("{}\n\n---\n\n{doc}", cfg.llm_prompt);
            let out = run(
                prog,
                args,
                &input,
                Duration::from_secs(cfg.llm_timeout_secs),
            )?;
            let texts = parse_reply(&out, cfg)?;
            cache::put(repo_root, &key, &texts).context("caching the extracted criteria")?;
            eprintln!(
                "note: {} criteria extracted by `{prog}`; cached under {} (delete it to re-extract)",
                texts.len(),
                cache::DIR
            );
            texts
        }
    };
    Ok(texts
        .into_iter()
        .enumerate()
        .map(|(i, text)| Criterion {
            id: format!("c{}", i + 1),
            source_line: line_of(doc, &text),
            text,
        })
        .collect())
}

/// Run `prog args`, feed `input` on stdin, and return stdout; fail on a non-zero exit or after
/// `timeout`.
fn run(prog: &str, args: &[String], input: &str, timeout: Duration) -> Result<String> {
    let mut child = Command::new(prog)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("running intent.llm_command `{prog}`"))?;
    let mut stdin = child.stdin.take().context("opening stdin")?;
    let input = input.to_string();
    let writer = std::thread::spawn(move || stdin.write_all(input.as_bytes()));
    let (mut so, mut se) = (
        child.stdout.take().context("opening stdout")?,
        child.stderr.take().context("opening stderr")?,
    );
    let out = std::thread::spawn(move || {
        let mut s = String::new();
        so.read_to_string(&mut s).map(|_| s)
    });
    let err = std::thread::spawn(move || {
        let mut s = String::new();
        se.read_to_string(&mut s).map(|_| s)
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(st) = child.try_wait().context("waiting for intent.llm_command")? {
            break st;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("intent.llm_command `{prog}` timed out after {timeout:?}");
        }
        std::thread::sleep(POLL);
    };
    // A command that exits without reading stdin closes the pipe; that is not our error.
    let _ = writer.join();
    let stdout = out
        .join()
        .map_err(|_| anyhow::anyhow!("stdout reader panicked"))?
        .context("reading stdout")?;
    let stderr = err.join().ok().and_then(Result::ok).unwrap_or_default();
    if !status.success() {
        bail!(
            "intent.llm_command `{prog}` failed ({status}): {}",
            stderr.trim()
        );
    }
    Ok(stdout)
}

/// The outermost `[...]` of `reply` as a JSON array of non-empty strings, within the limits.
fn parse_reply(reply: &str, cfg: &Intent) -> Result<Vec<String>> {
    let (Some(start), Some(end)) = (reply.find('['), reply.rfind(']')) else {
        bail!("intent.llm_command printed no JSON array");
    };
    let json = reply.get(start..=end).unwrap_or_default();
    let raw: Vec<String> = serde_json::from_str(json)
        .context("intent.llm_command output is not a JSON array of strings")?;
    let texts: Vec<String> = raw
        .into_iter()
        .map(|t| t.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|t| !t.is_empty())
        .collect();
    if texts.is_empty() {
        bail!("intent.llm_command returned no criteria");
    }
    if texts.len() > cfg.llm_max_criteria {
        bail!(
            "intent.llm_command returned {} criteria (intent.llm_max_criteria is {})",
            texts.len(),
            cfg.llm_max_criteria
        );
    }
    if let Some(long) = texts.iter().find(|t| t.chars().count() > cfg.llm_max_chars) {
        bail!(
            "a criterion is longer than intent.llm_max_chars ({}): {}...",
            cfg.llm_max_chars,
            long.chars().take(60).collect::<String>()
        );
    }
    Ok(texts)
}

/// 1-based line of the document sharing the most words with `text`, or 0 if none share any.
fn line_of(doc: &str, text: &str) -> u32 {
    let words: Vec<String> = text
        .split_whitespace()
        .map(|w| w.to_lowercase())
        .filter(|w| w.len() > 3)
        .collect();
    let mut best = (0, 0u32);
    for (i, line) in doc.lines().enumerate() {
        let l = line.to_lowercase();
        let hits = words.iter().filter(|w| l.contains(w.as_str())).count();
        if hits > best.0 {
            best = (hits, u32::try_from(i + 1).unwrap_or(u32::MAX));
        }
    }
    best.1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(cmd: &[&str]) -> Intent {
        Intent {
            llm_command: cmd.iter().map(|s| s.to_string()).collect(),
            ..Intent::default()
        }
    }

    #[test]
    fn reply_parsing_validates_shape_and_limits() {
        let c = Intent::default();
        assert_eq!(
            parse_reply("Sure:\n[\"a  b\", \"\", \"c\"]\nDone", &c).expect("ok"),
            ["a b", "c"]
        );
        assert!(parse_reply("no array", &c).is_err());
        assert!(parse_reply("[1, 2]", &c).is_err());
        assert!(parse_reply("[\"\"]", &c).is_err());
        let small = Intent {
            llm_max_criteria: 1,
            llm_max_chars: 3,
            ..Intent::default()
        };
        assert!(parse_reply("[\"a\", \"b\"]", &small).is_err());
        assert!(parse_reply("[\"abcd\"]", &small).is_err());
    }

    #[test]
    fn line_of_finds_the_best_matching_line() {
        let doc = "# Ticket\nUsers can reset their password by email.\nOther text.\n";
        assert_eq!(line_of(doc, "Password reset works by email"), 2);
        assert_eq!(line_of(doc, "zzz"), 0);
    }

    #[test]
    fn missing_command_is_a_clear_error() {
        let dir = tempfile::tempdir().expect("tmp");
        let e = criteria("doc", &Intent::default(), dir.path()).expect_err("no command");
        assert!(format!("{e:#}").contains("intent.llm_command"), "{e:#}");
    }

    #[cfg(unix)]
    #[test]
    fn command_output_is_cached_and_reused() {
        let dir = tempfile::tempdir().expect("tmp");
        let counter = dir.path().join("calls");
        let script = format!(
            "cat > /dev/null; echo x >> '{}'; echo '[\"Rate limit logins\", \"Read limit from config\"]'",
            counter.display()
        );
        let c = cfg(&["sh", "-c", &script]);
        let doc = "We need to rate limit logins.\nThe limit comes from config.\n";
        let first = criteria(doc, &c, dir.path()).expect("first");
        assert_eq!(first[0].id, "c1");
        assert_eq!(first[1].text, "Read limit from config");
        assert_eq!(first[0].source_line, 1);
        let again = criteria(doc, &c, dir.path()).expect("cached");
        assert_eq!(again, first);
        let calls = std::fs::read_to_string(&counter).expect("calls");
        assert_eq!(calls.lines().count(), 1, "second run reads the cache");
    }

    #[cfg(unix)]
    #[test]
    fn failing_and_slow_commands_are_errors() {
        let dir = tempfile::tempdir().expect("tmp");
        let fail = cfg(&["sh", "-c", "echo boom >&2; exit 3"]);
        let e = criteria("d1", &fail, dir.path()).expect_err("fails");
        assert!(format!("{e:#}").contains("boom"), "{e:#}");
        let slow = Intent {
            llm_timeout_secs: 0,
            ..cfg(&["sh", "-c", "sleep 5"])
        };
        let e = criteria("d2", &slow, dir.path()).expect_err("times out");
        assert!(format!("{e:#}").contains("timed out"), "{e:#}");
    }
}
