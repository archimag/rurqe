//! Parsing of a legacy `schema.lock` snapshot into a [`SchemaCatalog`].

mod sections;
mod table;
mod value;

use super::SchemaCatalog;
use crate::catalog::lock;
use crate::catalog::types::{EnumDef, TableDef, TriggerDef, ViewDef};
use crate::error::Error;

use self::sections::{parse_enum, parse_trigger, parse_view};
use self::table::parse_table;
use self::value::lock_error;

/// Parses a legacy `schema.lock` snapshot back into a catalog.
///
/// The parser accepts exactly the representation the retired v3 writer
/// produced and returns the catalog sorted by enum name,
/// table name, column name, foreign-key name, and index name, so the result
/// equals the catalog that generated the snapshot. The labels of an enum keep
/// the order they were rendered in.
///
/// A snapshot that still carries the v1 header is read as well: it differs
/// only in writing the SQL text of a column default verbatim, without the
/// quotes and escapes the current format applies.
pub(super) fn parse_lock(lock: &str) -> Result<SchemaCatalog, Error> {
    let mut lines = lock.lines();
    let defaults = match lines.next() {
        Some(header) if header == lock::LOCK_HEADER => Defaults::Quoted,
        Some(header) if header == lock::V2_LOCK_HEADER => Defaults::Quoted,
        Some(header) if header == lock::LEGACY_LOCK_HEADER => Defaults::Verbatim,
        _ => {
            return Err(lock_error(format!(
                "expected `{}` header",
                lock::LOCK_HEADER
            )))
        }
    };

    let lines: Vec<&str> = lines.collect();
    let mut enums: Vec<EnumDef> = Vec::new();
    let mut tables: Vec<TableDef> = Vec::new();
    let mut views: Vec<ViewDef> = Vec::new();
    let mut triggers: Vec<TriggerDef> = Vec::new();
    let mut index = 0;

    while index < lines.len() {
        let Some(line) = lines.get(index).copied() else {
            break;
        };
        let trimmed = line.trim_start();

        if trimmed.is_empty() {
            index += 1;
            continue;
        }

        if trimmed == lock::ENUM_HEADER {
            let (definition, next) = parse_enum(&lines, index + 1)?;
            enums.push(definition);
            index = next;
            continue;
        }

        if trimmed == lock::VIEW_HEADER {
            let (view, next) = parse_view(&lines, index + 1)?;
            views.push(view);
            index = next;
            continue;
        }

        if trimmed == lock::TRIGGER_HEADER {
            let (trigger, next) = parse_trigger(&lines, index + 1)?;
            triggers.push(trigger);
            index = next;
            continue;
        }

        if trimmed.starts_with("table \"") {
            let (table, next) = parse_table(&lines, index, defaults)?;
            tables.push(table);
            index = next;
            continue;
        }

        return Err(lock_error(
            "expected a `table` declaration or an `[[enum]]`, `[[view]]`, or `[[trigger]]` section",
        ));
    }

    enums.sort_by(|left, right| left.name.cmp(&right.name));
    tables.sort_by(|left, right| left.name.cmp(&right.name));
    views.sort_by(|left, right| left.name.cmp(&right.name));
    triggers.sort_by(|left, right| left.name.cmp(&right.name));

    Ok(SchemaCatalog::new(tables)
        .with_enums(enums)
        .with_views(views)
        .with_triggers(triggers))
}

/// How the column defaults of a snapshot are encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Defaults {
    /// A v1 snapshot writes the SQL text of a default verbatim.
    Verbatim,
    /// A v2 snapshot writes the SQL text of a default as a quoted, escaped string.
    Quoted,
}
