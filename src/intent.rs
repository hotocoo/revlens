//! Intent documents: acceptance criteria and agent assumptions.

use std::collections::HashSet;
use std::io::ErrorKind;
use std::path::Path;

use anyhow::{Context, Result};

use crate::config::Config;
use crate::model::{Assumption, Criterion};

/// List items (and Given/When/Then scenarios) of an intent document, in document order.
pub fn parse_criteria(doc: &str, cfg: &Config) -> Vec<Criterion> {
    let lines = classify(doc);
    let scoped = lines
        .iter()
        .any(|l| matches!(l.kind, Kind::Heading { level: _, ref text } if is_criteria_heading(text, cfg)));
    let mut out = Vec::new();
    let mut section: Option<usize> = None; // level of the open criteria heading
    let mut gwt = Gwt::default();
    for l in &lines {
        let text = match &l.kind {
            Kind::Heading { level, text } => {
                if section.is_some_and(|s| *level <= s) {
                    section = None;
                }
                if is_criteria_heading(text, cfg) {
                    section = Some(*level);
                }
                gwt = Gwt::default();
                continue;
            }
            Kind::Item { indent, text } => {
                if let Some(t) = gwt.feed(text) {
                    t
                } else if *indent < 2 && (!scoped || section.is_some()) {
                    clean(text)
                } else {
                    continue;
                }
            }
            Kind::Text(text) => match gwt.feed(text) {
                Some(t) => t,
                None => continue,
            },
        };
        if !text.is_empty() {
            out.push(Criterion {
                id: format!("c{}", out.len() + 1),
                text,
                source_line: l.no,
            });
        }
    }
    out
}

/// PR bodies usually follow a template (summary, checklist), so by default only items under an
/// acceptance-criteria heading count; an empty result means the body has no such section.
pub fn intent_from_pr_body(body: &str, cfg: &Config) -> Vec<Criterion> {
    let scoped = classify(body)
        .iter()
        .any(|l| matches!(&l.kind, Kind::Heading { text, .. } if is_criteria_heading(text, cfg)));
    if cfg.intent.pr_require_heading && !scoped {
        return Vec::new();
    }
    parse_criteria(body, cfg)
}

/// JSONL; blank lines skipped; a missing file is no assumptions.
pub fn load_assumptions(path: &Path) -> Result<Vec<Assumption>> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        out.push(
            serde_json::from_str(line)
                .with_context(|| format!("{}: line {}", path.display(), i + 1))?,
        );
    }
    Ok(out)
}

enum Kind {
    Heading { level: usize, text: String },
    Item { indent: usize, text: String },
    Text(String),
}

struct Line {
    no: u32,
    kind: Kind,
}

/// Split into headings, list items and plain text, skipping fenced code and blank lines.
/// Blank out `<!-- ... -->` comments (PR templates), keeping line numbering intact.
fn strip_comments(doc: &str) -> String {
    let mut out = String::with_capacity(doc.len());
    let mut rest = doc;
    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        let after = &rest[start..];
        let end = after.find("-->").map_or(after.len(), |e| e + 3);
        out.extend(after[..end].chars().filter(|c| *c == '\n'));
        rest = &after[end..];
    }
    out.push_str(rest);
    out
}

fn classify(doc: &str) -> Vec<Line> {
    let doc = strip_comments(doc);
    let mut out = Vec::new();
    let mut fence = false;
    for (i, raw) in doc.lines().enumerate() {
        let t = raw.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            fence = !fence;
            continue;
        }
        if fence || t.is_empty() {
            continue;
        }
        let indent = raw.len() - t.len();
        let kind = if let Some(h) = heading(raw) {
            h
        } else if let Some(text) = item_text(t) {
            Kind::Item {
                indent,
                text: text.to_string(),
            }
        } else {
            Kind::Text(t.to_string())
        };
        out.push(Line {
            no: i as u32 + 1,
            kind,
        });
    }
    out
}

