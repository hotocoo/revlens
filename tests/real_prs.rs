//! Network tests against real pull requests. Run with `cargo test --test real_prs -- --ignored`.

mod common;

use common::git;
use revlens::analyze;
use revlens::config::Config;
use revlens::git::Repo;
use revlens::model::{Analysis, Change, HunkKind};

/// Blobless clone of `url`, fetch `pull/<n>/head`, analyze `base...head`.
fn analyze_pr(url: &str, pr: u32, base: &str, head: &str) -> Analysis {
    let dir = tempfile::tempdir().expect("tempdir");
    let target = dir.path().join("repo");
    let target_str = target.to_str().expect("utf-8 path");
    git(
        dir.path(),
        &[
            "clone",
            "-q",
            "--filter=blob:none",
            "--no-checkout",
            url,
            target_str,
        ],
    )
    .expect("clone");
    git(
        &target,
        &["fetch", "-q", "origin", &format!("pull/{pr}/head")],
    )
    .expect("fetch pr");
    let repo = Repo::discover(&target).expect("repo");
    analyze::run(&repo, base, head, &Config::default()).expect("analyze")
}

fn dump(a: &Analysis) {
    for s in &a.symbols {
        eprintln!("symbol {} {} {:?}", s.path, s.name, s.change);
    }
    for h in a
        .hunks
        .iter()
        .filter(|h| matches!(h.kind, HunkKind::Moved { .. }))
    {
        eprintln!("moved hunk {}:{} {:?}", h.path, h.new_start, h.kind);
    }
}

#[test]
#[ignore = "network"]
fn httpx_3389_moves_helpers_out_of_utils() {
    let a = analyze_pr(
        "https://github.com/encode/httpx",
        3389,
        "b47d94c90445",
        "433684fc8a10",
    );
    dump(&a);
    // Tests moved verbatim from tests/test_utils.py: pure moves, demoted.
    assert!(
        a.hunks
            .iter()
            .any(|h| matches!(h.kind, HunkKind::Moved { .. }) && h.path == "tests/test_utils.py")
    );
    assert!(
        a.symbols
            .iter()
            .any(|s| matches!(s.change, Change::Moved { .. }))
    );
    // `same_origin` left httpx/_utils.py as `_same_origin`, now calling the renamed
    // `_port_or_default`: linked to its origin but not demoted, because the body changed.
    assert!(a.symbols.iter().any(|s| s.name == "_same_origin"
        && matches!(&s.change, Change::MovedAndChanged { from } if from.path == "httpx/_utils.py")));
}

#[test]
#[ignore = "network"]
fn zod_6541_has_rename_or_move() {
    let a = analyze_pr(
        "https://github.com/colinhacks/zod",
        6541,
        "68aca3dc402c",
        "a906e585c143",
    );
    dump(&a);
    // The PR renames `process` to `processSchema` inside to-json-schema.ts.
    assert!(a.symbols.iter().any(|s| {
        s.name == "processSchema"
            && s.change
                == Change::Renamed {
                    from: "process".into(),
                }
    }));
}
