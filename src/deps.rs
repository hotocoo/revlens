//! Dependency names added by a change to a manifest (package.json, pyproject.toml, Pipfile,
//! requirements*.txt).
//!
//! `scan` parses the manifest before and after the change and diffs the dependency sets, which is
//! exact. `added` is the fallback for a manifest that cannot be parsed: a hunk only carries three
//! lines of context, so the enclosing section is tracked from the lines the hunk shows and, when
//! its header is out of view, inferred from the shape of the line.

use std::collections::{BTreeMap, HashMap};

use anyhow::{Context, Result, anyhow};

use crate::config::Config;
use crate::git::Repo;
use crate::model::{FileDiff, FileStatus, Hunk, HunkId, Line, LineKind};

/// Result of reading one manifest.
#[derive(Debug, Clone, PartialEq)]
pub enum Scan {
    /// Names added by the change, assigned to the hunk that shows them.
    Parsed(HashMap<HunkId, Vec<String>>),
    /// A blob could not be read or parsed; callers fall back to the line heuristic.
    Unparsed,
}

/// Manifest path (as in `Hunk::path`) -> scan result.
pub type Scans = HashMap<String, Scan>;

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    PackageJson,
    Pyproject,
    Pipfile,
    Requirements,
}

fn kind_of(path: &str) -> Option<Kind> {
    let base = path.rsplit('/').next().unwrap_or(path).to_lowercase();
    match base.as_str() {
        "package.json" => Some(Kind::PackageJson),
        "pyproject.toml" => Some(Kind::Pyproject),
        "pipfile" => Some(Kind::Pipfile),
        _ if is_requirements(path) => Some(Kind::Requirements),
        _ => None,
    }
}

/// Parse every changed manifest at `merge_base` and `head` and attach each added dependency name
/// to the hunk whose added lines mention it (else the file's first hunk).
pub fn scan(
    repo: &Repo,
    merge_base: &str,
    head: &str,
    files: &[FileDiff],
    hunks: &[Hunk],
    cfg: &Config,
) -> Result<Scans> {
    let manifests: Vec<(&FileDiff, Kind)> = files
        .iter()
        .filter(|f| !f.generated && f.status != FileStatus::Deleted)
        .filter_map(|f| Some((f, kind_of(f.new_path.as_deref()?)?)))
        .collect();
    let paths = |side: fn(&FileDiff) -> Option<&str>| -> Vec<String> {
        manifests
            .iter()
            .filter_map(|(f, _)| side(f).map(str::to_string))
            .collect()
    };
    let new_blobs: HashMap<_, _> = repo
        .read_blobs(head, &paths(|f| f.new_path.as_deref()))?
        .into_iter()
        .collect();
    let old_blobs: HashMap<_, _> = repo
        .read_blobs(merge_base, &paths(|f| f.old_path.as_deref()))?
        .into_iter()
        .collect();
    let mut out = Scans::new();
    for (f, kind) in manifests {
        let path = f.path();
        let blob =
            |m: &HashMap<String, Option<String>>, p: &Option<String>| -> Option<Option<String>> {
                match p {
                    None => Some(None),
                    Some(p) => m.get(p).cloned().flatten().map(Some),
                }
            };
        let (Some(new), Some(old)) = (blob(&new_blobs, &f.new_path), blob(&old_blobs, &f.old_path))
        else {
            out.insert(path.to_string(), Scan::Unparsed);
            continue;
        };
        let sections = &cfg.patterns.dependency_sections;
        let names = |text: Option<String>| match text {
            None => Ok(BTreeMap::new()),
            Some(t) => parse(kind, &t, sections),
        };
        let scan = match (names(old), names(new)) {
            (Ok(old), Ok(new)) => {
                let added: Vec<String> = new
                    .into_iter()
                    .filter(|(k, _)| !old.contains_key(k))
                    .map(|(_, shown)| shown)
                    .collect();
                Scan::Parsed(assign(path, &added, hunks))
            }
            (Err(e), _) | (_, Err(e)) => {
                eprintln!("warning: parsing {path}: {e:#}; using line heuristic");
                Scan::Unparsed
            }
        };
        out.insert(path.to_string(), scan);
    }
    Ok(out)
}

/// Each name goes to the first hunk of `path` whose added lines mention it, else the first hunk.
fn assign(path: &str, names: &[String], hunks: &[Hunk]) -> HashMap<HunkId, Vec<String>> {
    let in_file: Vec<&Hunk> = hunks.iter().filter(|h| h.path == path).collect();
    let mut out: HashMap<HunkId, Vec<String>> = HashMap::new();
    for name in names {
        let mentions = |h: &&&Hunk| {
            h.lines
                .iter()
                .any(|l| l.kind == LineKind::Added && l.text.contains(name.as_str()))
        };
        if let Some(h) = in_file.iter().find(mentions).or(in_file.first()) {
            out.entry(h.id.clone()).or_default().push(name.clone());
        }
    }
    out
}