fn heading(raw: &str) -> Option<Kind> {
    if raw.len() - raw.trim_start().len() > 3 {
        return None;
    }
    let t = raw.trim();
    let level = t.bytes().take_while(|&b| b == b'#').count();
    let rest = t[level..].strip_prefix(' ')?;
    (1..=6).contains(&level).then(|| Kind::Heading {
        level,
        text: rest.trim().trim_end_matches('#').trim().to_string(),
    })
}

/// Text after a `-`, `*`, `+`, `1.` or `1)` marker.
fn item_text(t: &str) -> Option<&str> {
    let digits = t.bytes().take_while(u8::is_ascii_digit).count();
    let rest = if digits > 0 {
        t[digits..].strip_prefix(['.', ')'])?
    } else {
        t.strip_prefix(['-', '*', '+'])?
    };
    rest.strip_prefix(' ').map(str::trim)
}

/// Whole-word, case-insensitive match of any configured heading keyword.
fn is_criteria_heading(text: &str, cfg: &Config) -> bool {
    let hay = clean(text).to_lowercase();
    cfg.intent.headings.iter().any(|k| {
        let k = k.to_lowercase();
        hay.match_indices(&k).any(|(i, m)| {
            let before = hay[..i]
                .chars()
                .next_back()
                .is_none_or(|c| !c.is_alphanumeric());
            let after = hay[i + m.len()..]
                .chars()
                .next()
                .is_none_or(|c| !c.is_alphanumeric());
            before && after
        })
    })
}

/// Strip checkbox marks and emphasis markers.
fn clean(text: &str) -> String {
    let mut t = text.trim();
    for mark in ["[ ]", "[x]", "[X]"] {
        if let Some(r) = t.strip_prefix(mark) {
            t = r.trim_start();
            break;
        }
    }
    strip_emphasis(t)
        .split_whitespace()
        .map(
            |w| match w.strip_prefix('_').and_then(|r| r.strip_suffix('_')) {
                Some(inner) if !inner.is_empty() && !inner.starts_with('_') => inner,
                _ => w,
            },
        )
        .collect::<Vec<_>>()
        .join(" ")
}

/// Remove markdown emphasis stars (`**x**`, `*x*`) but keep literal ones: backtick spans,
/// unpaired `**`, and stars inside words (`*.py`, `2*3`).
fn strip_emphasis(t: &str) -> String {
    let c: Vec<char> = t.chars().collect();
    let (mut in_code, mut pairs, mut i) = (false, Vec::new(), 0);
    while i < c.len() {
        if c[i] == '`' {
            in_code = !in_code;
        } else if !in_code && c[i] == '*' && c.get(i + 1) == Some(&'*') {
            pairs.push(i);
            i += 1;
        }
        i += 1;
    }
    let drop: HashSet<usize> = if pairs.len() % 2 == 0 {
        pairs.iter().flat_map(|&p| [p, p + 1]).collect()
    } else {
        HashSet::new()
    };
    let bold: String = c
        .iter()
        .enumerate()
        .filter(|(i, _)| !drop.contains(i))
        .map(|(_, ch)| *ch)
        .collect();
    let mut in_code = false;
    let words: Vec<&str> = bold
        .split(' ')
        .map(|w| {
            let outside = !in_code;
            if w.matches('`').count() % 2 == 1 {
                in_code = !in_code;
            }
            match w.strip_prefix('*').and_then(|r| r.strip_suffix('*')) {
                Some(inner) if outside && !inner.is_empty() && !inner.contains('*') => inner,
                _ => w,
            }
        })
        .collect();
    words.join(" ")
}

/// Tracks Given/When context; yields one criterion text per Then clause.
#[derive(Default)]
struct Gwt {
    ctx: Vec<String>,
    last_then: bool,
    active: bool,
}

