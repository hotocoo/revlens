//! Tunables. Every value has a default here and can be overridden in `.revlens/config.toml`.

use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub const CONFIG_FILE: &str = ".revlens/config.toml";

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub weights: Weights,
    pub patterns: Patterns,
    pub limits: Limits,
    pub server: Server,
    pub intent: Intent,
}

/// Intent document parsing.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Intent {
    /// Heading words (case-insensitive, whole-word) that introduce acceptance criteria lists.
    pub headings: Vec<String>,
    /// For `--pr`, take criteria only from sections under one of `headings`; a PR template
    /// checklist is not acceptance criteria.
    pub pr_require_heading: bool,
}

impl Default for Intent {
    fn default() -> Self {
        Self {
            headings: strings(&[
                "acceptance",
                "criteria",
                "requirements",
                "definition of done",
                "AC",
            ]),
            pr_require_heading: true,
        }
    }
}

/// Points added to a hunk's score per rule.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Weights {
    pub public_sig_changed: u32,
    pub test_removed: u32,
    pub new_dependency: u32,
    pub sensitive_area: u32,
    pub reuse_miss: u32,
    pub suppression_added: u32,
    pub error_handling_weakened: u32,
    pub infra_config: u32,
    pub no_test_touch: u32,
    /// Size adds `min(size_cap, log2(added + removed))`.
    pub size_cap: u32,
}

/// Substring / glob-ish patterns. Matching is case-insensitive substring unless noted.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Patterns {
    /// Matched against file paths and added lines.
    pub sensitive: Vec<String>,
    /// Matched against added lines.
    pub suppressions: Vec<String>,
    /// Path substrings marking CI / build / infra config.
    pub infra_paths: Vec<String>,
    /// Path substrings marking generated files and lockfiles (score forced to 0).
    pub generated_paths: Vec<String>,
    /// Path substrings marking test files.
    pub test_paths: Vec<String>,
    /// Path substrings of files where indentation carries meaning (whitespace-only edits there
    /// are never demoted as formatting).
    pub indent_sensitive: Vec<String>,
    /// Name prefixes (on any dot-separated segment of a qualified name) marking test definitions.
    pub test_symbols: Vec<String>,
    /// Substrings counted per line in test files to detect dropped assertions.
    pub assertions: Vec<String>,
    /// Keys of package.json blocks that list dependencies.
    pub dependency_sections: Vec<String>,
    /// Line-leading keywords of error handling blocks; removing them weakens handling.
    pub error_handling: Vec<String>,
    /// Added handlers that swallow errors (whitespace-insensitive, line-leading).
    pub weak_handlers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Limits {
    /// Minimum lines in a block for the line-based move fallback.
    pub moved_min_block: usize,
    /// Minimum AST nodes for a function to be considered by reuse-miss detection.
    pub reuse_min_nodes: usize,
    /// Minimum AST nodes for a definition to be paired as a rename across unrelated files.
    pub rename_min_nodes: usize,
    /// Scan the base tree for existing duplicates of added functions (`--no-reuse` turns it off).
    pub reuse_enabled: bool,
    /// Files larger than this (bytes) are skipped by reuse-miss scanning.
    pub reuse_max_file_bytes: usize,
    /// Files with average line length above this are treated as minified and skipped.
    pub minified_avg_line: usize,
    /// Rename/copy similarity threshold passed to `git diff -M/-C` (percent).
    pub rename_similarity: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Server {
    /// Address used without `--share`.
    pub bind: String,
    /// Address used with `--share`.
    pub share_bind: String,
    /// 0 picks a free port.
    pub port: u16,
    /// Debounce for persisting the live session document, in milliseconds.
    pub persist_debounce_ms: u64,
    /// Largest accepted WebSocket message, in bytes.
    pub max_ws_message_bytes: usize,
    /// Session document size above which clients are refused, in bytes.
    pub max_doc_bytes: usize,
    /// Concurrent WebSocket connections allowed.
    pub max_ws_connections: usize,
    /// Address a UDP socket is connected to (nothing is sent) to learn which local interface
    /// address leads off this machine; printed as the `--share` URL. Must never be routed.
    pub lan_probe: String,
}

impl Default for Weights {
    fn default() -> Self {
        Self {
            public_sig_changed: 30,
            test_removed: 30,
            new_dependency: 25,
            sensitive_area: 20,
            reuse_miss: 20,
            suppression_added: 15,
            error_handling_weakened: 15,
            infra_config: 15,
            no_test_touch: 10,
            size_cap: 10,
        }
    }
}

fn strings(xs: &[&str]) -> Vec<String> {
    xs.iter().map(|s| s.to_string()).collect()
}

impl Default for Patterns {
    fn default() -> Self {
        Self {
            sensitive: strings(&[
                "auth",
                "token",
                "secret",
                "password",
                "passwd",
                "credential",
                "crypto",
                "sql",
                "migration",
                "eval(",
                "exec(",
                "subprocess",
                "pickle",
                "unsafe ",
                "dangerouslysetinnerhtml",
                "innerhtml",
                "permission",
                "session",
            ]),
            suppressions: strings(&[
                "@ts-ignore",
                "@ts-expect-error",
                "@ts-nocheck",
                "as any",
                "eslint-disable",
                "# type: ignore",
                "# noqa",
                "# pragma: no cover",
                "# pylint: disable",
                "#[allow(",
                "//nolint",
            ]),
            infra_paths: strings(&[
                ".github/workflows/",
                ".gitlab-ci",
                "dockerfile",
                "docker-compose",
                ".env",
                "makefile",
                "terraform",
                ".circleci/",
                "jenkinsfile",
                "helm/",
                "k8s/",
            ]),
            generated_paths: strings(&[
                "package-lock.json",
                "yarn.lock",
                "pnpm-lock.yaml",
                "poetry.lock",
                "uv.lock",
                "pipfile.lock",
                "cargo.lock",
                "go.sum",
                ".pb.go",
                ".min.js",
                ".snap",
                "/dist/",
                "/build/",
                "_pb2.py",
                ".generated.",
                "/vendor/",
            ]),
            test_paths: strings(&[
                "/test_",
                "_test.",
                ".test.",
                ".spec.",
                "/tests/",
                "/test/",
                "/__tests__/",
                "conftest.py",
            ]),
            indent_sensitive: strings(&[".py", ".pyi", ".yml", ".yaml", "makefile", ".mk"]),
            test_symbols: strings(&["test_", "Test"]),
            assertions: strings(&["assert", "expect(", "t.Error", "t.Fatal"]),
            dependency_sections: strings(&[
                "dependencies",
                "devDependencies",
                "peerDependencies",
                "optionalDependencies",
            ]),
            error_handling: strings(&["try", "catch", "except", "if err != nil"]),
            weak_handlers: strings(&[
                "except:",
                "except Exception: pass",
                "catch {}",
                "catch (e) {}",
            ]),
        }
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            moved_min_block: 3,
            reuse_min_nodes: 30,
            reuse_enabled: true,
            rename_min_nodes: 30,
            reuse_max_file_bytes: 500_000,
            minified_avg_line: 300,
            rename_similarity: 50,
        }
    }
}