/// Normalized name -> name as written.
type Names = BTreeMap<String, String>;

fn parse(kind: Kind, text: &str, sections: &[String]) -> Result<Names> {
    let raw = match kind {
        Kind::PackageJson => package_json_names(text, sections)?,
        Kind::Pyproject => pyproject_names(text)?,
        Kind::Pipfile => pipfile_names(text)?,
        Kind::Requirements => requirements_names(text),
    };
    let npm = kind == Kind::PackageJson;
    Ok(raw
        .into_iter()
        .map(|n| (if npm { n.clone() } else { normalize_py(&n) }, n))
        .collect())
}

/// PEP 503: case-insensitive, runs of `-_.` equal.
fn normalize_py(name: &str) -> String {
    let mut out = String::new();
    for c in name.to_lowercase().chars() {
        let c = if "-_.".contains(c) { '-' } else { c };
        if c != '-' || !out.ends_with('-') {
            out.push(c);
        }
    }
    out
}

fn package_json_names(text: &str, sections: &[String]) -> Result<Vec<String>> {
    let v: serde_json::Value = serde_json::from_str(text).context("invalid JSON")?;
    let obj = v.as_object().ok_or_else(|| anyhow!("not a JSON object"))?;
    Ok(sections
        .iter()
        .filter_map(|s| obj.get(s)?.as_object())
        .flat_map(|m| m.keys().cloned())
        .collect())
}

fn requirements_names(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| l.split(" #").next().unwrap_or(l).trim())
        .filter(|t| !t.is_empty() && !t.starts_with('#') && !t.starts_with('-'))
        .map(|t| req_name(t).map_or_else(|| t.to_string(), |(n, _)| n))
        .collect()
}

/// PEP 508 strings of a TOML array; non-string entries (`{include-group = ...}`) are skipped.
fn req_strings(v: Option<&toml::Value>) -> Vec<String> {
    v.and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|e| req_name(e.as_str()?).map(|(n, _)| n))
        .collect()
}

fn keys(v: Option<&toml::Value>) -> Vec<String> {
    v.and_then(toml::Value::as_table)
        .into_iter()
        .flat_map(|t| t.keys().cloned())
        .collect()
}

fn pyproject_names(text: &str) -> Result<Vec<String>> {
    let doc: toml::Table = toml::from_str(text).context("invalid TOML")?;
    let mut out = Vec::new();
    let project = doc.get("project");
    out.extend(req_strings(project.and_then(|p| p.get("dependencies"))));
    let groups = |v: Option<&toml::Value>| -> Vec<String> {
        v.and_then(toml::Value::as_table)
            .into_iter()
            .flat_map(|t| t.values())
            .flat_map(|g| req_strings(Some(g)))
            .collect()
    };
    out.extend(groups(project.and_then(|p| p.get("optional-dependencies"))));
    out.extend(groups(doc.get("dependency-groups")));
    let poetry = doc.get("tool").and_then(|t| t.get("poetry"));
    let poetry_keys = |v: Option<&toml::Value>| keys(v).into_iter().filter(|k| k != "python");
    out.extend(poetry_keys(poetry.and_then(|p| p.get("dependencies"))));
    out.extend(poetry_keys(poetry.and_then(|p| p.get("dev-dependencies"))));
    for g in poetry
        .and_then(|p| p.get("group"))
        .and_then(toml::Value::as_table)
        .into_iter()
        .flat_map(|t| t.values())
    {
        out.extend(poetry_keys(g.get("dependencies")));
    }
    Ok(out)
}

fn pipfile_names(text: &str) -> Result<Vec<String>> {
    let doc: toml::Table = toml::from_str(text).context("invalid TOML")?;
    Ok(["packages", "dev-packages"]
        .iter()
        .flat_map(|s| keys(doc.get(*s)))
        .collect())
}

/// Names newly added by `lines`; a name that is also removed (a version bump) is not new.
pub fn added(path: &str, lines: &[Line], sections: &[String]) -> Vec<String> {
    let base = path.rsplit('/').next().unwrap_or(path).to_lowercase();
    let seen = match base.as_str() {
        "package.json" => package_json(lines, sections),
        "pyproject.toml" => pyproject(lines),
        _ if is_requirements(path) => requirements(lines),
        _ => Vec::new(),
    };
    let mut out: Vec<String> = Vec::new();
    for (kind, name) in &seen {
        let bumped = seen
            .iter()
            .any(|(k, n)| *k == LineKind::Removed && n == name);
        if *kind == LineKind::Added && !bumped && !out.contains(name) {
            out.push(name.clone());
        }
    }
    out
}

