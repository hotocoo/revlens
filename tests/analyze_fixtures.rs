mod common;

use common::{has, in_file, run, summary};
use revlens::model::{Change, HunkKind, Rule};

const PY_HELPER: &str =
    "def helper(x):\n    total = 0\n    for i in range(x):\n        total += i\n    return total\n";
const PY_KEEP: &str = "def keep():\n    return 1\n";

fn py_a(extra: &str) -> String {
    format!("{extra}\n\n{PY_KEEP}")
}

const TS_HELPER: &str = "export function helper(x: number): number {\n  let total = 0;\n  for (let i = 0; i < x; i++) {\n    total += i;\n  }\n  return total;\n}\n";
const TS_KEEP: &str = "export function keep(): number {\n  return 1;\n}\n";

fn all_moved_and_free(a: &revlens::model::Analysis) {
    assert!(!a.hunks.is_empty());
    for h in &a.hunks {
        assert!(matches!(h.kind, HunkKind::Moved { .. }), "{h:?}");
        assert_eq!(h.score, 0);
        assert!(has(h, &Rule::Moved));
    }
}

#[test]
fn python_function_move_between_files() {
    let a = run(
        &[("a.py", &py_a(PY_HELPER))],
        &[("a.py", PY_KEEP), ("b.py", PY_HELPER)],
    );
    all_moved_and_free(&a);
    assert!(
        a.symbols
            .iter()
            .any(|s| s.name == "helper" && matches!(s.change, Change::Moved { .. }))
    );
    insta::assert_json_snapshot!(summary(&a));
}

#[test]
fn typescript_function_move_between_files() {
    let a = run(
        &[("a.ts", &format!("{TS_HELPER}\n{TS_KEEP}"))],
        &[("a.ts", TS_KEEP), ("lib/b.ts", TS_HELPER)],
    );
    all_moved_and_free(&a);
    assert!(a.symbols.iter().any(|s| s.name == "helper"
        && matches!(&s.change, Change::Moved { from } if from.path == "a.ts")));
}

#[test]
fn move_plus_edit_is_moved_and_changed() {
    let edited = PY_HELPER.replace("total += i", "total += i * 2");
    let a = run(
        &[("a.py", &py_a(PY_HELPER))],
        &[("a.py", PY_KEEP), ("b.py", &edited)],
    );
    assert!(
        a.symbols
            .iter()
            .any(|s| s.name == "helper" && matches!(s.change, Change::MovedAndChanged { .. }))
    );
    let new_side = in_file(&a, "b.py");
    assert!(
        new_side
            .iter()
            .all(|h| !matches!(h.kind, HunkKind::Moved { .. }))
    );
    assert!(new_side.iter().any(|h| h.score > 0));
}

#[test]
fn renamed_function() {
    let renamed = PY_HELPER.replace("helper", "accumulate");
    let a = run(&[("a.py", PY_HELPER)], &[("a.py", &renamed)]);
    assert!(a.symbols.iter().any(|s| s.name == "accumulate"
        && s.change
            == Change::Renamed {
                from: "helper".into()
            }));
    insta::assert_json_snapshot!(summary(&a));
}

#[test]
fn public_signature_change() {
    let a = run(
        &[("lib/parse.py", "def parse(a):\n    return a\n")],
        &[("lib/parse.py", "def parse(a, b):\n    return a\n")],
    );
    let h = &a.hunks[0];
    assert!(has(h, &Rule::PublicSigChanged));
    let reason = &h
        .signals
        .iter()
        .find(|s| s.rule == Rule::PublicSigChanged)
        .expect("signal")
        .reason;
    assert_eq!(reason, "public signature of 'parse' changed: (a) -> (a, b)");
    insta::assert_json_snapshot!(summary(&a));
}

