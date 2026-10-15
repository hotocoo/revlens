//! Go definitions: functions, methods (qualified by receiver type) and type declarations.
//! Exported means the name starts with an upper-case letter.

use tree_sitter::Node;

use super::{Owner, Walker};
use crate::model::SymKind;

impl Walker<'_> {
    pub(super) fn go_block(&mut self, root: Node) {
        let mut c = root.walk();
        for d in root.children(&mut c) {
            match d.kind() {
                "function_declaration" => {
                    let name = self.field_text(d, "name");
                    let sig = self.go_fn_sig(d);
                    let public = go_exported(&name);
                    let t = is_go_test(&name);
                    self.push(d, d, name, None, SymKind::Function, public, sig, t);
                }
                "method_declaration" => {
                    let name = self.field_text(d, "name");
                    let recv = d
                        .child_by_field_name("receiver")
                        .map(|r| receiver_type(self.text(r)))
                        .unwrap_or_default();
                    let sig = self.go_fn_sig(d);
                    let public = go_exported(&recv) && go_exported(&name);
                    let o = Owner {
                        name: recv,
                        public,
                        is_test: false,
                        is_type: true,
                        members_public: false,
                    };
                    let t = is_go_test(&name);
                    self.push(d, d, name, Some(&o), SymKind::Method, public, sig, t);
                }
                "type_declaration" => {
                    let mut tc = d.walk();
                    for spec in d.children(&mut tc) {
                        if matches!(spec.kind(), "type_spec" | "type_alias") {
                            self.go_type(d, spec);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn go_type(&mut self, decl: Node, spec: Node) {
        let name = self.field_text(spec, "name");
        let ty = spec.child_by_field_name("type");
        let kind = match (spec.kind(), ty.map(|t| t.kind())) {
            ("type_alias", _) => SymKind::TypeAlias,
            (_, Some("struct_type")) => SymKind::Class,
            (_, Some("interface_type")) => SymKind::Interface,
            _ => SymKind::TypeAlias,
        };
        let sig = self.sig(&[spec.child_by_field_name("type_parameters"), ty]);
        // A grouped `type ( ... )` spans many specs; each spec is its own definition.
        let span = if decl.named_child_count() > 1 {
            spec
        } else {
            decl
        };
        let public = go_exported(&name);
        self.push(span, spec, name, None, kind, public, sig, false);
    }

    fn go_fn_sig(&self, f: Node) -> String {
        self.sig(&[
            f.child_by_field_name("type_parameters"),
            f.child_by_field_name("parameters"),
            f.child_by_field_name("result"),
        ])
    }
}

fn go_exported(name: &str) -> bool {
    name.chars().next().is_some_and(char::is_uppercase)
}

/// `Test*`, `Benchmark*`, `Fuzz*` and `Example*` functions (the `_test.go` file check is done by
/// path patterns elsewhere).
fn is_go_test(name: &str) -> bool {
    ["Test", "Benchmark", "Fuzz", "Example"]
        .iter()
        .any(|p| name.starts_with(p))
}

/// `(s *Server[T])` -> `Server`.
fn receiver_type(recv: &str) -> String {
    let inner = recv.trim_start_matches('(').trim_end_matches(')').trim();
    let ty = inner.rsplit([' ', '*']).next().unwrap_or(inner);
    ty.split('[').next().unwrap_or(ty).trim().to_string()
}
