//! Python definitions.

use tree_sitter::Node;

use super::{Owner, Walker};
use crate::model::SymKind;

impl Walker<'_> {
    pub(super) fn py_block(&mut self, parent: Node, owner: Option<&Owner>) {
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
                is_type: true,
                members_public: false,
            };
            self.py_block(body, Some(&o));
        }
    }
}

fn py_public(name: &str) -> bool {
    !name.starts_with('_') || (name.starts_with("__") && name.ends_with("__") && name.len() > 4)
}
