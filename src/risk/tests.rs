use super::*;
use crate::model::{Lang, Line, Loc, SymKind};

fn hunk(path: &str, spec: &[&str]) -> Hunk {
    let lines: Vec<Line> = spec
        .iter()
        .map(|s| {
            let (k, t) = s.split_at(1);
            let kind = match k {
                "+" => LineKind::Added,
                "-" => LineKind::Removed,
                _ => LineKind::Context,
            };
            Line {
                kind,
                text: t.to_string(),
            }
        })
        .collect();
    Hunk {
        id: format!("h_{path}"),
        path: path.into(),
        old_start: 1,
        old_len: 1,
        new_start: 1,
        new_len: 1,
        lines,
        kind: HunkKind::Changed,
        symbols: vec![],
        signals: vec![],
        score: 0,
    }
}

fn file(path: &str, status: FileStatus, lang: Option<Lang>) -> FileDiff {
    FileDiff {
        old_path: Some(path.into()),
        new_path: Some(path.into()),
        status,
        lang,
        generated: false,
    }
}

fn sym(name: &str, path: &str, change: Change, public: bool) -> SymbolChange {
    SymbolChange {
        name: name.into(),
        kind: SymKind::Function,
        path: path.into(),
        public,
        change,
        old_sig: Some("(a)".into()),
        new_sig: Some("(a, b)".into()),
        hunks: vec![],
    }
}

/// Score one hunk; `extra` are other changed files (so NoTestTouch can be satisfied).
fn scored(h: Hunk, f: FileDiff, syms: &[SymbolChange], extra: &[FileDiff]) -> Hunk {
    let mut h = h;
    h.symbols = (0..syms.len()).collect();
    let mut files = vec![f];
    files.extend_from_slice(extra);
    let mut hs = [h];
    score(
        &files,
        &mut hs,
        syms,
        &Findings::default(),
        &Config::default(),
    );
    let [h] = hs;
    h
}

fn rules(h: &Hunk) -> Vec<Rule> {
    h.signals.iter().map(|s| s.rule.clone()).collect()
}

fn reason(h: &Hunk, rule: Rule) -> String {
    h.signals
        .iter()
        .find(|s| s.rule == rule)
        .map(|s| s.reason.clone())
        .unwrap_or_default()
}

fn doc(path: &str) -> FileDiff {
    file(path, FileStatus::Modified, None)
}

#[test]
fn public_signature_change_and_removal() {
    let s = [sym("parse", "lib/a.py", Change::SignatureChanged, true)];
    let h = scored(
        hunk("lib/a.py", &["-def parse(a):", "+def parse(a, b):"]),
        file("lib/a.py", FileStatus::Modified, Some(Lang::Python)),
        &s,
        &[],
    );
    assert_eq!(
        reason(&h, Rule::PublicSigChanged),
        "public signature of 'parse' changed: (a) -> (a, b)"
    );
    let s = [
        sym("parse", "lib/a.py", Change::Removed, true),
        sym("_p", "lib/a.py", Change::Removed, false),
    ];
    let h = scored(
        hunk("lib/a.py", &["-def parse(a):"]),
        doc("lib/a.py"),
        &s,
        &[],
    );
    assert_eq!(
        reason(&h, Rule::PublicSigChanged),
        "public symbol 'parse' removed"
    );
    let s = [sym("_p", "lib/a.py", Change::SignatureChanged, false)];
    assert!(
        !rules(&scored(hunk("lib/a.py", &["-x"]), doc("lib/a.py"), &s, &[]))
            .contains(&Rule::PublicSigChanged)
    );
}

