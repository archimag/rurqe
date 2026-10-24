//! Reading the schema digest out of the generated schema code.
//!
//! The generated module embeds the canonical catalog digest as
//! `pub const SCHEMA_HASH: &str = "…";`, which is the only bridge between the code
//! and the live catalog. The constant is located with the project's Rust parser
//! instead of a hand-written scanner, so line and block comments, string and
//! character literals, raw strings, and nested inline modules are all handled by
//! the grammar itself rather than by a supported-subset approximation.

/// Extracts the `pub const SCHEMA_HASH: &str = "...";` literal from generated code.
///
/// The constant is searched among the item declarations of the file, including the
/// items of inline modules, so a constant declared one level down is found as well.
/// Only a string literal is accepted, because a constant computed from another
/// value carries no digest this verifier could compare against. Code that does not
/// parse as a Rust file yields `None`: it cannot carry a generated marker either.
pub fn extract_schema_hash(code: &str) -> Option<String> {
    let file = syn::parse_file(code).ok()?;

    find_schema_hash(&file.items)
}

/// Searches a list of items for the string `SCHEMA_HASH` constant.
fn find_schema_hash(items: &[syn::Item]) -> Option<String> {
    use syn::{Expr, Item, Lit};

    for item in items {
        match item {
            Item::Const(item_const) if item_const.ident == "SCHEMA_HASH" => {
                let Expr::Lit(literal) = &*item_const.expr else {
                    continue;
                };
                let Lit::Str(value) = &literal.lit else {
                    continue;
                };

                return Some(value.value());
            }
            Item::Mod(item_mod) => {
                let Some((_, nested)) = &item_mod.content else {
                    continue;
                };
                if let Some(found) = find_schema_hash(nested) {
                    return Some(found);
                }
            }
            _ => {}
        }
    }

    None
}
