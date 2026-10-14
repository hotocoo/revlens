//! Language detection and definition extraction (functions, classes, ...) via tree-sitter.

use anyhow::{Context, Result, anyhow};
use tree_sitter::{Node, Parser};

use crate::fingerprint;
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
}

pub fn detect(path: &str) -> Option<Lang> {
    let p = path.to_ascii_lowercase();
    let ext = p.rsplit_once('.')?.1;
    match ext {
        "ts" | "mts" | "cts" => Some(Lang::TypeScript),
        "tsx" => Some(Lang::Tsx),
        "py" | "pyi" => Some(Lang::Python),
        _ => None,
    }
}

/// Parse once and collect definitions in source order.
pub fn defs(lang: Lang, src: &str) -> Result<Vec<Def>> {
    let language = match lang {
        Lang::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        Lang::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
        Lang::Python => tree_sitter_python::LANGUAGE.into(),
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
        _ => w.ts_block(root),
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
        });
    }

    // ---- TypeScript / TSX ----

    fn ts_block(&mut self, parent: Node) {
        let mut c = parent.walk();
        for n in parent.children(&mut c) {
            if n.kind() == "export_statement" {
                let decl = n
                    .child_by_field_name("declaration")
                    .or_else(|| n.child_by_field_name("value"));
                if let Some(d) = decl {
                    self.ts_decl(n, d, true);
                }
            } else {
                self.ts_decl(n, n, false);
            }
        }
    }

    fn ts_decl(&mut self, span: Node, d: Node, exported: bool) {
        match d.kind() {
            "function_declaration"
            | "generator_function_declaration"
            | "function_expression"
            | "function" => {
                let name = self.field_text(d, "name");
                let sig = self.ts_fn_sig(d);
                let t = is_ts_test(&name);
                self.push(span, d, name, None, SymKind::Function, exported, sig, t);
            }
            "class_declaration" | "abstract_class_declaration" | "class" => {
                self.ts_class(span, d, exported)
            }
            "interface_declaration" => {
                let name = self.field_text(d, "name");
                let ext = find_kind(d, "extends_type_clause");
                let sig = self.sig(&[
                    d.child_by_field_name("type_parameters"),
                    ext,
                    d.child_by_field_name("body"),
                ]);
                let t = is_ts_test(&name);
                self.push(span, d, name, None, SymKind::Interface, exported, sig, t);
            }
            "type_alias_declaration" => {
                let name = self.field_text(d, "name");
                let sig = self.sig(&[
                    d.child_by_field_name("type_parameters"),
                    d.child_by_field_name("value"),
                ]);
                let t = is_ts_test(&name);
                self.push(span, d, name, None, SymKind::TypeAlias, exported, sig, t);
            }
            "lexical_declaration" => {
                let mut c = d.walk();
                for v in d
                    .children(&mut c)
                    .filter(|v| v.kind() == "variable_declarator")
                {
                    let Some(val) = v.child_by_field_name("value") else {
                        continue;
                    };
                    if !matches!(
                        val.kind(),
                        "arrow_function" | "function_expression" | "function"
                    ) {
                        continue;
                    }
                    let name = self.field_text(v, "name");
                    let sig = self.ts_fn_sig(val);
                    let t = is_ts_test(&name);
                    self.push(span, v, name, None, SymKind::Function, exported, sig, t);
                }
            }
            _ => {}
        }
    }

    fn ts_fn_sig(&self, f: Node) -> String {
        self.sig_with(
            &[
                f.child_by_field_name("type_parameters"),
                f.child_by_field_name("parameters")
                    .or_else(|| f.child_by_field_name("parameter")),
                f.child_by_field_name("return_type"),
            ],
            "",
        )
    }

    fn ts_class(&mut self, span: Node, d: Node, exported: bool) {
        let name = self.field_text(d, "name");
        let sig = self.sig(&[
            d.child_by_field_name("type_parameters"),
            find_kind(d, "class_heritage"),
        ]);
        let t = is_ts_test(&name);
        self.push(
            span,
            d,
            name.clone(),
            None,
            SymKind::Class,
            exported,
            sig,
            t,
        );
        let Some(body) = d.child_by_field_name("body") else {
            return;
        };
        let owner = Owner {
            name,
            public: exported,
            is_test: false,
        };
        let mut c = body.walk();
        for m in body
            .children(&mut c)
            .filter(|m| m.kind() == "method_definition")
        {
            let mname = self.field_text(m, "name");
            let hidden = m
                .child_by_field_name("name")
                .is_some_and(|n| n.kind() == "private_property_identifier")
                || find_kind(m, "accessibility_modifier")
                    .is_some_and(|a| matches!(self.text(a), "private" | "protected"));
            let sig = self.ts_fn_sig(m);
            let t = is_ts_test(&mname);
            self.push(
                m,
                m,
                mname,
                Some(&owner),
                SymKind::Method,
                owner.public && !hidden,
                sig,
                t,
            );
        }
    }

    // ---- Python ----

    fn py_block(&mut self, parent: Node, owner: Option<&Owner>) {
        let mut c = parent.walk();
        for n in parent.children(&mut c) {
            let (span, d) = if n.kind() == "decorated_definition" {
                match n.child_by_field_name("definition") {
                    Some(d) => (n, d),
                    None => continue,
                }
            } else {
                (n, n)
            };
            match d.kind() {
                "function_definition" => self.py_fn(span, d, owner),
                "class_definition" => self.py_class(span, d, owner),
                _ => {}
            }
        }
    }

    fn py_fn(&mut self, span: Node, d: Node, owner: Option<&Owner>) {
        let name = self.field_text(d, "name");
        let ret = d
            .child_by_field_name("return_type")
            .map(|r| format!("-> {}", self.text(r)));
        let params = self.sig(&[
            d.child_by_field_name("type_parameters"),
            d.child_by_field_name("parameters"),
        ]);
        let sig = [params, ret.unwrap_or_default()]
            .join(" ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let public = py_public(&name) && owner.is_none_or(|o| o.public);
        let t = name.starts_with("test_") || owner.is_some_and(|o| o.is_test);
        let kind = if owner.is_some() {
            SymKind::Method
        } else {
            SymKind::Function
        };
        self.push(span, d, name, owner, kind, public, sig, t);
    }

    fn py_class(&mut self, span: Node, d: Node, outer: Option<&Owner>) {
        let name = self.field_text(d, "name");
        let sig = self.sig(&[d.child_by_field_name("superclasses")]);
        let public = py_public(&name) && outer.is_none_or(|o| o.public);
        let is_test = name.starts_with("Test");
        let qualified = outer.map_or_else(|| name.clone(), |o| format!("{}.{}", o.name, name));
        self.push(span, d, name, outer, SymKind::Class, public, sig, is_test);
        if let Some(body) = d.child_by_field_name("body") {
            let o = Owner {
                name: qualified,
                public,
                is_test,
            };
            self.py_block(body, Some(&o));
        }
    }
}