#[test]
fn test_removal_variants() {
    let mut gone = file("tests/test_a.py", FileStatus::Deleted, Some(Lang::Python));
    gone.new_path = None;
    let h = scored(hunk("tests/test_a.py", &["-def test_a():"]), gone, &[], &[]);
    assert_eq!(
        reason(&h, Rule::TestRemoved),
        "test file deleted: tests/test_a.py"
    );

    let s = [sym(
        "TestX.test_y",
        "tests/test_a.py",
        Change::Removed,
        true,
    )];
    let h = scored(
        hunk("tests/test_a.py", &["-    def test_y(self):"]),
        doc("tests/test_a.py"),
        &s,
        &[],
    );
    assert_eq!(
        reason(&h, Rule::TestRemoved),
        "test removed: 'TestX.test_y'"
    );

    let h = scored(
        hunk(
            "web/a.test.ts",
            &[
                "-expect(a).toBe(1);",
                "-expect(b).toBe(2);",
                "+expect(a).toBe(1);",
            ],
        ),
        doc("web/a.test.ts"),
        &[],
        &[],
    );
    assert_eq!(
        reason(&h, Rule::TestRemoved),
        "assertions dropped in test file: 2 -> 1"
    );

    let h = scored(
        hunk(
            "web/a.test.ts",
            &["-expect(a).toBe(1);", "+expect(a).toBe(2);"],
        ),
        doc("web/a.test.ts"),
        &[],
        &[],
    );
    assert!(!rules(&h).contains(&Rule::TestRemoved));
    let h = scored(hunk("src/a.ts", &["-assert(x)"]), doc("src/a.ts"), &[], &[]);
    assert!(!rules(&h).contains(&Rule::TestRemoved));
}

