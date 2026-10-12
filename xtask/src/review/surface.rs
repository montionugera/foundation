//! The public surface of a Rust file, as the review check compares it.

use syn::{ForeignItem, ImplItem, Item, TraitItem, Visibility};

/// Whether the texts `old` and `new` of a `.rs` file differ in their public surface:
/// each inner attribute of the file, which a `//!` line is, and each item with `pub`
/// alone, at any depth, with its attributes and its doc. A `pub` item of a private
/// module, of an `impl` block with no trait, or of an `extern` block counts with the
/// header and the attributes of each module and block around it. A function body does
/// not count, and no item of a macro does. A text that is not Rust counts as a
/// difference.
pub(super) fn changed(old: &str, new: &str) -> bool {
    match (surface(old), surface(new)) {
        (Some(old), Some(new)) => old != new,
        _ => true,
    }
}

/// The file of `text` with only its public surface, or `None` when `text` does not
/// parse.
fn surface(text: &str) -> Option<syn::File> {
    let mut file = syn::parse_file(text).ok()?;
    keep(&mut file.items);
    Some(file)
}

/// Leaves in `items` each `pub` item with no function body, and each block or private
/// module that holds one.
fn keep(items: &mut Vec<Item>) {
    items.retain_mut(|item| {
        let vis = match item {
            Item::Fn(function) => {
                function.block.stmts.clear();
                &function.vis
            }
            Item::Trait(item) => {
                for item in &mut item.items {
                    if let TraitItem::Fn(function) = item
                        && let Some(body) = &mut function.default
                    {
                        body.stmts.clear();
                    }
                }
                &item.vis
            }
            Item::Mod(module) => {
                let inner = module.content.as_mut().is_some_and(|(_, items)| {
                    keep(items);
                    !items.is_empty()
                });
                return inner || public(&module.vis);
            }
            Item::Impl(block) => {
                block.items.retain_mut(|item| match item {
                    ImplItem::Fn(function) => {
                        function.block.stmts.clear();
                        public(&function.vis)
                    }
                    ImplItem::Const(item) => public(&item.vis),
                    ImplItem::Type(item) => public(&item.vis),
                    ImplItem::Macro(_) => false,
                    _ => true,
                });
                return block.trait_.is_none() && !block.items.is_empty();
            }
            Item::ForeignMod(block) => {
                block.items.retain(|item| match item {
                    ForeignItem::Fn(item) => public(&item.vis),
                    ForeignItem::Static(item) => public(&item.vis),
                    ForeignItem::Type(item) => public(&item.vis),
                    ForeignItem::Macro(_) => false,
                    _ => true,
                });
                return !block.items.is_empty();
            }
            Item::Macro(_) => return false,
            Item::Const(item) => &item.vis,
            Item::Enum(item) => &item.vis,
            Item::ExternCrate(item) => &item.vis,
            Item::Static(item) => &item.vis,
            Item::Struct(item) => &item.vis,
            Item::TraitAlias(item) => &item.vis,
            Item::Type(item) => &item.vis,
            Item::Union(item) => &item.vis,
            Item::Use(item) => &item.vis,
            // Syntax that `syn` does not model: it stays, so its change counts.
            _ => return true,
        };
        public(vis)
    });
}

