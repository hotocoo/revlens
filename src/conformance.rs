//! Conformance record: how the diff maps to the stated intent, plus what is still unreviewed.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt::Write;

use anyhow::{Context, Result};

use crate::model::{
    Analysis, ConformanceRecord, CritStatus, CriterionResult, Hunk, HunkKind, HunkRef, Rule,
    SessionState,
};

pub const SCHEMA: u32 = 1;

fn href(h: &Hunk) -> HunkRef {
    HunkRef {
        id: h.id.clone(),
        path: h.path.clone(),
        new_start: h.new_start,
        score: h.score,
    }
}

/// Moved, format-only and generated hunks need neither a criterion nor a reviewer.
fn demoted(h: &Hunk) -> bool {
    matches!(h.kind, HunkKind::Moved { .. } | HunkKind::FormatOnly)
        || h.signals.iter().any(|s| s.rule == Rule::Generated)
}

/// Mappings that point at a hunk or criterion the analysis does not have.
pub fn orphaned(analysis: &Analysis, state: &SessionState) -> usize {
    let hunks: HashSet<&str> = analysis.hunks.iter().map(|h| h.id.as_str()).collect();
    let crits: HashSet<&str> = analysis.criteria.iter().map(|c| c.id.as_str()).collect();
    state
        .mappings
        .iter()
        .filter(|m| !hunks.contains(m.hunk.as_str()) || !crits.contains(m.criterion.as_str()))
        .count()
}

pub fn build(analysis: &Analysis, state: &SessionState, generated_at: String) -> ConformanceRecord {
    let reviewers: BTreeSet<String> = state
        .mappings
        .iter()
        .map(|m| m.by.clone())
        .chain(state.reviewed.values().cloned())
        .chain(state.claims.values().map(|c| c.user.clone()))
        .collect();

    let mut by_crit: HashMap<&str, HashSet<&str>> = HashMap::new();
    for m in &state.mappings {
        by_crit.entry(&m.criterion).or_default().insert(&m.hunk);
    }
    let known: HashSet<&str> = analysis.criteria.iter().map(|c| c.id.as_str()).collect();
    let mapped: HashSet<&str> = state
        .mappings
        .iter()
        .filter(|m| known.contains(m.criterion.as_str()))
        .map(|m| m.hunk.as_str())
        .collect();

    let criteria: Vec<CriterionResult> = analysis
        .criteria
        .iter()
        .map(|c| CriterionResult {
            criterion: c.clone(),
            status: state.status.get(&c.id).copied().unwrap_or_default(),
            hunks: analysis
                .hunks
                .iter()
                .filter(|h| {
                    by_crit
                        .get(c.id.as_str())
                        .is_some_and(|s| s.contains(h.id.as_str()))
                })
                .map(href)
                .collect(),
        })
        .collect();

    let pick = |f: &dyn Fn(&Hunk) -> bool| -> Vec<HunkRef> {
        analysis.hunks.iter().filter(|h| f(h)).map(href).collect()
    };
    ConformanceRecord {
        schema: SCHEMA,
        range: analysis.range.clone(),
        generated_at,
        reviewers: reviewers.into_iter().collect(),
        missing_work: criteria
            .iter()
            .filter(|c| c.hunks.is_empty() && c.status != CritStatus::NotApplicable)
            .map(|c| c.criterion.id.clone())
            .collect(),
        criteria,
        scope_creep: pick(&|h| !demoted(h) && !mapped.contains(h.id.as_str())),
        unreviewed: pick(&|h| !demoted(h) && !state.reviewed.contains_key(&h.id)),
        reuse_misses: pick(&|h| {
            h.signals
                .iter()
                .any(|s| matches!(s.rule, Rule::ReuseMiss { .. }))
        }),
    }
}

fn status_text(s: CritStatus) -> &'static str {
    match s {
        CritStatus::Open => "OPEN",
        CritStatus::Met => "MET",
        CritStatus::NotMet => "NOT MET",
        CritStatus::NotApplicable => "N/A",
    }
}

fn loc(h: &HunkRef) -> String {
    format!("`{}:{}`", h.path, h.new_start)
}

fn cell(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
}

fn hunk_section(out: &mut String, title: &str, hunks: &[HunkRef]) {
    let _ = writeln!(out, "\n## {title}\n");
    if hunks.is_empty() {
        out.push_str("None.\n");
    }
    for h in hunks {
        let _ = writeln!(out, "- {} (risk {})", loc(h), h.score);
    }
}