impl Default for Server {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1".into(),
            share_bind: "0.0.0.0".into(),
            port: 0,
            persist_debounce_ms: 1000,
            max_ws_message_bytes: 1 << 20,
            max_doc_bytes: 8 << 20,
            max_ws_connections: 64,
            lan_probe: "192.0.2.1:9".into(),
        }
    }
}

impl Config {
    /// Load `<repo_root>/.revlens/config.toml` if present, else defaults.
    pub fn load(repo_root: &Path) -> Result<Self> {
        let path = repo_root.join(CONFIG_FILE);
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        Self::parse(&text).with_context(|| format!("parsing {}", path.display()))
    }

    pub fn parse(text: &str) -> Result<Self> {
        Ok(toml::from_str(text)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_is_defaults() {
        assert_eq!(Config::parse("").expect("parse"), Config::default());
    }

    #[test]
    fn partial_override_keeps_other_defaults() {
        let c =
            Config::parse("[weights]\ntest_removed = 99\n[server]\nport = 4100\n").expect("parse");
        assert_eq!(c.weights.test_removed, 99);
        assert_eq!(
            c.weights.public_sig_changed,
            Weights::default().public_sig_changed
        );
        assert_eq!(c.server.port, 4100);
        assert_eq!(c.server.bind, "127.0.0.1");
    }

    #[test]
    fn unknown_key_is_an_error() {
        assert!(Config::parse("[weights]\nnope = 1\n").is_err());
    }

    #[test]
    fn missing_file_is_defaults() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(Config::load(dir.path()).expect("load"), Config::default());
    }
}