impl Gwt {
    fn feed(&mut self, raw: &str) -> Option<String> {
        let text = clean(raw);
        let (word, rest) = text.split_once(' ').unwrap_or((&text, ""));
        let rest = rest.trim();
        match word.to_lowercase().as_str() {
            "given" => {
                self.ctx = vec![format!("Given {rest}")];
                (self.active, self.last_then) = (true, false);
                Some(String::new())
            }
            "when" => {
                if self.last_then {
                    self.ctx.clear();
                }
                self.ctx.push(format!("When {rest}"));
                (self.active, self.last_then) = (true, false);
                Some(String::new())
            }
            "then" => {
                self.active = true;
                self.last_then = true;
                Some(self.then(rest))
            }
            "and" | "but" if self.active => {
                if self.last_then {
                    Some(self.then(rest))
                } else {
                    let w = if word.eq_ignore_ascii_case("and") {
                        "And"
                    } else {
                        "But"
                    };
                    self.ctx.push(format!("{w} {rest}"));
                    Some(String::new())
                }
            }
            _ => None,
        }
        .map(|s| s.trim_end().to_string())
    }

    fn then(&self, rest: &str) -> String {
        let mut parts = self.ctx.clone();
        parts.push(format!("Then {rest}"));
        parts.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(doc: &str) -> Vec<(String, u32)> {
        parse_criteria(doc, &Config::default())
            .into_iter()
            .map(|c| (c.text, c.source_line))
            .collect()
    }

    #[test]
    fn heading_scoped_items_stop_at_same_level() {
        let doc = "# Title\n- not me\n## Acceptance Criteria\n- [ ] one\n- [x] **two**\n1. three\n2) four\n### Notes\n* sub heading still in\n## Other\n- not me either\n";
        assert_eq!(
            texts(doc),
            vec![
                ("one".into(), 4),
                ("two".into(), 5),
                ("three".into(), 6),
                ("four".into(), 7),
                ("sub heading still in".into(), 9),
            ]
        );
    }

    #[test]
    fn literal_stars_survive_but_emphasis_is_stripped() {
        let got = |l: &str| texts(&format!("- {l}\n"))[0].0.clone();
        assert_eq!(got("Ignore `**/*.ts` files"), "Ignore `**/*.ts` files");
        assert_eq!(got("Support `*.py` globs"), "Support `*.py` globs");
        assert_eq!(got("Rate is 2*3"), "Rate is 2*3");
        assert_eq!(got("Match *.py files"), "Match *.py files");
        assert_eq!(
            got("**bold phrase** and *em* here"),
            "bold phrase and em here"
        );
        assert_eq!(got("**`code`** works"), "`code` works");
        assert_eq!(got("Unpaired ** stays"), "Unpaired ** stays");
    }

    #[test]
    fn ids_are_sequential() {
        let cs = parse_criteria("- a\n- b\n+ c\n", &Config::default());
        let ids: Vec<_> = cs.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["c1", "c2", "c3"]);
    }

    #[test]
    fn no_heading_takes_top_level_items_only() {
        assert_eq!(
            texts("intro\n- a\n  - nested\n- b\n"),
            vec![("a".into(), 2), ("b".into(), 4)]
        );
    }

    #[test]
    fn heading_keywords_are_whole_words_and_ac_is_exact() {
        assert_eq!(
            texts("## AC\n- yes\n## Package notes\n- no\n"),
            vec![("yes".into(), 2)]
        );
        assert_eq!(
            texts("## Definition of Done\n- ok\n"),
            vec![("ok".into(), 2)]
        );
        assert_eq!(texts("## Requirements:\n- ok\n"), vec![("ok".into(), 2)]);
    }

    #[test]
    fn custom_headings() {
        let cfg = Config::parse("[intent]\nheadings = [\"goals\"]\n").expect("cfg");
        let cs = parse_criteria("## Goals\n- g\n## Acceptance\n- a\n", &cfg);
        assert_eq!(cs.len(), 1);
        assert_eq!(cs[0].text, "g");
    }

    #[test]
    fn given_when_then_one_per_then() {
        let doc = "## Scenario\nGiven a user\nWhen they log in\nThen they see home\nAnd they see a greeting\n\nGiven an admin\nThen they see admin\n";
        assert_eq!(
            texts(doc),
            vec![
                (
                    "Given a user, When they log in, Then they see home".into(),
                    4
                ),
                (
                    "Given a user, When they log in, Then they see a greeting".into(),
                    5
                ),
                ("Given an admin, Then they see admin".into(), 8),
            ]
        );
    }