pub fn render_markdown(rec: &ConformanceRecord) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# Lens conformance record\n");
    let _ = writeln!(
        out,
        "Range: `{}..{}` (merge base `{}`)  ",
        rec.range.base, rec.range.head, rec.range.merge_base
    );
    let _ = writeln!(out, "Generated: {}  ", rec.generated_at);
    let who = if rec.reviewers.is_empty() {
        "none".into()
    } else {
        rec.reviewers.join(", ")
    };
    let _ = writeln!(out, "Reviewers: {who}");
    out.push_str("\n## Criteria\n\n");
    if rec.criteria.is_empty() {
        out.push_str("None.\n");
    } else {
        out.push_str("| ID | Criterion | Status | Hunks |\n| --- | --- | --- | --- |\n");
        for c in &rec.criteria {
            let hunks = if c.hunks.is_empty() {
                "none".to_string()
            } else {
                c.hunks.iter().map(loc).collect::<Vec<_>>().join(", ")
            };
            let _ = writeln!(
                out,
                "| {} | {} | {} | {} |",
                c.criterion.id,
                cell(&c.criterion.text),
                status_text(c.status),
                hunks
            );
        }
    }
    hunk_section(&mut out, "Scope creep", &rec.scope_creep);
    out.push_str("\n## Missing work\n\n");
    if rec.missing_work.is_empty() {
        out.push_str("None.\n");
    }
    for id in &rec.missing_work {
        let text = rec
            .criteria
            .iter()
            .find(|c| &c.criterion.id == id)
            .map_or("", |c| &c.criterion.text);
        let _ = writeln!(out, "- {id}: {text}");
    }
    hunk_section(&mut out, "Unreviewed", &rec.unreviewed);
    hunk_section(&mut out, "Reuse misses", &rec.reuse_misses);
    let _ = write!(
        out,
        "\n---\nGenerated by revlens {}\n",
        env!("CARGO_PKG_VERSION")
    );
    out
}

pub fn render_json(rec: &ConformanceRecord) -> Result<String> {
    serde_json::to_string_pretty(rec).context("serializing conformance record")
}

#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
mod tests {
    use super::*;
    use crate::model::{Criterion, Loc, Mapping, Range, RiskSignal};
    use proptest::prelude::*;

    fn hunk(
        id: &str,
        path: &str,
        start: u32,
        kind: HunkKind,
        rules: Vec<Rule>,
        score: u32,
    ) -> Hunk {
        Hunk {
            id: id.into(),
            path: path.into(),
            old_start: start,
            old_len: 1,
            new_start: start,
            new_len: 1,
            lines: vec![],
            kind,
            symbols: vec![],
            signals: rules
                .into_iter()
                .map(|rule| RiskSignal {
                    rule,
                    points: 1,
                    reason: "r".into(),
                })
                .collect(),
            score,
        }
    }

    fn crit(n: usize) -> Criterion {
        Criterion {
            id: format!("c{n}"),
            text: format!("criterion {n}"),
            source_line: n as u32,
        }
    }

    fn analysis(hunks: Vec<Hunk>, ncrit: usize) -> Analysis {
        Analysis {
            range: Range {
                base: "main".into(),
                head: "feat".into(),
                merge_base: "abc123".into(),
            },
            files: vec![],
            hunks,
            symbols: vec![],
            criteria: (1..=ncrit).map(crit).collect(),
            assumptions: vec![],
        }
    }

    fn map(c: &str, h: &str, by: &str) -> Mapping {
        Mapping {
            criterion: c.into(),
            hunk: h.into(),
            by: by.into(),
        }
    }

    fn realistic() -> (Analysis, SessionState) {
        let a = analysis(
            vec![
                hunk(
                    "h1",
                    "src/auth.ts",
                    10,
                    HunkKind::Changed,
                    vec![Rule::SensitiveArea],
                    42,
                ),
                hunk(
                    "h2",
                    "src/util.ts",
                    5,
                    HunkKind::New,
                    vec![Rule::ReuseMiss {
                        existing: Loc {
                            path: "src/lib.ts".into(),
                            line: 3,
                        },
                    }],
                    25,
                ),
                hunk(
                    "h3",
                    "src/old.ts",
                    1,
                    HunkKind::Moved {
                        from: Loc {
                            path: "src/a.ts".into(),
                            line: 1,
                        },
                    },
                    vec![Rule::Moved],
                    0,
                ),
                hunk(
                    "h4",
                    "package-lock.json",
                    1,
                    HunkKind::Changed,
                    vec![Rule::Generated],
                    0,
                ),
                hunk("h5", "src/extra.ts", 7, HunkKind::Changed, vec![], 12),
            ],
            3,
        );
        let mut s = SessionState::default();
        s.mappings = vec![
            map("c1", "h1", "alice"),
            map("c2", "h2", "bob"),
            map("c1", "gone", "bob"),
            map("c9", "h1", "carol"),
        ];
        s.status.insert("c1".into(), CritStatus::Met);
        s.status.insert("c3".into(), CritStatus::NotApplicable);
        s.reviewed.insert("h1".into(), "alice".into());
        (a, s)
    }