#[test]
fn dependency_only_in_manifests() {
    let h = scored(
        hunk(
            "package.json",
            &[r#" "dependencies": {"#, r#"+"left-pad": "^1.0.0""#],
        ),
        doc("package.json"),
        &[],
        &[],
    );
    assert_eq!(
        reason(&h, Rule::NewDependency),
        "new dependency 'left-pad' in package.json"
    );
    let h = scored(
        hunk("requirements.txt", &["+httpx>=0.27"]),
        doc("requirements.txt"),
        &[],
        &[],
    );
    assert_eq!(
        h.signals[0].points,
        Config::default().weights.new_dependency
    );
    let h = scored(
        hunk("notes.txt", &["+httpx>=0.27"]),
        doc("notes.txt"),
        &[],
        &[],
    );
    assert!(!rules(&h).contains(&Rule::NewDependency));
    let h = scored(
        hunk("package.json", &[r#"+"name": "x""#]),
        doc("package.json"),
        &[],
        &[],
    );
    assert!(!rules(&h).contains(&Rule::NewDependency));
}

#[test]
fn sensitive_lists_path_and_line_matches_once() {
    let h = scored(
        hunk(
            "src/auth/login.py",
            &["+tok = get_token()", "+tok2 = get_token()", "+x = 1"],
        ),
        doc("src/auth/login.py"),
        &[],
        &[],
    );
    assert_eq!(
        reason(&h, Rule::SensitiveArea),
        "sensitive area: path matches 'auth'; added lines mention 'token'"
    );
    let h = scored(
        hunk("src/a.py", &["-password = 1", "+x = 1"]),
        doc("src/a.py"),
        &[],
        &[],
    );
    assert!(!rules(&h).contains(&Rule::SensitiveArea));
}

#[test]
fn suppression_markers() {
    let h = scored(
        hunk("a.ts", &[" ok", "+// @ts-ignore", "+x as any"]),
        doc("a.ts"),
        &[],
        &[],
    );
    assert_eq!(
        reason(&h, Rule::SuppressionAdded),
        "suppression added: '@ts-ignore' (line 2), 'as any' (line 3)"
    );
    let h = scored(hunk("a.ts", &["-// @ts-ignore"]), doc("a.ts"), &[], &[]);
    assert!(!rules(&h).contains(&Rule::SuppressionAdded));
}

#[test]
fn error_handling_weakened_variants() {
    let rm = scored(
        hunk(
            "a.py",
            &[
                "-    try:",
                "-        f()",
                "-    except ValueError:",
                "+    f()",
            ],
        ),
        doc("a.py"),
        &[],
        &[],
    );
    assert!(reason(&rm, Rule::ErrorHandlingWeakened).starts_with("error handling removed: 2"));
    let keep = scored(
        hunk("a.py", &["-    try:", "+        try:"]),
        doc("a.py"),
        &[],
        &[],
    );
    assert!(!rules(&keep).contains(&Rule::ErrorHandlingWeakened));
    for lines in [
        &["+    except:"][..],
        &["+    except Exception:", "+        pass"],
        &["+} catch (e) {", "+}"],
        &["+try { f() } catch {}"][..],
    ] {
        let h = scored(hunk("a.ts", lines), doc("a.ts"), &[], &[]);
        let expect = lines != &["+try { f() } catch {}"][..];
        assert_eq!(
            rules(&h).contains(&Rule::ErrorHandlingWeakened),
            expect,
            "{lines:?}"
        );
    }
    let h = scored(
        hunk("a.ts", &["+} catch (e) {", "+  log(e)", "+}"]),
        doc("a.ts"),
        &[],
        &[],
    );
    assert!(!rules(&h).contains(&Rule::ErrorHandlingWeakened));
}

#[test]
fn infra_and_size() {
    let h = scored(
        hunk(".github/workflows/ci.yml", &["+a", "+b", "+c", "+d"]),
        doc(".github/workflows/ci.yml"),
        &[],
        &[],
    );
    assert!(reason(&h, Rule::InfraConfig).contains("'.github/workflows/'"));
    assert_eq!(
        h.signals
            .iter()
            .find(|s| s.rule == Rule::Size)
            .map(|s| s.points),
        Some(2)
    );
    let big: Vec<String> = (0..5000).map(|i| format!("+l{i}")).collect();
    let spec: Vec<&str> = big.iter().map(String::as_str).collect();
    let h = scored(hunk("big.txt", &spec), doc("big.txt"), &[], &[]);
    assert_eq!(
        h.signals
            .iter()
            .find(|s| s.rule == Rule::Size)
            .map(|s| s.points),
        Some(10)
    );
    let h = scored(hunk("one.txt", &["+a"]), doc("one.txt"), &[], &[]);
    assert!(h.signals.is_empty());
}

#[test]
fn no_test_touch_needs_matching_changed_test() {
    let src = || file("src/parse.py", FileStatus::Modified, Some(Lang::Python));
    let h = scored(hunk("src/parse.py", &["+x"]), src(), &[], &[]);
    assert!(reason(&h, Rule::NoTestTouch).contains("'parse'"));
    let t = file(
        "tests/test_parse.py",
        FileStatus::Modified,
        Some(Lang::Python),
    );
    assert!(
        !rules(&scored(hunk("src/parse.py", &["+x"]), src(), &[], &[t]))
            .contains(&Rule::NoTestTouch)
    );
    let unrelated = file(
        "tests/test_other.py",
        FileStatus::Modified,
        Some(Lang::Python),
    );
    assert!(
        rules(&scored(
            hunk("src/parse.py", &["+x"]),
            src(),
            &[],
            &[unrelated]
        ))
        .contains(&Rule::NoTestTouch)
    );
}

#[test]
fn demotions_zero_the_score_but_keep_the_hunk() {
    let mut h = hunk("src/auth.py", &["+token = 1"]);
    h.kind = HunkKind::Moved {
        from: Loc {
            path: "old.py".into(),
            line: 7,
        },
    };
    let h = scored(
        h,
        file("src/auth.py", FileStatus::Modified, Some(Lang::Python)),
        &[],
        &[],
    );
    assert_eq!((h.score, rules(&h)), (0, vec![Rule::Moved]));
    assert_eq!(h.signals[0].reason, "moved from old.py:7");

    let mut h = hunk("src/auth.py", &["-a  b", "+a b"]);
    h.kind = HunkKind::FormatOnly;
    let h = scored(h, doc("src/auth.py"), &[], &[]);
    assert_eq!((h.score, rules(&h)), (0, vec![Rule::FormatOnly]));

    let mut f = doc("package-lock.json");
    f.generated = true;
    let h = scored(
        hunk("package-lock.json", &[r#"+"left-pad": "^1.0.0""#]),
        f,
        &[],
        &[],
    );
    assert_eq!((h.score, rules(&h)), (0, vec![Rule::Generated]));
}

#[test]
fn score_is_sum_and_rescoring_is_idempotent() {
    let mut hs = [hunk(".github/workflows/ci.yml", &["+a"])];
    let files = [doc(".github/workflows/ci.yml")];
    let cfg = Config::default();
    score(&files, &mut hs, &[], &Findings::default(), &cfg);
    score(&files, &mut hs, &[], &Findings::default(), &cfg);
    assert_eq!(hs[0].score, cfg.weights.infra_config);
    assert_eq!(hs[0].signals.len(), 1);
}

#[test]
fn rank_is_total_and_deterministic() {
    let mk = |path: &str, new_start: u32, score: u32| {
        let mut h = hunk(path, &["+a"]);
        h.new_start = new_start;
        h.score = score;
        h.id = format!("{path}{new_start}");
        h
    };
    let mut hs = vec![
        mk("b", 1, 5),
        mk("a", 9, 5),
        mk("a", 2, 5),
        mk("z", 1, 50),
        mk("a", 2, 0),
    ];
    rank(&mut hs);
    let order: Vec<_> = hs.iter().map(|h| h.id.as_str()).collect();
    assert_eq!(order, ["z1", "a2", "a9", "b1", "a2"]);
}

#[test]
fn matching_is_case_insensitive_with_leading_slash() {
    let pats = vec!["/tests/".to_string(), "Makefile".to_string()];
    assert_eq!(matching("tests/a.py", &pats), ["/tests/"]);
    assert_eq!(matching("sub/makefile", &pats), ["Makefile"]);
}

#[test]
fn no_test_touch_requires_whole_token_match() {
    let src = |p: &str| file(p, FileStatus::Modified, Some(Lang::Python));
    let t = |p: &str| file(p, FileStatus::Modified, Some(Lang::Python));
    let covered = |s: &str, tests: &[&str]| {
        let extra: Vec<_> = tests.iter().map(|p| t(p)).collect();
        !rules(&scored(hunk(s, &["+x"]), src(s), &[], &extra)).contains(&Rule::NoTestTouch)
    };
    assert!(!covered("src/io.py", &["tests/test_ratio.py"]));
    assert!(!covered("src/db.py", &["tests/test_dbx.py"]));
    assert!(!covered("src/a.py", &["tests/test_cat.py"]));
    assert!(covered("src/io.py", &["tests/test_io.py"]));
    assert!(covered("src/http_utils.py", &["tests/test_http_utils.py"]));
}

#[test]
fn test_prefix_is_anchored_to_basename() {
    let f = file(
        "src/latest_release.py",
        FileStatus::Modified,
        Some(Lang::Python),
    );
    let h = scored(hunk("src/latest_release.py", &["+x"]), f, &[], &[]);
    assert!(rules(&h).contains(&Rule::NoTestTouch));
    assert!(is_test_path("src/test_a.py", &Config::default().patterns));
    assert!(is_test_path("test_a.py", &Config::default().patterns));
}

#[test]
fn new_dependency_in_requirements_variants() {
    for p in [
        "requirements-prod.txt",
        "requirements_dev.txt",
        "requirements/base.txt",
    ] {
        let h = scored(
            hunk(p, &["+httpx>=0.27"]),
            file(p, FileStatus::Modified, None),
            &[],
            &[],
        );
        assert!(rules(&h).contains(&Rule::NewDependency), "{p}");
    }
}

#[test]
fn suppression_and_sensitive_fire_on_net_additions_only() {
    // Re-indented line keeps its existing markers: nothing was added.
    let h = scored(
        hunk(
            "src/a.py",
            &[
                "-x = f(y)  # noqa: F821  token",
                "+    x = f(y)  # noqa: F821  token",
            ],
        ),
        doc("src/a.py"),
        &[],
        &[],
    );
    assert!(!rules(&h).contains(&Rule::SuppressionAdded));
    assert!(!rules(&h).contains(&Rule::SensitiveArea));
    // One removed, two added: the second is new.
    let h = scored(
        hunk(
            "a.ts",
            &["-a as any", "+b as any", "+c as any", "+// token"],
        ),
        doc("a.ts"),
        &[],
        &[],
    );
    assert_eq!(
        reason(&h, Rule::SuppressionAdded),
        "suppression added: 'as any' (line 2)"
    );
    assert_eq!(
        reason(&h, Rule::SensitiveArea),
        "sensitive area: added lines mention 'token'"
    );
}

#[test]
fn public_rename_is_reported_and_not_demoted() {
    let mut s = sym("g", "lib.py", Change::Renamed { from: "f".into() }, true);
    s.new_sig = None;
    let h = scored(
        hunk("lib.py", &["-def f(a):", "+def g(a):"]),
        file("lib.py", FileStatus::Modified, Some(Lang::Python)),
        &[s],
        &[],
    );
    assert_eq!(
        reason(&h, Rule::PublicSigChanged),
        "public symbol 'f' renamed to 'g'"
    );
}

#[test]
fn moved_label_points_to_destination_on_the_removed_side() {
    let from = Loc {
        path: "new.py".into(),
        line: 3,
    };
    let mut h = hunk("old.py", &["-a = 1", "-b = 2"]);
    h.kind = HunkKind::Moved { from: from.clone() };
    let h = scored(h, doc("old.py"), &[], &[]);
    assert_eq!(h.signals[0].reason, "moved to new.py:3");
}
