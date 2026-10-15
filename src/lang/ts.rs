//! TypeScript and TSX definitions.

use tree_sitter::Node;

use super::{Owner, Walker, find_kind};
use crate::model::SymKind;

impl Walker<'_> {
    pub(super) fn ts_block(&mut self, parent: Node) {
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
            is_type: true,
            members_public: false,
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
}

fn is_ts_test(name: &str) -> bool {
    name.starts_with("test")
}