    #[test]
    fn build_realistic() {
        let (a, s) = realistic();
        let r = build(&a, &s, "T".into());
        assert_eq!(r.schema, SCHEMA);
        assert_eq!(r.reviewers, ["alice", "bob", "carol"]);
        assert_eq!(r.criteria[0].hunks.len(), 1);
        assert_eq!(r.criteria[0].status, CritStatus::Met);
        assert_eq!(r.criteria[1].status, CritStatus::Open);
        assert!(r.missing_work.is_empty(), "c3 is N/A: {:?}", r.missing_work);
        let ids = |v: &[HunkRef]| v.iter().map(|h| h.id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(&r.scope_creep), ["h5"]);
        assert_eq!(ids(&r.unreviewed), ["h2", "h5"]);
        assert_eq!(ids(&r.reuse_misses), ["h2"]);
        assert_eq!(orphaned(&a, &s), 2);
    }

    #[test]
    fn missing_work_when_unmapped_and_not_na() {
        let (a, mut s) = realistic();
        s.status.clear();
        let r = build(&a, &s, "T".into());
        assert_eq!(r.missing_work, ["c3"]);
    }

    #[test]
    fn claims_count_as_reviewers() {
        let (a, mut s) = realistic();
        s.claims.insert(
            "h1".into(),
            crate::model::Claim {
                user: "zed".into(),
                at_ms: 1,
            },
        );
        assert!(
            build(&a, &s, "T".into())
                .reviewers
                .contains(&"zed".to_string())
        );
    }

    #[test]
    fn markdown_snapshot() {
        let (a, s) = realistic();
        let md = render_markdown(&build(&a, &s, "2026-01-02T03:04:05Z".into()));
        insta::assert_snapshot!(md.replace(env!("CARGO_PKG_VERSION"), "VERSION"));
    }

    #[test]
    fn markdown_empty_and_escapes() {
        let mut a = analysis(vec![], 0);
        a.criteria.push(Criterion {
            id: "c1".into(),
            text: "a | b".into(),
            source_line: 1,
        });
        let r = build(&a, &SessionState::default(), "T".into());
        let md = render_markdown(&r);
        assert!(md.contains("a \\| b"));
        assert!(md.contains("Reviewers: none"));
        let none = render_markdown(&build(
            &analysis(vec![], 0),
            &SessionState::default(),
            "T".into(),
        ));
        assert!(none.contains("## Criteria\n\nNone."));
    }

    #[test]
    fn json_roundtrip() {
        let (a, s) = realistic();
        let r = build(&a, &s, "T".into());
        let back: ConformanceRecord =
            serde_json::from_str(&render_json(&r).expect("json")).expect("parse");
        assert_eq!(back, r);
    }

    fn arb_kind() -> impl Strategy<Value = (HunkKind, Vec<Rule>)> {
        prop_oneof![
            Just((HunkKind::Changed, vec![])),
            Just((HunkKind::New, vec![])),
            Just((HunkKind::FormatOnly, vec![Rule::FormatOnly])),
            Just((
                HunkKind::Moved {
                    from: Loc {
                        path: "x".into(),
                        line: 1
                    }
                },
                vec![Rule::Moved]
            )),
            Just((HunkKind::Changed, vec![Rule::Generated])),
        ]
    }

    proptest! {
        #[test]
        fn partitions_hold(
            kinds in prop::collection::vec(arb_kind(), 0..12),
            ncrit in 0usize..6,
            maps in prop::collection::vec((0usize..8, 0usize..14), 0..30),
            na in prop::collection::vec(any::<bool>(), 6),
        ) {
            let hunks: Vec<Hunk> = kinds.into_iter().enumerate()
                .map(|(i, (k, r))| hunk(&format!("h{i}"), "f", i as u32, k, r, 1)).collect();
            let a = analysis(hunks, ncrit);
            let mut s = SessionState::default();
            s.mappings = maps.into_iter().map(|(c, h)| map(&format!("c{c}"), &format!("h{h}"), "u")).collect();
            for (i, n) in na.iter().enumerate() {
                if *n { s.status.insert(format!("c{}", i + 1), CritStatus::NotApplicable); }
            }
            let r = build(&a, &s, "T".into());
            for h in &a.hunks {
                let in_map = r.criteria.iter().any(|c| c.hunks.iter().any(|x| x.id == h.id));
                let in_creep = r.scope_creep.iter().any(|x| x.id == h.id);
                if demoted(h) {
                    prop_assert!(!in_creep);
                } else {
                    prop_assert!(in_map != in_creep, "{} map={} creep={}", h.id, in_map, in_creep);
                }
            }
            for c in &r.criteria {
                let missing = r.missing_work.contains(&c.criterion.id);
                if c.status == CritStatus::NotApplicable {
                    prop_assert!(!missing);
                } else {
                    prop_assert!(c.hunks.is_empty() == missing);
                }
            }
        }
    }
}
