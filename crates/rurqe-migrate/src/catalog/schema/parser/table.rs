//! Parsing of `table` blocks: columns, keys, checks, and indexes.

use crate::catalog::types::{
    CheckDef, ColumnDef, ForeignKeyAction, ForeignKeyDef, IndexDef, TableDef,
};
use crate::error::Error;

use super::value::{
    lock_error, parse_escaped_quoted_prefix, parse_quoted_default, parse_quoted_list,
    parse_quoted_prefix, parse_type_kind,
};
use super::Defaults;

/// Parses the `table "<name>" { ... }` block starting at `start`.
///
/// The `defaults` of the snapshot decide how the SQL text of a default is read.
pub(super) fn parse_table(
    lines: &[&str],
    start: usize,
    defaults: Defaults,
) -> Result<(TableDef, usize), Error> {
    let header = lines.get(start).copied().unwrap_or_default();
    let mut table = TableBuilder::new(parse_table_header(header.trim_start())?);

    // Buffer of raw lines belonging to the body item being parsed. A column
    // default may span multiple lines, so items are collected until the next item
    // keyword or the closing brace is reached.
    let mut item_lines: Vec<String> = Vec::new();
    let body = lines.get(start + 1..).unwrap_or_default();

    for (offset, line) in body.iter().enumerate() {
        let trimmed = line.trim_start();

        if trimmed.is_empty() {
            if item_lines.is_empty() {
                return Err(lock_error("unexpected blank line inside a table block"));
            }
            item_lines.push((*line).to_owned());
            continue;
        }

        let is_close = trimmed == "}";
        let is_item = trimmed.starts_with("column ")
            || trimmed.starts_with("primary key ")
            || trimmed.starts_with("check ")
            || trimmed.starts_with("foreign key ")
            || trimmed.starts_with("index ")
            || trimmed.starts_with("unique index ");

        if is_close || is_item {
            if !item_lines.is_empty() {
                apply_table_item(&mut table, &item_lines.join("\n"), defaults)?;
                item_lines.clear();
            }
            if is_close {
                return Ok((table.finish(), start + offset + 2));
            }
            item_lines.push((*line).to_owned());
        } else {
            if item_lines.is_empty() {
                return Err(lock_error("unexpected line inside a table block"));
            }
            item_lines.push((*line).to_owned());
        }
    }

    Err(lock_error("unterminated table block: missing `}`"))
}

/// Builds a [`TableDef`] while a `table` block is being parsed.
struct TableBuilder {
    name: String,
    columns: Vec<ColumnDef>,
    primary_key: Vec<String>,
    checks: Vec<CheckDef>,
    foreign_keys: Vec<ForeignKeyDef>,
    indexes: Vec<IndexDef>,
}

impl TableBuilder {
    fn new(name: String) -> Self {
        Self {
            name,
            columns: Vec::new(),
            primary_key: Vec::new(),
            checks: Vec::new(),
            foreign_keys: Vec::new(),
            indexes: Vec::new(),
        }
    }

    /// Finalizes the table, sorting columns, checks, foreign keys, and indexes by name
    /// so the result matches the canonical ordering used by the renderer.
    fn finish(mut self) -> TableDef {
        self.columns
            .sort_by(|left, right| left.name.cmp(&right.name));
        self.checks
            .sort_by(|left, right| left.name.cmp(&right.name));
        self.foreign_keys
            .sort_by(|left, right| left.name.cmp(&right.name));
        self.indexes
            .sort_by(|left, right| left.name.cmp(&right.name));
        TableDef {
            name: self.name,
            columns: self.columns,
            primary_key: self.primary_key,
            checks: self.checks,
            foreign_keys: self.foreign_keys,
            indexes: self.indexes,
        }
    }
}

/// A located nullability marker inside a column definition tail.
struct NullabilityMarker {
    start: usize,
    len: usize,
    nullable: bool,
    has_default: bool,
}

/// Dispatches one buffered table-body item to the matching parser.
fn apply_table_item(table: &mut TableBuilder, item: &str, defaults: Defaults) -> Result<(), Error> {
    let item = item.trim_start();
    if item.starts_with("column ") {
        table.columns.push(parse_column(item, defaults)?);
    } else if item.starts_with("primary key ") {
        table.primary_key = parse_primary_key(item)?;
    } else if item.starts_with("check ") {
        table.checks.push(parse_check(item)?);
    } else if item.starts_with("foreign key ") {
        table.foreign_keys.push(parse_foreign_key(item)?);
    } else if item.starts_with("unique index ") {
        table.indexes.push(parse_index(item, true)?);
    } else if item.starts_with("index ") {
        table.indexes.push(parse_index(item, false)?);
    } else {
        return Err(lock_error("unrecognised table-body item"));
    }
    Ok(())
}