#[test]
fn deleted_test_file_and_removed_test() {
    let tests = "def test_a():\n    assert 1 == 1\n\n\ndef test_b():\n    assert 2 == 2\n";
    let a = run(
        &[
            ("src/m.py", "def f():\n    return 1\n"),
            ("tests/test_m.py", tests),
        ],
        &[("src/m.py", "def f():\n    return 1\n")],
    );
    assert!(
        in_file(&a, "tests/test_m.py")
            .iter()
            .all(|h| has(h, &Rule::TestRemoved))
    );

    let a = run(
        &[("tests/test_m.py", tests)],
        &[("tests/test_m.py", "def test_a():\n    assert 1 == 1\n")],
    );
    let h = &in_file(&a, "tests/test_m.py")[0];
    assert!(
        h.signals
            .iter()
            .any(|s| s.rule == Rule::TestRemoved && s.reason.contains("test_b"))
    );
}

#[test]
fn new_dependencies() {
    let a = run(
        &[
            (
                "package.json",
                "{\n  \"name\": \"x\",\n  \"dependencies\": {\n    \"zod\": \"^3.0.0\"\n  }\n}\n",
            ),
            ("requirements.txt", "zod\n"),
        ],
        &[
            (
                "package.json",
                "{\n  \"name\": \"x\",\n  \"dependencies\": {\n    \"left-pad\": \"^1.3.0\",\n    \"zod\": \"^3.0.0\"\n  }\n}\n",
            ),
            ("requirements.txt", "zod\nhttpx>=0.27\n"),
        ],
    );
    for (path, name) in [("package.json", "left-pad"), ("requirements.txt", "httpx")] {
        let h = &in_file(&a, path)[0];
        let s = h
            .signals
            .iter()
            .find(|s| s.rule == Rule::NewDependency)
            .unwrap_or_else(|| panic!("{path}: {h:?}"));
        assert!(s.reason.contains(name), "{}", s.reason);
    }
}

#[test]
fn suppression_and_bare_except() {
    let a = run(
        &[
            ("a.ts", "export const x = 1;\n"),
            ("b.py", "def f():\n    return 1\n"),
        ],
        &[
            ("a.ts", "// @ts-ignore\nexport const x: number = \"a\";\n"),
            (
                "b.py",
                "def f():\n    try:\n        return 1\n    except:\n        pass\n",
            ),
        ],
    );
    assert!(has(in_file(&a, "a.ts")[0], &Rule::SuppressionAdded));
    assert!(has(in_file(&a, "b.py")[0], &Rule::ErrorHandlingWeakened));
}

#[test]
fn whitespace_only_change_is_format_only() {
    let a = run(&[("a.py", "x = [1,2,3]\n")], &[("a.py", "x = [1, 2, 3]\n")]);
    let h = &a.hunks[0];
    assert_eq!(h.kind, HunkKind::FormatOnly);
    assert_eq!((h.score, has(h, &Rule::FormatOnly)), (0, true));
}

#[test]
fn lockfile_is_generated() {
    let a = run(
        &[("package-lock.json", "{\n  \"a\": \"1.0.0\"\n}\n")],
        &[(
            "package-lock.json",
            "{\n  \"a\": \"1.0.1\",\n  \"b\": \"2.0.0\"\n}\n",
        )],
    );
    let h = &a.hunks[0];
    assert!(a.files[0].generated);
    assert_eq!((h.score, has(h, &Rule::Generated)), (0, true));
}

