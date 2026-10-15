# Changelog

All notable changes are listed here. The format follows Keep a Changelog and the project follows semantic versioning.

## 0.2.0

- `revlens export --post` comments the record on the PR (`--pr`) and edits that comment on reruns.
- `revlens export --fail-on <conditions>` and `--fail-on-score <N>` exit with status 2 when the record fails them.
- GitHub Action (`uses: hotocoo/revlens@<tag>`): installs a release binary, posts the record, writes the job summary.
- Rust and Go: definitions, moves, renames, signature changes and reuse misses; `Cargo.toml` and `go.mod` dependencies; `#[allow(` and `//nolint` suppressions; `go.sum` and `.pb.go` treated as generated.
- Near-duplicate reuse misses by AST shingle similarity (`limits.near_duplicate_percent`, default 75).
- Parsing runs on all cores (`limits.threads`); analyses are cached in `.revlens/cache/` (`--no-cache`, `limits.cache_enabled`).
- Rust struct and enum signatures ignore comments; `/tests.rs` counts as a test path.
- `--llm-intent`: criteria extracted by a configured command (`intent.llm_command`), cached so `review` and `export` agree.
- `revlens mcp`: MCP stdio server with a validated `record_assumption` tool for coding agents.
- Re-anchoring: after a force-push or rebase, a review carries over to the new commits for every hunk whose content is unchanged.
- Comment threads on hunks and lines (reply, resolve), live line cursors, `open_threads` in the record and `--fail-on open-threads`.
- `--intent` and `--pr` can be combined: range from the PR, criteria from the file.

## 0.1.0

First release.

- `revlens analyze <range> [--json]`: hunks ranked by review risk, with a reason for every point.
- Structural diff for TypeScript, TSX and Python using tree-sitter: added, removed, renamed, moved and signature-changed symbols.
- Moved, renamed and format-only changes are detected and demoted to score 0, never hidden.
- Risk rules: public signature change, test removed, new dependency, sensitive area, reuse miss, suppression added, error handling weakened, infrastructure config, no test touched, size.
- Intent files: acceptance criteria from headings, checkboxes, bullets and Given/When/Then scenarios.
- `.revlens/assumptions.jsonl`: assumptions recorded by coding agents, shown next to the diff.
- `revlens review`: local web UI with live multi-reviewer sessions (Yjs CRDT over WebSocket), hunk claims, presence, criterion-to-hunk mapping, and persistence across restarts.
- `revlens review --pr N`: range and criteria taken from a GitHub PR through the `gh` CLI.
- `revlens export`: conformance record as Markdown or JSON (criteria status, scope creep, missing work, unreviewed hunks, reuse misses).
- Security: localhost by default, 128-bit session token, `--share` for LAN, cross-origin guard, message, document and connection limits.
- `.revlens/config.toml`: every weight, pattern, limit and server setting is configurable; see `examples/config.toml`.