/// `requirements*.txt`, or any `.txt` under a `requirements*` directory.
fn is_requirements(path: &str) -> bool {
    let p = path.to_lowercase();
    p.ends_with(".txt") && p.split('/').any(|c| c.starts_with("requirements"))
}

type Seen = Vec<(LineKind, String)>;

fn requirements(lines: &[Line]) -> Seen {
    lines
        .iter()
        .filter(|l| l.kind != LineKind::Context)
        .filter_map(|l| {
            let t = l.text.trim();
            if t.is_empty() || t.starts_with('#') {
                return None;
            }
            let name = if t.starts_with('-') {
                t.to_string()
            } else {
                req_name(t).map_or_else(|| t.to_string(), |(n, _)| n)
            };
            Some((l.kind, name))
        })
        .collect()
}

const REQ_OPS: &str = "<>=!~[;@ (";

/// `"requests[socks]>=2"` -> (`requests`, `[socks]>=2`).
fn req_name(s: &str) -> Option<(String, &str)> {
    let end = s
        .find(|c: char| !(c.is_ascii_alphanumeric() || "-_.".contains(c)))
        .unwrap_or(s.len());
    let (name, rest) = s.split_at(end);
    let rest = rest.trim_start();
    let valid = name.starts_with(|c: char| c.is_ascii_alphabetic())
        && (rest.is_empty() || REQ_OPS.contains(rest.chars().next().unwrap_or(' ')));
    valid.then(|| (name.to_string(), rest))
}

/// `"key": "value"` or `key = "value"` -> (key, value).
fn key_value(t: &str) -> Option<(&str, &str)> {
    let (k, v) = t.split_once(':').or_else(|| t.split_once('='))?;
    let k = k.trim().trim_matches('"');
    let v = v.trim().trim_end_matches(',').trim();
    let v = v.strip_prefix('"')?.strip_suffix('"')?;
    (!k.is_empty() && !k.contains(' ')).then_some((k, v))
}

const SPEC_PREFIXES: [&str; 7] = [
    "workspace:",
    "npm:",
    "file:",
    "git",
    "http",
    "github:",
    "link:",
];

fn looks_like_spec(v: &str) -> bool {
    v.starts_with(|c: char| c.is_ascii_digit() || "^~<>=*".contains(c))
        || SPEC_PREFIXES.iter().any(|p| v.starts_with(p))
}

fn package_json(lines: &[Line], sections: &[String]) -> Seen {
    let mut in_dep: Option<bool> = None;
    let mut out = Vec::new();
    for l in lines {
        let t = l.text.trim();
        if l.kind != LineKind::Removed
            && let Some(key) = t.strip_suffix('{').and_then(|k| k.trim().strip_suffix(':'))
        {
            in_dep = Some(sections.iter().any(|s| s == key.trim().trim_matches('"')));
            continue;
        }
        if t.starts_with('}') {
            if l.kind != LineKind::Removed {
                in_dep = Some(false);
            }
            continue;
        }
        let Some((key, value)) = key_value(t) else {
            continue;
        };
        let is_dep = match in_dep {
            Some(b) => b,
            None => key != "version" && looks_like_spec(value),
        };
        if is_dep && l.kind != LineKind::Context {
            out.push((l.kind, key.to_string()));
        }
    }
    out
}

#[derive(PartialEq)]
enum Table {
    Unknown,
    Project,
    OptionalDeps,
    PoetryDeps,
    Other,
}

fn table_of(header: &str) -> Table {
    match header {
        "project" => Table::Project,
        "tool.poetry.dependencies" => Table::PoetryDeps,
        h if h.starts_with("project.optional-dependencies") || h == "dependency-groups" => {
            Table::OptionalDeps
        }
        h if h.starts_with("tool.poetry.group.") && h.ends_with(".dependencies") => {
            Table::PoetryDeps
        }
        _ => Table::Other,
    }
}

fn quoted(t: &str) -> impl Iterator<Item = &str> {
    t.split('"').skip(1).step_by(2)
}

fn pyproject(lines: &[Line]) -> Seen {
    let mut table = Table::Unknown;
    let mut in_list = false;
    let mut out = Vec::new();
    for l in lines {
        let t = l.text.trim();
        let tracked = l.kind != LineKind::Removed;
        if tracked && t.starts_with('[') && t.ends_with(']') && !t.starts_with("[\"") {
            table = table_of(t.trim_matches(|c| c == '[' || c == ']').trim());
            in_list = false;
            continue;
        }
        let starts_list = (table == Table::Project && t.starts_with("dependencies"))
            || (table == Table::OptionalDeps && t.contains("= ["))
            || (table == Table::Unknown && t.starts_with("dependencies") && t.contains('['));
        if tracked && starts_list {
            in_list = true;
        }
        let in_array = in_list || starts_list;
        let names: Vec<String> = match table {
            Table::PoetryDeps => key_value(t)
                .filter(|(k, _)| *k != "python")
                .map(|(k, _)| k.to_string())
                .into_iter()
                .collect(),
            _ if in_array => quoted(t)
                .filter_map(|s| req_name(s).map(|(n, _)| n))
                .collect(),
            Table::Unknown => unknown_toml(t),
            _ => Vec::new(),
        };
        if tracked && in_array && t.contains(']') {
            in_list = false;
        }
        if l.kind != LineKind::Context {
            out.extend(names.into_iter().map(|n| (l.kind, n)));
        }
    }
    out
}