#[test]
fn ranking_is_ordered_and_deterministic() {
    let base = [
        ("lib/parse.py", "def parse(a):\n    return a\n"),
        ("docs.txt", "one\n"),
        (".github/workflows/ci.yml", "on: push\n"),
        ("src/auth.py", "def login():\n    return 1\n"),
    ];
    let head = [
        ("lib/parse.py", "def parse(a, b):\n    return a\n"),
        ("docs.txt", "one\ntwo\n"),
        (".github/workflows/ci.yml", "on: [push, pull_request]\n"),
        ("src/auth.py", "def login():\n    return 2\n"),
    ];
    let fx = common::fixture(&base, &head).expect("fixture");
    let first = common::analyze(&fx).expect("analyze");
    let second = common::analyze(&fx).expect("analyze");
    assert_eq!(first, second);
    assert!(first.hunks.windows(2).all(|w| w[0].score >= w[1].score));
    assert_eq!(first.hunks[0].path, "lib/parse.py");
    assert_eq!(
        first.hunks.last().map(|h| h.path.as_str()),
        Some("docs.txt")
    );
    insta::assert_json_snapshot!(summary(&first));
}

#[test]
fn unparseable_file_does_not_abort() {
    let a = run(
        &[("a.py", "def f():\n    return 1\n")],
        &[("a.py", "def f(:\n    return (\n")],
    );
    assert_eq!(a.hunks.len(), 1);
}

fn dep_reason(a: &revlens::model::Analysis, path: &str) -> Option<String> {
    in_file(a, path)
        .iter()
        .flat_map(|h| h.signals.iter())
        .find(|s| s.rule == Rule::NewDependency)
        .map(|s| s.reason.clone())
}

#[test]
fn new_package_json_with_inline_dependencies() {
    let a = run(
        &[("a.txt", "x\n")],
        &[(
            "package.json",
            "{\"name\":\"x\",\"dependencies\": { \"axios\": \"1.7.0\" }}\n",
        )],
    );
    let r = dep_reason(&a, "package.json").expect("NewDependency");
    assert!(r.contains("'axios'") && !r.contains("heuristic"), "{r}");
}

#[test]
fn package_json_every_section_and_only_additions() {
    let base =
        "{\n  \"scripts\": {\"t\": \"jest\"},\n  \"dependencies\": {\"zod\": \"^3.0.0\"}\n}\n";
    let head = "{\n  \"scripts\": {\"t\": \"jest\", \"lint\": \"eslint\"},\n  \"dependencies\": {\"zod\": \"^3.1.0\"},\n  \"devDependencies\": {\"vitest\": \"1\"},\n  \"peerDependencies\": {\"react\": \"18\"},\n  \"optionalDependencies\": {\"fsevents\": \"2\"}\n}\n";
    let a = run(&[("package.json", base)], &[("package.json", head)]);
    let r = dep_reason(&a, "package.json").expect("NewDependency");
    for n in ["vitest", "react", "fsevents"] {
        assert!(r.contains(n), "{r}");
    }
    assert!(!r.contains("zod") && !r.contains("lint"), "{r}");

    let bump = head.replace("\"^3.1.0\"", "\"^3.2.0\"");
    let a = run(&[("package.json", head)], &[("package.json", &bump)]);
    assert!(dep_reason(&a, "package.json").is_none());
}

#[test]
fn dependency_moved_between_sections_is_not_new() {
    let base = "{\"dependencies\": {\"zod\": \"1\"}}\n";
    let head = "{\"devDependencies\": {\"zod\": \"1\"}}\n";
    let a = run(&[("package.json", base)], &[("package.json", head)]);
    assert!(dep_reason(&a, "package.json").is_none());
}

#[test]
fn new_pyproject_all_dependency_tables() {
    let toml = r#"[project]
name = "x"
license = "MIT"
dependencies = ["httpx>=0.27", "Rich_Text[extra] ; python_version > '3'"]

[project.optional-dependencies]
dev = ["pytest"]

[dependency-groups]
lint = ["ruff", {include-group = "dev"}]

[tool.poetry.dependencies]
python = "^3.11"
requests = "^2"

[tool.poetry.group.docs.dependencies]
mkdocs = "^1"
"#;
    let a = run(&[("a.txt", "x\n")], &[("pyproject.toml", toml)]);
    let r = dep_reason(&a, "pyproject.toml").expect("NewDependency");
    for n in ["httpx", "Rich_Text", "pytest", "ruff", "requests", "mkdocs"] {
        assert!(r.contains(&format!("'{n}'")), "{n} in {r}");
    }
    assert!(!r.contains("python") && !r.contains("MIT"), "{r}");
}