    #[test]
    fn gwt_as_list_items_and_emphasis() {
        let doc = "- **Given** x\n- **When** y\n- **Then** z\n";
        assert_eq!(texts(doc), vec![("Given x, When y, Then z".into(), 3)]);
    }

    #[test]
    fn gwt_and_but_continue_given_context() {
        let doc = "Given a\nAnd b\nBut not c\nWhen d\nThen e\n";
        assert_eq!(
            texts(doc),
            vec![("Given a, And b, But not c, When d, Then e".into(), 5)]
        );
    }

    #[test]
    fn gwt_alongside_scoped_items() {
        let doc = "## Acceptance\n- item\nGiven g\nThen t\n";
        assert_eq!(
            texts(doc),
            vec![("item".into(), 2), ("Given g, Then t".into(), 4)]
        );
    }

    #[test]
    fn code_fences_and_empty_items_skipped() {
        assert_eq!(
            texts("```\n- code\n```\n- [ ]\n- real\n"),
            vec![("real".into(), 5)]
        );
    }

    #[test]
    fn emphasis_stripped_identifiers_kept() {
        assert_eq!(
            texts("- _soft_ and *bold* keep my_var and _private\n"),
            vec![("soft and bold keep my_var and _private".into(), 1)]
        );
    }

    #[test]
    fn empty_doc() {
        assert!(texts("").is_empty());
        assert!(texts("just prose\n").is_empty());
    }

    #[test]
    fn pr_body_uses_same_parser() {
        let cs = intent_from_pr_body("## Requirements\n- x\n", &Config::default());
        assert_eq!(cs.len(), 1);
    }

    #[test]
    fn assumptions_jsonl() {
        let dir = tempfile::tempdir().expect("tmp");
        let p = dir.path().join("a.jsonl");
        std::fs::write(
            &p,
            "{\"text\":\"t1\",\"agent\":\"claude\",\"path\":\"a.ts\",\"line\":3,\"at\":\"x\"}\n\n  \n{\"text\":\"t2\"}\n",
        )
        .expect("write");
        let a = load_assumptions(&p).expect("load");
        assert_eq!(a.len(), 2);
        assert_eq!(a[0].agent.as_deref(), Some("claude"));
        assert_eq!(a[1].line, None);
    }

    #[test]
    fn assumptions_bad_line_names_line_number() {
        let dir = tempfile::tempdir().expect("tmp");
        let p = dir.path().join("a.jsonl");
        std::fs::write(&p, "{\"text\":\"ok\"}\n\nnot json\n").expect("write");
        let err = format!("{:#}", load_assumptions(&p).expect_err("bad"));
        assert!(err.contains("line 3"), "{err}");
    }

    #[test]
    fn assumptions_missing_file_is_empty_but_dir_errors() {
        let dir = tempfile::tempdir().expect("tmp");
        assert!(
            load_assumptions(&dir.path().join("none"))
                .expect("ok")
                .is_empty()
        );
        assert!(load_assumptions(dir.path()).is_err());
    }

    #[test]
    fn html_comments_are_ignored() {
        let doc =
            "<!-- Given this project is run by volunteers\nplease read -->\n- [ ] real item\n";
        let c = parse_criteria(doc, &Config::default());
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].text, "real item");
        assert_eq!(c[0].source_line, 3);
    }

    #[test]
    fn pr_template_without_criteria_heading_yields_nothing() {
        let body = "# Summary\nMoves helpers.\n# Checklist\n- [x] I understand this PR may be closed\n- [ ] Docs updated\n";
        assert!(intent_from_pr_body(body, &Config::default()).is_empty());
        let mut cfg = Config::default();
        cfg.intent.pr_require_heading = false;
        assert_eq!(intent_from_pr_body(body, &cfg).len(), 2);
    }

    #[test]
    fn pr_body_with_criteria_heading_keeps_only_that_section() {
        let body = "## Summary\n- context\n## Acceptance criteria\n- [ ] Fee is added\n## Checklist\n- [x] tests\n";
        let c = intent_from_pr_body(body, &Config::default());
        assert_eq!(
            c.iter().map(|c| c.text.as_str()).collect::<Vec<_>>(),
            ["Fee is added"]
        );
    }
}