fn is_ts_test(name: &str) -> bool {
    name.starts_with("test")
}

fn py_public(name: &str) -> bool {
    !name.starts_with('_') || (name.starts_with("__") && name.ends_with("__") && name.len() > 4)
}

fn find_kind<'t>(n: Node<'t>, kind: &str) -> Option<Node<'t>> {
    let mut c = n.walk();
    n.children(&mut c).find(|ch| ch.kind() == kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(d: &[Def]) -> Vec<(&str, SymKind, bool)> {
        d.iter()
            .map(|d| (d.qualified.as_str(), d.kind, d.public))
            .collect()
    }

    #[test]
    fn detects_languages() {
        assert_eq!(detect("a/b.ts"), Some(Lang::TypeScript));
        assert_eq!(detect("a/b.d.ts"), Some(Lang::TypeScript));
        assert_eq!(detect("x.mts"), Some(Lang::TypeScript));
        assert_eq!(detect("x.cts"), Some(Lang::TypeScript));
        assert_eq!(detect("x.TSX"), Some(Lang::Tsx));
        assert_eq!(detect("x.py"), Some(Lang::Python));
        assert_eq!(detect("x.pyi"), Some(Lang::Python));
        assert_eq!(detect("x.rs"), None);
        assert_eq!(detect("Makefile"), None);
    }

    const TS: &str = r#"
import { a } from "./a";
export function add(a: number, b: number): number {
  function inner() { return 1; }
  return a + b;
}
function* gen<T>(x: T) { yield x; }
export default class Svc<T> extends Base implements I {
  private secret(): void {}
  #hidden = 1;
  #priv() {}
  protected prot() {}
  async run(x: string): Promise<void> { await x; }
  testThing() {}
}
class Local { m() {} }
export interface Shape extends Base { area(): number }
export type Id = string | number;
export const mul = (a: number, b: number): number => a * b;
const fn2 = function (x) { return x; };
const notFn = 5;
const test_helper = () => {};
/** doc */
export abstract class Abs { abstract f(): void; g() {} }
"#;

    #[test]
    fn typescript_defs() {
        let d = defs(Lang::TypeScript, TS).expect("defs");
        use SymKind::*;
        assert_eq!(
            names(&d),
            vec![
                ("add", Function, true),
                ("gen", Function, false),
                ("Svc", Class, true),
                ("Svc.secret", Method, false),
                ("Svc.#priv", Method, false),
                ("Svc.prot", Method, false),
                ("Svc.run", Method, true),
                ("Svc.testThing", Method, true),
                ("Local", Class, false),
                ("Local.m", Method, false),
                ("Shape", Interface, true),
                ("Id", TypeAlias, true),
                ("mul", Function, true),
                ("fn2", Function, false),
                ("test_helper", Function, false),
                ("Abs", Class, true),
                ("Abs.g", Method, true),
            ]
        );
        let add = &d[0];
        assert_eq!(add.signature, "(a: number, b: number): number");
        assert_eq!((add.start_line, add.end_line), (3, 6));
        assert!(!add.is_test && d[7].is_test);
        assert_eq!(d[2].signature, "<T> extends Base implements I");
        assert_eq!(d[10].signature, "extends Base { area(): number }");
        assert_eq!(d[11].signature, "string | number");
        assert!(d.iter().all(|x| x.nodes > 0));
    }

    #[test]
    fn rename_keeps_shape_not_exact() {
        let a = defs(Lang::TypeScript, "function f(a: number) { return a + 1; }").expect("a");
        let b = defs(
            Lang::TypeScript,
            "function g(z: number) { return z + 1; } // c",
        )
        .expect("b");
        assert_eq!(a[0].shape, b[0].shape);
        assert_ne!(a[0].exact, b[0].exact);
    }

    #[test]
    fn tsx_defs() {
        let src = "export const App = (p: { n: string }) => <div>{p.n} é</div>;
export function B() { return <b/>; }
";
        let d = defs(Lang::Tsx, src).expect("tsx");
        assert_eq!(
            d.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(),
            ["App", "B"]
        );
        assert!(d.iter().all(|d| d.public));
    }

    const PY: &str = r#"
import os

def top(a: int, b=2) -> str:
    def nested():
        pass
    return "é"

def _private():
    pass

@decorator
@other(1)
def decorated():
    pass

class Svc(Base):
    def __init__(self):
        pass
    def _hidden(self):
        pass
    @staticmethod
    def make(x):
        pass
    class Inner:
        def m(self): pass

class _Priv:
    def pub(self): pass

class TestThing:
    def test_a(self): pass
    def helper(self): pass

async def fetch(url): pass
def test_top(): pass
"#;

    #[test]
    fn python_defs() {
        let d = defs(Lang::Python, PY).expect("py");
        use SymKind::*;
        assert_eq!(
            names(&d),
            vec![
                ("top", Function, true),
                ("_private", Function, false),
                ("decorated", Function, true),
                ("Svc", Class, true),
                ("Svc.__init__", Method, true),
                ("Svc._hidden", Method, false),
                ("Svc.make", Method, true),
                ("Svc.Inner", Class, true),
                ("Svc.Inner.m", Method, true),
                ("_Priv", Class, false),
                ("_Priv.pub", Method, false),
                ("TestThing", Class, true),
                ("TestThing.test_a", Method, true),
                ("TestThing.helper", Method, true),
                ("fetch", Function, true),
                ("test_top", Function, true),
            ]
        );
        assert_eq!(d[0].signature, "(a: int, b=2) -> str");
        // decorators included in span
        assert_eq!((d[2].start_line, d[2].end_line), (12, 15));
        assert_eq!(d[3].signature, "(Base)");
        let t: Vec<_> = d
            .iter()
            .filter(|d| d.is_test)
            .map(|d| d.qualified.as_str())
            .collect();
        assert_eq!(
            t,
            [
                "TestThing",
                "TestThing.test_a",
                "TestThing.helper",
                "test_top"
            ]
        );
    }

    #[test]
    fn syntax_errors_tolerated() {
        assert!(
            defs(
                Lang::Python,
                "def (:
"
            )
            .is_ok()
        );
        assert!(defs(Lang::TypeScript, "function {").is_ok());
        assert!(defs(Lang::Python, "").expect("empty").is_empty());
    }
}
