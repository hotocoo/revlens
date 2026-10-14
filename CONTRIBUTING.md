# Contributing

Issues and pull requests are welcome.

## Setup

Requirements: Rust 1.85 or newer, `git`, and Node 22 for the UI. `gh` is only needed for `review --pr` and for posting exports.

```sh
git clone https://github.com/hotocoo/revlens && cd revlens
cargo build
cargo test
```

### UI

The Preact UI lives in `ui/`. Its bundle in `ui/dist/` is committed and embedded into the binary, so `cargo build` needs no Node. After changing `ui/src/`:

```sh
cd ui
npm ci
npm run check    # tsc --noEmit
npm run build    # writes ui/dist
```

Commit the regenerated `ui/dist`. CI fails if it is out of date.

## Before you push

```sh
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
```

Tests that need the network (real public PRs at pinned commits) are `#[ignore]`d. Run them with `cargo test -- --ignored`; CI runs them nightly.

### Coverage

CI requires 80% line coverage, excluding `src/main.rs` and `src/server.rs`:

```sh
cargo install cargo-llvm-cov
cargo llvm-cov --ignore-filename-regex 'src/(main|server)\.rs' --fail-under-lines 80
```

## Rules of the codebase

- Write the test first.
- No `unwrap` or `expect` outside tests. Use `anyhow` with context.
- Every tunable (weights, patterns, limits, addresses) is a field of `Config` in `src/config.rs` with a default. Add the field to `examples/config.toml` too; `tests/config_example.rs` fails if they differ.
- No stub or demo data. Keep files under 800 lines.
- Do not add a dependency unless the standard library and the existing dependencies cannot do the job.

## Adding a language

Lens parses with tree-sitter. To add a language:

1. Add the grammar crate to `Cargo.toml`.
2. Add a `Lang` variant in `src/model.rs`. In `src/lang.rs`, map its file extensions in `detect`, load the grammar in `defs`, and add a walker that emits a `Def` (name, kind, public, signature, fingerprints) for functions, classes and methods.
3. Add fixtures under `tests/` with a small before and after repository and an `insta` snapshot of the analysis.
4. If the language has a dependency manifest, teach `src/deps.rs` to read it so the new-dependency rule works.
5. Add any language-specific defaults (test paths, suppression markers, lockfiles) to `src/config.rs` and `examples/config.toml`.

## License

By contributing you agree that your work is licensed under MIT OR Apache-2.0, as the project is.