#[test]
fn pyproject_name_normalization_is_not_an_addition() {
    let base = "[project]\ndependencies = [\"Typing_Extensions>=4\"]\n";
    let head = "[project]\ndependencies = [\"typing-extensions>=4.1\"]\n";
    let a = run(&[("pyproject.toml", base)], &[("pyproject.toml", head)]);
    assert!(dep_reason(&a, "pyproject.toml").is_none());
}

#[test]
fn new_requirements_files_and_pipfile() {
    let a = run(
        &[("a.txt", "x\n")],
        &[
            (
                "requirements/dev.txt",
                "# tools\n-r base.txt\n\npytest>=8  # test runner\nblack[jupyter]==24\n",
            ),
            (
                "Pipfile",
                "[packages]\nflask = \"*\"\n[dev-packages]\npytest = \"*\"\n",
            ),
        ],
    );
    let r = dep_reason(&a, "requirements/dev.txt").expect("requirements");
    assert!(r.contains("'pytest'") && r.contains("'black'"), "{r}");
    assert!(!r.contains("base.txt"), "{r}");
    let r = dep_reason(&a, "Pipfile").expect("Pipfile");
    assert!(r.contains("'flask'") && r.contains("'pytest'"), "{r}");
}

#[test]
fn signal_lands_on_the_hunk_that_mentions_the_name() {
    let filler: String = (0..30).map(|i| format!("  \"k{i}\": \"v\",\n")).collect();
    let base = format!("{{\n  \"a\": 1,\n{filler}  \"dependencies\": {{}}\n}}\n");
    let head = format!("{{\n  \"a\": 2,\n{filler}  \"dependencies\": {{\"axios\": \"1\"}}\n}}\n");
    let a = run(&[("package.json", &base)], &[("package.json", &head)]);
    let hs = in_file(&a, "package.json");
    assert_eq!(hs.len(), 2, "{hs:?}");
    let flagged: Vec<_> = hs.iter().filter(|h| has(h, &Rule::NewDependency)).collect();
    assert_eq!(flagged.len(), 1);
    assert!(flagged[0].lines.iter().any(|l| l.text.contains("axios")));
}

#[test]
fn unparseable_manifest_falls_back_to_line_heuristic() {
    let base = "{\n  \"dependencies\": {\n    \"zod\": \"1\"\n  }\n}\n";
    let head = "{\n  \"dependencies\": {\n    \"zod\": \"1\",\n    \"axios\": \"1\",\n  }\n}\n";
    let a = run(&[("package.json", base)], &[("package.json", head)]);
    let r = dep_reason(&a, "package.json").expect("NewDependency");
    assert!(r.contains("'axios'") && r.contains("line heuristic"), "{r}");
}

const PY_PARSE: &str = "def parse_rows(rows):\n    out = []\n    for row in rows:\n        if row.get('ok'):\n            out.append(row['value'] * 2)\n        else:\n            out.append(0)\n    return out\n";
/// Same structure as `PY_PARSE`: renamed identifiers, different literals.
const PY_PARSE_COPY: &str = "def read_items(items):\n    res = []\n    for it in items:\n        if it.get('good'):\n            res.append(it['n'] * 3)\n        else:\n            res.append(1)\n    return res\n";

fn reuse_signal(a: &revlens::model::Analysis, path: &str) -> Option<(Rule, String)> {
    in_file(a, path)
        .iter()
        .flat_map(|h| h.signals.iter())
        .find(|s| matches!(s.rule, Rule::ReuseMiss { .. }))
        .map(|s| (s.rule.clone(), s.reason.clone()))
}

