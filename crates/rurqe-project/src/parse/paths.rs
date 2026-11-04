//! Reading of the paths a `rurqe.toml` document declares.
//!
//! Every declared path is written as the plain relative path it resolves to
//! against the directory holding the configuration file. A value the host reads
//! as absolute or rooted, and a value carrying a `..` segment — a round trip
//! ending back inside the directory included — are refused at the key that
//! declares them, so a declaration relocates an artifact inside the project but
//! never rewrites where it lands.

use std::ops::Range;
use std::path::{Component, Path};

use toml_edit::Item;

use crate::ConfigError;
use super::syntax_error;

/// Reads a key whose value is a non-empty string, trimmed of surrounding space.
fn read_required_string(
    contents: &str,
    span: Option<Range<usize>>,
    item: &Item,
    key: &str,
) -> Result<String, ConfigError> {
    let value = item.as_str().ok_or_else(|| {
        syntax_error(contents, span.clone(), format!("`{key}` must be a string"))
    })?;
    let value = value.trim();

    if value.is_empty() {
        return Err(syntax_error(
            contents,
            span,
            format!("`{key}` requires a non-empty value"),
        ));
    }

    Ok(value.to_owned())
}

/// Reads a key whose value is a path the document declares.
///
/// The value is read like any other string and then resolved against the
/// directory holding `rurqe.toml`, so it is written as the plain relative path it
/// resolves to, and [`validate_declared_path`] refuses a value that would place
/// the artifact elsewhere.
pub(super) fn read_required_path(
    contents: &str,
    span: Option<Range<usize>>,
    item: &Item,
    key: &str,
) -> Result<String, ConfigError> {
    let value = read_required_string(contents, span.clone(), item, key)?;

    validate_declared_path(contents, span, key, &value)?;

    Ok(value)
}

/// Refuses a declared path that names its location through anything but a plain
/// relative path.
///
/// A value the host reads as absolute or rooted replaces the configuration
/// directory entirely, and a `..` segment rewrites the position the path has
/// reached, so neither shape is accepted: the refusal covers the segment itself,
/// a round trip such as `ddl/../schema` included.
fn validate_declared_path(
    contents: &str,
    span: Option<Range<usize>>,
    key: &str,
    value: &str,
) -> Result<(), ConfigError> {
    let path = Path::new(value);

    if matches!(
        path.components().next(),
        Some(Component::Prefix(_) | Component::RootDir)
    ) {
        return Err(syntax_error(
            contents,
            span,
            format!(
                "`{key}` must be a path relative to the directory holding the configuration \
                 file; `{value}` is absolute and would discard it"
            ),
        ));
    }

    if path
        .components()
        .any(|component| component == Component::ParentDir)
    {
        return Err(syntax_error(
            contents,
            span,
            format!(
                "`{key}` must be a path without a `..` segment; `{value}` contains one, and \
                 every such segment is refused, including a round trip that ends back inside \
                 the directory holding the configuration file"
            ),
        ));
    }

    Ok(())
}