/// Whether `vis` is `pub` alone.
fn public(vis: &Visibility) -> bool {
    matches!(vis, Visibility::Public(_))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Asserts that each pair differs in its public surface, in both directions.
    fn counts(pairs: &[(&str, &str)]) {
        for (old, new) in pairs {
            assert!(changed(old, new), "{old:?} to {new:?}");
            assert!(changed(new, old), "{new:?} to {old:?}");
        }
    }

    #[test]
    fn a_change_to_a_pub_item_or_its_doc_counts() {
        counts(&[
            ("pub fn a() {}", "pub fn a(n: u8) {}"),
            ("pub fn a() {}", "pub fn a() -> u8 { 1 }"),
            ("pub fn a() {}", "/// A doc.\npub fn a() {}"),
            (
                "/// A doc.\npub fn a() {}",
                "/// Another doc.\npub fn a() {}",
            ),
            ("pub fn a() {}", "#[must_use]\npub fn a() {}"),
            ("pub fn a() {}", "pub(crate) fn a() {}"),
            ("fn a() {}", "pub fn a() {}"),
            ("pub fn a() {}", ""),
            ("pub enum A { B }", "pub enum A { B, C }"),
            ("pub enum A { B }", "pub enum A {\n/// A doc.\nB }"),
            ("pub enum A { B }", "pub enum A { B(u8) }"),
            ("pub struct A { b: u8 }", "pub struct A { b: u16 }"),
            (
                "pub struct A { pub b: u8 }",
                "pub struct A {\n/// A doc.\npub b: u8 }",
            ),
            ("pub const A: u8 = 1;", "pub const A: u8 = 2;"),
            ("pub static A: u8 = 1;", "pub static A: u16 = 1;"),
            ("pub type A = u8;", "pub type A = u16;"),
            ("pub union A { b: u8 }", "pub union A { b: u16 }"),
            ("pub use a::B;", "pub use a::C;"),
        ]);
    }

    #[test]
    fn a_change_to_a_pub_item_of_a_module_a_trait_or_a_block_counts() {
        counts(&[
            ("pub mod a;", "/// A doc.\npub mod a;"),
            ("pub mod a {}", "pub mod a { pub fn b() {} }"),
            ("mod a { pub fn b() {} }", "mod a { pub fn b(n: u8) {} }"),
            ("mod a { pub fn b() {} }", "mod c { pub fn b() {} }"),
            ("pub trait A {}", "pub trait A { fn b(&self); }"),
            (
                "pub trait A { fn b(&self); }",
                "pub trait A { fn b(&self) {} }",
            ),
            (
                "pub trait A { fn b(&self); }",
                "pub trait A {\n/// A doc.\nfn b(&self); }",
            ),
            (
                "impl A { pub fn b(&self) {} }",
                "impl A { pub fn b(&mut self) {} }",
            ),
            (
                "impl A { pub fn b() {} }",
                "impl A {\n/// A doc.\npub fn b() {} }",
            ),
            ("impl A { pub fn b() {} }", "impl C { pub fn b() {} }"),
            (
                "impl A { pub fn b() {} }",
                "#[cfg(unix)]\nimpl A { pub fn b() {} }",
            ),
            (
                "mod a { pub fn b() {} }",
                "#[cfg(unix)]\nmod a { pub fn b() {} }",
            ),
            (
                "unsafe extern \"C\" { pub fn a(); }",
                "#[cfg(unix)]\nunsafe extern \"C\" { pub fn a(); }",
            ),
            (
                "unsafe extern \"C\" { pub fn a(); }",
                "unsafe extern \"C\" { fn a(); }",
            ),
            (
                "impl A { pub type B = u8; }",
                "impl A { pub type B = u16; }",
            ),
            (
                "impl A { pub const B: u8 = 1; }",
                "impl A { pub const B: u8 = 2; }",
            ),
            ("impl A { fn b() {} }", "impl A { pub fn b() {} }"),
            (
                "unsafe extern \"C\" { pub fn a(); }",
                "unsafe extern \"C\" { pub fn a(n: u8); }",
            ),
        ]);
    }

    #[test]
    fn a_change_to_an_inner_attribute_or_a_text_that_is_not_rust_counts() {
        counts(&[
            ("//! A crate.\n", "//! Another crate.\n"),
            ("", "//! A crate.\n"),
            ("", "#![no_std]\n"),
            ("pub fn a() {}", "pub fn a( {}"),
            ("pub fn a( {}", "pub fn a() {}"),
        ]);
    }

    #[test]
    fn a_change_to_no_pub_item_does_not_count() {
        let pairs = [
            ("pub fn a() {}", "pub fn a() { b(); }"),
            (
                "pub fn a() -> u8 { 1 }",
                "pub fn a() -> u8 {\n// A comment.\n2 }",
            ),
            ("pub fn a() {}", "// A comment.\npub fn a()   {}\n\n"),
            ("pub fn a() {}", "pub fn a() { pub fn b() {} }"),
            ("", "fn a() {}"),
            ("fn a() {}", "/// A doc.\nfn a(n: u8) {}"),
            ("", "pub(crate) fn a() {}"),
            ("", "pub(super) struct A;"),
            ("", "pub(in crate::a) enum A {}"),
            ("", "struct A { pub b: u8 }"),
            ("", "mod a;"),
            ("", "#[cfg(test)]\nmod tests;"),
            ("", "mod a { fn b() {} }"),
            (
                "mod a { pub fn b() {} }",
                "mod a { pub fn b() {} fn c() {} }",
            ),
            ("", "/// A doc.\nmod a { fn b() {} }"),
            ("", "impl A { const B: u8 = 1; type C = u8; }"),
            ("", "unsafe extern \"C\" { static A: u8; type B; }"),
            ("pub mod a {}", "pub mod a { fn b() {} }"),
            ("", "impl A { fn b() {} }"),
            ("", "impl A { pub(crate) fn b() {} }"),
            ("impl A { pub fn b() {} }", "impl A { pub fn b() { c(); } }"),
            ("", "impl B for A { fn b(&self) {} }"),
            (
                "impl B for A { fn b(&self) {} }",
                "impl B for A { fn b(&self) { c(); } }",
            ),
            (
                "pub trait A { fn b(&self) {} }",
                "pub trait A { fn b(&self) { c(); } }",
            ),
            ("", "unsafe extern \"C\" { fn a(); }"),
            ("", "#[macro_export]\nmacro_rules! a { () => {}; }"),
            ("", "a!();"),
            ("", "use a::B;"),
            ("", "const A: u8 = 1;"),
            ("", "static A: u8 = 1;"),
            ("", "type A = u8;"),
            ("", "union A { b: u8 }"),
            ("", "trait A = B;"),
            ("", "extern crate a;"),
            ("", "impl A { a!(); }"),
            ("", "unsafe extern \"C\" { a!(); }"),
        ];
        for (old, new) in pairs {
            assert!(!changed(old, new), "{old:?} to {new:?}");
            assert!(!changed(new, old), "{new:?} to {old:?}");
        }
    }
}