#[test]
fn added_function_duplicating_existing_one_is_a_reuse_miss() {
    let a = run(
        &[("lib/parse.py", PY_PARSE)],
        &[("lib/parse.py", PY_PARSE), ("lib/new.py", PY_PARSE_COPY)],
    );
    let (rule, reason) = reuse_signal(&a, "lib/new.py").expect("ReuseMiss");
    assert_eq!(
        reason,
        "new 'read_items' duplicates existing 'parse_rows' at lib/parse.py:1"
    );
    let Rule::ReuseMiss { existing } = rule else {
        unreachable!()
    };
    assert_eq!((existing.path.as_str(), existing.line), ("lib/parse.py", 1));
    assert!(reuse_signal(&a, "lib/parse.py").is_none());
    let h = &in_file(&a, "lib/new.py")[0];
    assert!(h.score >= revlens::config::Config::default().weights.reuse_miss);
}

#[test]
fn reuse_miss_detects_typescript_duplicates() {
    let ts = |name: &str, v: &str| {
        format!(
            "export function {name}({v}s: number[]): number {{\n  let t = 0;\n  for (const x of {v}s) {{\n    if (x > 0) {{\n      t += x * 2;\n    }} else {{\n      t -= 1;\n    }}\n  }}\n  return t;\n}}\n"
        )
    };
    let a = run(
        &[("a.ts", &ts("total", "n"))],
        &[("a.ts", &ts("total", "n")), ("b.ts", &ts("sumAll", "q"))],
    );
    let (_, reason) = reuse_signal(&a, "b.ts").expect("ReuseMiss");
    assert!(
        reason.contains("'sumAll' duplicates existing 'total' at a.ts:1"),
        "{reason}"
    );
}

#[test]
fn reuse_miss_ignores_moved_removed_edited_and_small_functions() {
    // Moved: not an addition.
    let a = run(&[("a.py", PY_PARSE)], &[("b.py", PY_PARSE)]);
    assert!(reuse_signal(&a, "b.py").is_none());
    // The original is deleted in the same PR: a rewrite, not a duplicate.
    let a = run(
        &[("a.py", PY_PARSE), ("k.py", "KEEP = 1\n")],
        &[("b.py", PY_PARSE_COPY), ("k.py", "KEEP = 1\n")],
    );
    assert!(reuse_signal(&a, "b.py").is_none());
    // The original is edited in the same PR.
    let edited = PY_PARSE.replace("* 2", "* 5");
    let a = run(
        &[("a.py", PY_PARSE)],
        &[("a.py", &edited), ("b.py", PY_PARSE_COPY)],
    );
    assert!(reuse_signal(&a, "b.py").is_none());
    // Too small to be interesting.
    let tiny = "def f(a):\n    return a + 1\n";
    let a = run(
        &[("a.py", tiny)],
        &[("a.py", tiny), ("b.py", "def g(b):\n    return b + 2\n")],
    );
    assert!(reuse_signal(&a, "b.py").is_none());
}

#[test]
fn reuse_scan_can_be_disabled_and_skips_minified_files() {
    let fx = common::fixture(
        &[("a.py", PY_PARSE)],
        &[("a.py", PY_PARSE), ("b.py", PY_PARSE_COPY)],
    )
    .expect("fixture");
    let mut cfg = revlens::config::Config::default();
    cfg.limits.reuse_enabled = false;
    let a = common::analyze_with(&fx, &cfg).expect("analyze");
    assert!(reuse_signal(&a, "b.py").is_none());

    cfg.limits.reuse_enabled = true;
    cfg.limits.reuse_max_file_bytes = 10;
    let a = common::analyze_with(&fx, &cfg).expect("analyze");
    assert!(reuse_signal(&a, "b.py").is_none());

    cfg.limits.reuse_max_file_bytes = 500_000;
    cfg.limits.minified_avg_line = 5;
    let a = common::analyze_with(&fx, &cfg).expect("analyze");
    assert!(reuse_signal(&a, "b.py").is_none());
}
