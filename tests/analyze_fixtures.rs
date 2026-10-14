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
