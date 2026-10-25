//! Parsing of `[[enum]]`, `[[view]]`, and `[[trigger]]` sections.

use crate::catalog::lock;
use crate::catalog::types::{EnumDef, TriggerDef, ViewDef};
use crate::error::Error;

use super::value::{lock_error, parse_quoted_default, parse_quoted_value, parse_variant_list};

/// Parses the `[[enum]]` section whose entries start at `start`.
///
/// The section ends at the next section header or at the end of the snapshot, so
/// the returned index points at the line that follows it.
pub(super) fn parse_enum(lines: &[&str], start: usize) -> Result<(EnumDef, usize), Error> {
    let body = lines.get(start..).unwrap_or_default();
    let mut name: Option<String> = None;
    let mut variants: Option<Vec<String>> = None;

    for (offset, line) in body.iter().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed == lock::ENUM_HEADER
            || trimmed == lock::VIEW_HEADER
            || trimmed == lock::TRIGGER_HEADER
            || trimmed.starts_with("table \"")
        {
            return finish_enum(name, variants, start + offset);
        }

        let (key, value) = split_entry(trimmed)?;
        match key {
            "name" => name = Some(parse_quoted_value(value)?),
            "variants" => variants = Some(parse_variant_list(value)?),
            _ => return Err(lock_error("unrecognised enum entry")),
        }
    }

    finish_enum(name, variants, start + body.len())
}

/// Builds the parsed enum, reporting the entries a section is missing.
fn finish_enum(
    name: Option<String>,
    variants: Option<Vec<String>>,
    next: usize,
) -> Result<(EnumDef, usize), Error> {
    let name = name.ok_or_else(|| lock_error("enum section is missing a `name` entry"))?;
    let variants =
        variants.ok_or_else(|| lock_error("enum section is missing a `variants` entry"))?;
    Ok((EnumDef::new(name, variants), next))
}

/// Splits a `key = value` entry of an enum section.
fn split_entry(line: &str) -> Result<(&str, &str), Error> {
    match line.split_once('=') {
        Some((key, value)) => Ok((key.trim(), value.trim())),
        None => Err(lock_error("expected a `key = value` entry")),
    }
}

/// Parses the `[[view]]` section whose entries start at `start`.
pub(super) fn parse_view(lines: &[&str], start: usize) -> Result<(ViewDef, usize), Error> {
    let body = lines.get(start..).unwrap_or_default();
    let mut name: Option<String> = None;
    let mut definition: Option<String> = None;

    for (offset, line) in body.iter().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed == lock::ENUM_HEADER
            || trimmed == lock::VIEW_HEADER
            || trimmed == lock::TRIGGER_HEADER
            || trimmed.starts_with("table \"")
        {
            return finish_view(name, definition, start + offset);
        }

        let (key, value) = split_entry(trimmed)?;
        match key {
            "name" => name = Some(parse_quoted_value(value)?),
            "definition" => definition = Some(parse_quoted_default(value)?),
            _ => return Err(lock_error("unrecognised view entry")),
        }
    }

    finish_view(name, definition, start + body.len())
}

/// Builds the parsed view, reporting missing entries.
fn finish_view(
    name: Option<String>,
    definition: Option<String>,
    next: usize,
) -> Result<(ViewDef, usize), Error> {
    let name = name.ok_or_else(|| lock_error("view section is missing a `name` entry"))?;
    let definition =
        definition.ok_or_else(|| lock_error("view section is missing a `definition` entry"))?;
    Ok((ViewDef::new(name, definition), next))
}

/// Parses the `[[trigger]]` section whose entries start at `start`.
pub(super) fn parse_trigger(lines: &[&str], start: usize) -> Result<(TriggerDef, usize), Error> {
    let body = lines.get(start..).unwrap_or_default();
    let mut name: Option<String> = None;
    let mut table_name: Option<String> = None;
    let mut definition: Option<String> = None;

    for (offset, line) in body.iter().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed == lock::ENUM_HEADER
            || trimmed == lock::VIEW_HEADER
            || trimmed == lock::TRIGGER_HEADER
            || trimmed.starts_with("table \"")
        {
            return finish_trigger(name, table_name, definition, start + offset);
        }

        let (key, value) = split_entry(trimmed)?;
        match key {
            "name" => name = Some(parse_quoted_value(value)?),
            "table" => table_name = Some(parse_quoted_value(value)?),
            "definition" => definition = Some(parse_quoted_default(value)?),
            _ => return Err(lock_error("unrecognised trigger entry")),
        }
    }

    finish_trigger(name, table_name, definition, start + body.len())
}

/// Builds the parsed trigger, reporting missing entries.
fn finish_trigger(
    name: Option<String>,
    table_name: Option<String>,
    definition: Option<String>,
    next: usize,
) -> Result<(TriggerDef, usize), Error> {
    let name = name.ok_or_else(|| lock_error("trigger section is missing a `name` entry"))?;
    let table_name =
        table_name.ok_or_else(|| lock_error("trigger section is missing a `table` entry"))?;
    let definition =
        definition.ok_or_else(|| lock_error("trigger section is missing a `definition` entry"))?;
    Ok((TriggerDef::new(name, table_name, definition), next))
}