/// Parses a `table "<name>" {` header and returns the table name.
fn parse_table_header(line: &str) -> Result<String, Error> {
    let rest = line
        .strip_prefix("table ")
        .ok_or_else(|| lock_error("expected `table` keyword"))?;
    let (name, rest) = parse_quoted_prefix(rest)?;
    let rest = rest
        .strip_prefix(" {")
        .ok_or_else(|| lock_error("expected `{` after the table name"))?;
    if !rest.is_empty() {
        return Err(lock_error("unexpected text after the table header"));
    }
    Ok(name)
}

/// Parses a `column "<name>": <type> ...` definition.
///
/// The `defaults` of the snapshot decide how the SQL text of the default is read.
fn parse_column(item: &str, defaults: Defaults) -> Result<ColumnDef, Error> {
    let rest = item
        .strip_prefix("column ")
        .ok_or_else(|| lock_error("expected `column` keyword"))?;
    let (name, rest) = parse_quoted_prefix(rest)?;
    let rest = rest
        .strip_prefix(": ")
        .ok_or_else(|| lock_error("expected `: ` after a column name"))?;

    let marker = find_nullability_marker(rest)?;
    let type_kind = parse_type_kind(&rest[..marker.start])?;
    let default = if marker.has_default {
        let text = &rest[marker.start + marker.len..];
        Some(match defaults {
            Defaults::Verbatim => text.to_owned(),
            Defaults::Quoted => parse_quoted_default(text)?,
        })
    } else {
        None
    };

    Ok(ColumnDef {
        name,
        type_kind,
        nullable: marker.nullable,
        default,
    })
}

/// Parses a `primary key ("a", "b")` declaration.
fn parse_primary_key(item: &str) -> Result<Vec<String>, Error> {
    let rest = item
        .strip_prefix("primary key ")
        .ok_or_else(|| lock_error("expected `primary key` keyword"))?;
    let (columns, rest) = parse_quoted_list(rest)?;
    if !rest.is_empty() {
        return Err(lock_error(
            "unexpected text after the primary key declaration",
        ));
    }
    Ok(columns)
}

/// Parses a `check "<name>" "<expression>"` declaration.
fn parse_check(item: &str) -> Result<CheckDef, Error> {
    let rest = item
        .strip_prefix("check ")
        .ok_or_else(|| lock_error("expected `check` keyword"))?;
    let (name, rest) = parse_quoted_prefix(rest)?;
    let rest = rest
        .strip_prefix(' ')
        .ok_or_else(|| lock_error("expected a space before the check expression"))?;
    let (expression, rest) = parse_escaped_quoted_prefix(rest)?;
    if !rest.trim().is_empty() {
        return Err(lock_error(
            "unexpected text after the check constraint declaration",
        ));
    }
    Ok(CheckDef::new(name, expression))
}

/// Parses a `foreign key "<name>" (...) references "<table>" (...) [on delete <act>] [on update <act>]` declaration.
fn parse_foreign_key(item: &str) -> Result<ForeignKeyDef, Error> {
    let rest = item
        .strip_prefix("foreign key ")
        .ok_or_else(|| lock_error("expected `foreign key` keyword"))?;
    let (name, rest) = parse_quoted_prefix(rest)?;
    let rest = rest
        .strip_prefix(' ')
        .ok_or_else(|| lock_error("expected a space before the foreign-key columns"))?;
    let (columns, rest) = parse_quoted_list(rest)?;
    let rest = rest
        .strip_prefix(" references ")
        .ok_or_else(|| lock_error("expected `references` in the foreign key"))?;
    let (referenced_table, rest) = parse_quoted_prefix(rest)?;
    let rest = rest
        .strip_prefix(' ')
        .ok_or_else(|| lock_error("expected a space before the referenced columns"))?;
    let (referenced_columns, mut rest) = parse_quoted_list(rest)?;

    let mut on_delete = ForeignKeyAction::NoAction;
    let mut on_update = ForeignKeyAction::NoAction;

    rest = rest.trim_start();
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix("on delete ") {
            let (action, remainder) = parse_fk_action_and_rest(after)?;
            on_delete = action;
            rest = remainder.trim_start();
        } else if let Some(after) = rest.strip_prefix("on update ") {
            let (action, remainder) = parse_fk_action_and_rest(after)?;
            on_update = action;
            rest = remainder.trim_start();
        } else {
            return Err(lock_error(format!(
                "unexpected text after the foreign key declaration: `{rest}`"
            )));
        }
    }

    Ok(ForeignKeyDef {
        name,
        columns,
        referenced_table,
        referenced_columns,
        on_delete,
        on_update,
    })
}

