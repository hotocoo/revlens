//! Language detection and definition extraction (functions, classes, ...) via tree-sitter.

use anyhow::{Context, Result, anyhow};
use tree_sitter::{Node, Parser};

use crate::fingerprint;

mod go;
mod py;
mod rust;
#[cfg(test)]
mod tests;
mod ts;
use crate::model::{Lang, SymKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Def {
    pub name: String,
    /// `"Class.method"` or `"fn"`.
    pub qualified: String,
    pub kind: SymKind,
    pub public: bool,
    /// 1-based inclusive.
    pub start_line: u32,
    pub end_line: u32,
    /// Type params, params and return type (classes: heritage; interfaces: heritage + body;
    /// type aliases: the aliased type), whitespace-normalized.
    pub signature: String,
    pub exact: u64,
    pub shape: u64,
    /// `exact` with the definition's own name masked: equal means identical up to a rename.
    pub unnamed: u64,
    pub nodes: usize,
    pub is_test: bool,
    /// [`fingerprint::shingles`] of functions and methods (empty for other kinds), for
    /// near-duplicate detection.
    pub shingles: Vec<u64>,
}

pub fn detect(path: &str) -> Option<Lang> {
    let p = path.to_ascii_lowercase();
    let ext = p.rsplit_once('.')?.1;
    match ext {
        "ts" | "mts" | "cts" => Some(Lang::TypeScript),
        "tsx" => Some(Lang::Tsx),
        "py" | "pyi" => Some(Lang::Python),
        "rs" => Some(Lang::Rust),
        "go" => Some(Lang::Go),
        _ => None,
    }
}

/// Parse once and collect definitions in source order.
pub fn defs(lang: Lang, src: &str) -> Result<Vec<Def>> {
    let language = match lang {
        Lang::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        Lang::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
        Lang::Python => tree_sitter_python::LANGUAGE.into(),
        Lang::Rust => tree_sitter_rust::LANGUAGE.into(),
        Lang::Go => tree_sitter_go::LANGUAGE.into(),
    };
    let mut parser = Parser::new();
    parser.set_language(&language).context("loading grammar")?;
    let tree = parser
        .parse(src, None)
        .ok_or_else(|| anyhow!("parse failed"))?;
    let mut w = Walker {
        src: src.as_bytes(),
        out: Vec::new(),
    };
    let root = tree.root_node();
    match lang {
        Lang::Python => w.py_block(root, None),
        Lang::Rust => w.rs_block(root, None),
        Lang::Go => w.go_block(root),
        Lang::TypeScript | Lang::Tsx => w.ts_block(root),
    }
    Ok(w.out)
}

struct Walker<'a> {
    src: &'a [u8],
    out: Vec<Def>,
}

/// Enclosing class context: name used for qualification, and whether members can be public.
#[derive(Clone)]
struct Owner {
    name: String,
    public: bool,
    is_test: bool,
    /// A type (class, impl, trait) rather than a namespace (Rust `mod`): functions are methods.
    is_type: bool,
    /// Members are public without their own marker (Rust trait items and trait impls).
    members_public: bool,
}

impl<'a> Walker<'a> {
    fn text(&self, n: Node) -> &'a str {
        n.utf8_text(self.src).unwrap_or_default()
    }

    fn field_text(&self, n: Node, f: &str) -> String {
        n.child_by_field_name(f)
            .map(|c| self.text(c).to_string())
            .unwrap_or_default()
    }

    /// Normalized concatenation of the given nodes' text.
    fn sig(&self, parts: &[Option<Node>]) -> String {
        self.sig_with(parts, " ")
    }

    fn sig_with(&self, parts: &[Option<Node>], sep: &str) -> String {
        let joined: Vec<&str> = parts.iter().flatten().map(|n| self.text(*n)).collect();
        joined
            .join(sep)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[allow(clippy::too_many_arguments)]
    fn push(
        &mut self,
        span: Node,
        body: Node,
        name: String,
        owner: Option<&Owner>,
        kind: SymKind,
        public: bool,
        signature: String,
        is_test: bool,
    ) {
        if name.is_empty() {
            return;
        }
        let (shape, nodes) = fingerprint::shape(body, self.src);
        let unnamed = fingerprint::exact_unnamed(body, self.src, &name);
        let start = span.start_position();
        let end = span.end_position();
        let end_row = if end.column == 0 && end.row > start.row {
            end.row
        } else {
            end.row + 1
        };
        self.out.push(Def {
            qualified: owner.map_or_else(|| name.clone(), |o| format!("{}.{}", o.name, name)),
            name,
            kind,
            public,
            start_line: u32::try_from(start.row + 1).unwrap_or(u32::MAX),
            end_line: u32::try_from(end_row).unwrap_or(u32::MAX),
            signature,
            exact: fingerprint::exact(body, self.src),
            shape,
            unnamed,
            nodes,
            is_test,
            shingles: if matches!(kind, SymKind::Function | SymKind::Method) {
                fingerprint::shingles(body, self.src)
            } else {
                Vec::new()
            },
        });
    }
}

fn find_kind<'t>(n: Node<'t>, kind: &str) -> Option<Node<'t>> {
    let mut c = n.walk();
    n.children(&mut c).find(|ch| ch.kind() == kind)
}
