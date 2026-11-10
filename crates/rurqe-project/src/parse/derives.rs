//! Reading of the `[codegen].enum_derives` array.
//!
//! The array names the extra derives the generated enums carry, so a project whose
//! enums travel over a wire protocol declares its Serde and Specta trait paths once
//! instead of editing the generated module. Every entry is trimmed and has to be
//! non-empty, and a trait may appear once, because a repeated derive would be
//! emitted twice into `#[derive(...)]` and the generated module would stop
//! compiling. A rejected entry is reported at the entry itself and named by its
//! index, so the diagnostic points at the declaration that has to change instead of
//! at the key of the whole array.

use std::collections::BTreeSet;
use std::ops::Range;

use toml_edit::Item;

use super::syntax_error;
use crate::ConfigError;

/// Reads the `[codegen].enum_derives` array naming the extra derives of generated enums.
pub(super) fn read_enum_derives(
    contents: &str,
    span: Option<Range<usize>>,
    item: &Item,
    section: &str,
) -> Result<Vec<String>, ConfigError> {
    let array = item.as_array().ok_or_else(|| {
        syntax_error(
            contents,
            span.clone(),
            format!("`{section}` must be an array of Rust trait paths"),
        )
    })?;

    let mut derives: Vec<String> = Vec::with_capacity(array.len());
    let mut seen: BTreeSet<String> = BTreeSet::new();

    for (index, value) in array.iter().enumerate() {
        // The span of the entry itself, falling back to the key of the array.
        let entry_span = value.span().or_else(|| span.clone());
        // The entry is named by its index wherever it is refused, because TOML
        // records no position of its own for an element of an array.
        let key = format!("{section}[{index}]");

        let raw = value.as_str().ok_or_else(|| {
            syntax_error(
                contents,
                entry_span.clone(),
                format!("`{key}` must be a Rust trait path string"),
            )
        })?;

        let derive = raw.trim();

        if derive.is_empty() {
            return Err(syntax_error(
                contents,
                entry_span,
                format!("`{key}` requires a Rust trait path"),
            ));
        }

        if !seen.insert(derive.to_owned()) {
            return Err(syntax_error(
                contents,
                entry_span,
                format!("duplicate derive `{derive}` in `{section}`"),
            ));
        }

        derives.push(derive.to_owned());
    }

    Ok(derives)
}