/// Parses a foreign key action name and returns the remaining unparsed input.
fn parse_fk_action_and_rest(input: &str) -> Result<(ForeignKeyAction, &str), Error> {
    let input = input.trim_start();
    let lower = input.to_ascii_lowercase();
    if let Some(rest) = lower.strip_prefix("no action") {
        Ok((
            ForeignKeyAction::NoAction,
            &input[input.len() - rest.len()..],
        ))
    } else if let Some(rest) = lower.strip_prefix("set default") {
        Ok((
            ForeignKeyAction::SetDefault,
            &input[input.len() - rest.len()..],
        ))
    } else if let Some(rest) = lower.strip_prefix("set null") {
        Ok((
            ForeignKeyAction::SetNull,
            &input[input.len() - rest.len()..],
        ))
    } else if let Some(rest) = lower.strip_prefix("restrict") {
        Ok((
            ForeignKeyAction::Restrict,
            &input[input.len() - rest.len()..],
        ))
    } else if let Some(rest) = lower.strip_prefix("cascade") {
        Ok((
            ForeignKeyAction::Cascade,
            &input[input.len() - rest.len()..],
        ))
    } else {
        Err(lock_error("unrecognised foreign key action"))
    }
}

/// Parses an `index "<name>" (...)` or `unique index "<name>" (...)` declaration.
fn parse_index(item: &str, unique: bool) -> Result<IndexDef, Error> {
    let rest = item
        .strip_prefix(if unique { "unique index " } else { "index " })
        .ok_or_else(|| lock_error("expected `index` keyword"))?;
    let (name, rest) = parse_quoted_prefix(rest)?;
    let rest = rest
        .strip_prefix(' ')
        .ok_or_else(|| lock_error("expected a space before the indexed columns"))?;
    let (columns, mut rest) = parse_quoted_list(rest)?;

    let mut method = None;
    let mut expression = None;
    let mut predicate = None;

    rest = rest.trim_start();
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix("using ") {
            let (m, after_m) = parse_quoted_prefix(after.trim_start())?;
            method = Some(m);
            rest = after_m.trim_start();
        } else if let Some(after) = rest.strip_prefix("expression ") {
            let (expr, after_expr) = parse_escaped_quoted_prefix(after.trim_start())?;
            expression = Some(expr);
            rest = after_expr.trim_start();
        } else if let Some(after) = rest.strip_prefix("where ") {
            let (pred, after_pred) = parse_escaped_quoted_prefix(after.trim_start())?;
            predicate = Some(pred);
            rest = after_pred.trim_start();
        } else {
            return Err(lock_error(format!("unexpected text after index: `{rest}`")));
        }
    }

    let mut index_def = IndexDef::new(name, columns, unique);
    if let Some(m) = method {
        index_def = index_def.with_method(m);
    }
    if let Some(expr) = expression {
        index_def = index_def.with_expression(expr);
    }
    if let Some(pred) = predicate {
        index_def = index_def.with_predicate(pred);
    }
    Ok(index_def)
}

/// Locates the earliest nullability marker in a column tail.
///
/// The earliest match is chosen so that defaults containing `null` or
/// `default` never shadow the real marker placed right after the type.
fn find_nullability_marker(rest: &str) -> Result<NullabilityMarker, Error> {
    let mut markers: Vec<NullabilityMarker> = Vec::new();

    if let Some(start) = rest.find(" not null default ") {
        markers.push(NullabilityMarker {
            start,
            len: " not null default ".len(),
            nullable: false,
            has_default: true,
        });
    }
    if let Some(start) = rest.find(" null default ") {
        markers.push(NullabilityMarker {
            start,
            len: " null default ".len(),
            nullable: true,
            has_default: true,
        });
    }
    if rest.ends_with(" not null") {
        markers.push(NullabilityMarker {
            start: rest.len() - " not null".len(),
            len: " not null".len(),
            nullable: false,
            has_default: false,
        });
    }
    if rest.ends_with(" null") {
        markers.push(NullabilityMarker {
            start: rest.len() - " null".len(),
            len: " null".len(),
            nullable: true,
            has_default: false,
        });
    }

    markers
        .into_iter()
        .min_by_key(|marker| (marker.start, u8::from(!marker.has_default)))
        .ok_or_else(|| lock_error("column definition is missing a nullability marker"))
}
