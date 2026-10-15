//! Rust definitions: functions, structs, enums, unions, traits, type aliases, and the methods of
//! `impl` and `trait` blocks. `mod` blocks qualify their items.

use tree_sitter::Node;

use super::{Owner, Walker, find_kind};
use crate::model::SymKind;

impl Walker<'_> {
    /// Items of a source file, `mod` body or `impl`/`trait` body. Attributes precede their item
    /// as siblings, so `#[test]` and `#[cfg(test)]` are carried forward to the next item.
    pub(super) fn rs_block(&mut self, parent: Node, owner: Option<&Owner>) {
        let mut c = parent.walk();
        let mut test_attr = false;
        for n in parent.children(&mut c) {
            if n.kind() == "attribute_item" {
                test_attr |= is_test_attr(self.text(n));
                continue;
            }
            if n.is_named() && !n.kind().contains("comment") {
                self.rs_item(n, owner, test_attr);
                test_attr = false;
            }
        }
    }

    fn rs_item(&mut self, d: Node, owner: Option<&Owner>, test_attr: bool) {
        let public =
            find_kind(d, "visibility_modifier").is_some() && owner.is_none_or(|o| o.public);
        let in_test = test_attr || owner.is_some_and(|o| o.is_test);
        match d.kind() {
            "function_item" | "function_signature_item" => {
                let name = self.field_text(d, "name");
                let sig = self.rs_fn_sig(d);
                let kind = if owner.is_some_and(|o| o.is_type) {
                    SymKind::Method
                } else {
                    SymKind::Function
                };
                // Trait methods are as public as the trait; impl methods need their own `pub`
                // unless the impl is of a trait.
                let public = public || owner.is_some_and(|o| o.public && o.members_public);
                self.push(d, d, name, owner, kind, public, sig, in_test);
            }
            "struct_item" | "enum_item" | "union_item" => {
                let name = self.field_text(d, "name");
                let sig = self.sig(&[
                    d.child_by_field_name("type_parameters"),
                    find_kind(d, "where_clause"),
                    d.child_by_field_name("body"),
                ]);
                self.push(d, d, name, owner, SymKind::Class, public, sig, in_test);
            }
            "type_item" => {
                let name = self.field_text(d, "name");
                let sig = self.sig(&[
                    d.child_by_field_name("type_parameters"),
                    d.child_by_field_name("type"),
                ]);
                self.push(d, d, name, owner, SymKind::TypeAlias, public, sig, in_test);
            }
            "trait_item" => {
                let name = self.field_text(d, "name");
                let sig = self.sig(&[
                    d.child_by_field_name("type_parameters"),
                    d.child_by_field_name("bounds"),
                    find_kind(d, "where_clause"),
                ]);
                let qualified = self.qualify(owner, &name);
                self.push(d, d, name, owner, SymKind::Interface, public, sig, in_test);
                self.rs_body(d, qualified, public, true, in_test);
            }
            "impl_item" => {
                // `impl Trait for Type` and `impl Type` both qualify methods by the type.
                let ty = d
                    .child_by_field_name("type")
                    .map(|t| self.rs_type_name(t))
                    .unwrap_or_default();
                let of_trait = d.child_by_field_name("trait").is_some();
                let qualified = self.qualify(owner, &ty);
                let owner_public = owner.is_none_or(|o| o.public);
                self.rs_body(d, qualified, owner_public, of_trait, in_test);
            }
            "mod_item" => {
                let name = self.field_text(d, "name");
                if let Some(body) = d.child_by_field_name("body") {
                    let o = Owner {
                        name: self.qualify(owner, &name),
                        public,
                        is_test: in_test || name == "tests",
                        is_type: false,
                        members_public: false,
                    };
                    self.rs_block(body, Some(&o));
                }
            }
            _ => {}
        }
    }

    fn rs_body(
        &mut self,
        d: Node,
        qualified: String,
        public: bool,
        members_public: bool,
        is_test: bool,
    ) {
        if let Some(body) = d.child_by_field_name("body") {
            let o = Owner {
                name: qualified,
                public,
                is_test,
                is_type: true,
                members_public,
            };
            self.rs_block(body, Some(&o));
        }
    }

    fn rs_fn_sig(&self, f: Node) -> String {
        let ret = f
            .child_by_field_name("return_type")
            .map(|r| format!("-> {}", self.text(r)));
        let params = self.sig(&[
            find_kind(f, "function_modifiers"),
            f.child_by_field_name("type_parameters"),
            f.child_by_field_name("parameters"),
        ]);
        let wh = find_kind(f, "where_clause").map(|w| self.text(w).to_string());
        [params, ret.unwrap_or_default(), wh.unwrap_or_default()]
            .join(" ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// `Vec<T>` -> `Vec`, `crate::a::B` -> `B`, `&mut Foo` -> `Foo`.
    fn rs_type_name(&self, t: Node) -> String {
        let text = self.text(t);
        let base = text.split('<').next().unwrap_or(text);
        let base = base.rsplit("::").next().unwrap_or(base);
        base.trim_start_matches(['&', '*'])
            .trim_start_matches("mut ")
            .trim_start_matches("dyn ")
            .trim()
            .to_string()
    }

    fn qualify(&self, owner: Option<&Owner>, name: &str) -> String {
        owner.map_or_else(|| name.to_string(), |o| format!("{}.{}", o.name, name))
    }
}

/// `#[test]`, `#[tokio::test]`, `#[cfg(test)]` and similar.
fn is_test_attr(attr: &str) -> bool {
    let inner = attr
        .trim_start_matches("#[")
        .trim_end_matches(']')
        .split_whitespace()
        .collect::<String>();
    inner == "test" || inner.ends_with("::test") || inner == "cfg(test)"
}