/// Header out of view: accept a quoted requirement with a version operator, or `name = "spec"`.
fn unknown_toml(t: &str) -> Vec<String> {
    let bare = t.trim_end_matches(',').trim();
    if let Some(inner) = bare.strip_prefix('"').and_then(|s| s.strip_suffix('"'))
        && let Some((name, rest)) = req_name(inner)
        && !rest.is_empty()
    {
        return vec![name];
    }
    key_value(t)
        .filter(|(k, v)| *k != "version" && looks_like_spec(v))
        .map(|(k, _)| k.to_string())
        .into_iter()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(spec: &[&str]) -> Vec<Line> {
        spec.iter()
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
            .collect()
    }

    fn secs() -> Vec<String> {
        crate::config::Config::default()
            .patterns
            .dependency_sections
    }

    fn names(path: &str, spec: &[&str]) -> Vec<String> {
        added(path, &lines(spec), &secs())
    }

    #[test]
    fn package_json_added_in_block() {
        let n = names(
            "package.json",
            &[
                r#"   "dependencies": {"#,
                r#"+    "left-pad": "^1.3.0","#,
                r#"     "zod": "^3.0.0""#,
            ],
        );
        assert_eq!(n, ["left-pad"]);
    }

    #[test]
    fn package_json_outside_block_ignored() {
        let n = names(
            "package.json",
            &[r#"   "scripts": {"#, r#"+    "test": "jest","#, r#"   }"#],
        );
        assert!(n.is_empty());
    }

    #[test]
    fn package_json_block_closes_inside_hunk() {
        let n = names(
            "package.json",
            &[
                r#"   "dependencies": {"#,
                r#"   },"#,
                r#"   "scripts": {"#,
                r#"+    "lint": "eslint .""#,
            ],
        );
        assert!(n.is_empty());
    }

    #[test]
    fn package_json_unknown_state_uses_value_shape() {
        let n = names(
            "package.json",
            &[
                r#"+    "left-pad": "^1.3.0","#,
                r#"+    "version": "1.2.3","#,
                r#"+    "test": "jest","#,
            ],
        );
        assert_eq!(n, ["left-pad"]);
    }

    #[test]
    fn version_bump_is_not_new() {
        let n = names(
            "package.json",
            &[
                r#"   "devDependencies": {"#,
                r#"-    "zod": "^3.0.0","#,
                r#"+    "zod": "^3.1.0","#,
            ],
        );
        assert!(n.is_empty());
        assert!(names("requirements.txt", &["-flask==1", "+flask==2"]).is_empty());
    }

    #[test]
    fn requirements_any_added_noncomment_line() {
        let n = names(
            "sub/requirements-dev.txt",
            &["+# comment", "+", "+httpx>=0.27", "+-r base.txt", " pytest"],
        );
        assert_eq!(n, ["httpx", "-r base.txt"]);
    }

    #[test]
    fn pyproject_project_list_and_poetry() {
        let n = names(
            "pyproject.toml",
            &[
                " [project]",
                " dependencies = [",
                r#"+  "httpx>=0.27","#,
                " ]",
                " [tool.poetry.dependencies]",
                "+rich = \"^13\"",
                "+python = \"^3.11\"",
                " [tool.other]",
                "+x = \"^1\"",
            ],
        );
        assert_eq!(n, ["httpx", "rich"]);
    }

    #[test]
    fn pyproject_project_table_ignores_other_strings() {
        let n = names(
            "pyproject.toml",
            &[" [project]", r#"+license = "MIT""#, r#"+name = "x""#],
        );
        assert!(n.is_empty());
    }

    #[test]
    fn pyproject_unknown_state_and_optional_deps() {
        assert_eq!(
            names("pyproject.toml", &[r#"+  "httpx>=0.27","#, r#"+  "MIT","#]),
            ["httpx"]
        );
        let n = names(
            "pyproject.toml",
            &[
                " [project.optional-dependencies]",
                " dev = [",
                r#"+  "pytest","#,
                " ]",
            ],
        );
        assert_eq!(n, ["pytest"]);
    }

    #[test]
    fn other_files_have_no_dependencies() {
        assert!(names("src/a.py", &["+requests>=2"]).is_empty());
    }
}
