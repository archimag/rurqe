//! The engine-neutral schema catalog and the `schema.lock` round trip.

use crate::checksum::Checksum;
use crate::error::Error;

use super::lock;
use super::types::{
    CheckDef, ColumnDef, EnumDef, ForeignKeyAction, ForeignKeyDef, IndexDef, TableDef, TriggerDef,
    TypeKind, ViewDef,
};

/// The complete schema of a database as an engine-neutral catalog.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SchemaCatalog {
    /// The tables of the schema.
    pub tables: Vec<TableDef>,
    /// The enum types declared by the database.
    pub enums: Vec<EnumDef>,
    /// The views declared by the database.
    pub views: Vec<ViewDef>,
    /// The triggers declared by the database.
    pub triggers: Vec<TriggerDef>,
}

impl SchemaCatalog {
    /// Creates a catalog from the given tables and no enum types, views, or triggers.
    pub fn new(tables: Vec<TableDef>) -> Self {
        Self {
            tables,
            enums: Vec::new(),
            views: Vec::new(),
            triggers: Vec::new(),
        }
    }

    /// Attaches the enum types `enums` to the catalog.
    pub fn with_enums(mut self, enums: Vec<EnumDef>) -> Self {
        self.enums = enums;
        self
    }

    /// Attaches the views `views` to the catalog.
    pub fn with_views(mut self, views: Vec<ViewDef>) -> Self {
        self.views = views;
        self
    }

    /// Attaches the triggers `triggers` to the catalog.
    pub fn with_triggers(mut self, triggers: Vec<TriggerDef>) -> Self {
        self.triggers = triggers;
        self
    }

    /// Renders the canonical `schema.lock` snapshot.
    ///
    /// Enum sections, tables, views, and triggers are emitted sorted by name, so
    /// the snapshot depends only on the schema content and never on declaration order.
    pub fn to_lock(&self) -> String {
        lock::render_lock(&self.enums, &self.tables, &self.views, &self.triggers)
    }

    /// The SHA-256 digest of the canonical snapshot.
    pub fn checksum(&self) -> Checksum {
        Checksum::of(self.to_lock().as_bytes())
    }

    /// The hexadecimal digest of the canonical snapshot.
    pub fn schema_hash(&self) -> String {
        self.checksum().to_hex()
    }

    /// Renders the `SCHEMA_HASH` constant embedded into generated schema code.
    pub fn hash_const(&self) -> String {
        format!("pub const SCHEMA_HASH: &str = \"{}\";", self.schema_hash())
    }

    /// Parses a canonical `schema.lock` snapshot back into a catalog.
    ///
    /// The parser accepts exactly the representation produced by
    /// [`SchemaCatalog::to_lock`] and returns the catalog sorted by enum name,
    /// table name, column name, foreign-key name, and index name, so the result
    /// equals the catalog that generated the snapshot. The labels of an enum keep
    /// the order they were rendered in.
    ///
    /// A snapshot that still carries the v1 header is read as well: it differs
    /// only in writing the SQL text of a column default verbatim, without the
    /// quotes and escapes the current format applies.
    pub fn from_lock(lock: &str) -> Result<SchemaCatalog, Error> {
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
}

/// How the column defaults of a snapshot are encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Defaults {
    /// A v1 snapshot writes the SQL text of a default verbatim.
    Verbatim,
    /// A v2 snapshot writes the SQL text of a default as a quoted, escaped string.
    Quoted,
}

/// Parses the `table "<name>" { ... }` block starting at `start`.
///
/// The `defaults` of the snapshot decide how the SQL text of a default is read.
fn parse_table(
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

/// Parses the `[[enum]]` section whose entries start at `start`.
///
/// The section ends at the next section header or at the end of the snapshot, so
/// the returned index points at the line that follows it.
fn parse_enum(lines: &[&str], start: usize) -> Result<(EnumDef, usize), Error> {
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
fn parse_view(lines: &[&str], start: usize) -> Result<(ViewDef, usize), Error> {
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
fn parse_trigger(lines: &[&str], start: usize) -> Result<(TriggerDef, usize), Error> {
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

/// Parses a quoted value and rejects trailing text.
fn parse_quoted_value(input: &str) -> Result<String, Error> {
    let (value, rest) = parse_quoted_prefix(input)?;
    if !rest.trim().is_empty() {
        return Err(lock_error("unexpected text after a quoted value"));
    }
    Ok(value)
}

/// Parses the `["a", "b"]` label list of an enum section.
fn parse_variant_list(input: &str) -> Result<Vec<String>, Error> {
    let mut rest = input
        .strip_prefix('[')
        .ok_or_else(|| lock_error("expected `[` to start an enum label list"))?
        .trim_start();

    let mut variants = Vec::new();
    loop {
        if let Some(empty) = rest.strip_prefix(']') {
            if !empty.trim().is_empty() {
                return Err(lock_error("unexpected text after an enum label list"));
            }
            return Ok(variants);
        }

        let (variant, after) = parse_quoted_prefix(rest)?;
        variants.push(variant);

        let after = after.trim_start();
        match after.strip_prefix(',') {
            Some(remainder) => rest = remainder.trim_start(),
            None => {
                let trailing = after
                    .strip_prefix(']')
                    .ok_or_else(|| lock_error("expected `,` or `]` after an enum label"))?;
                if !trailing.trim().is_empty() {
                    return Err(lock_error("unexpected text after an enum label list"));
                }
                return Ok(variants);
            }
        }
    }
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

/// Creates an [`Error::InvalidLockFormat`] from the given message.
fn lock_error(message: impl Into<String>) -> Error {
    Error::InvalidLockFormat {
        message: message.into(),
    }
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

/// Parses a leading `"..."` identifier, decoding doubled quotes.
fn parse_quoted_prefix(input: &str) -> Result<(String, &str), Error> {
    let mut result = String::new();
    let mut chars = input.char_indices().peekable();
    match chars.next() {
        Some((_, '"')) => {}
        _ => return Err(lock_error("expected a double-quoted identifier")),
    }
    while let Some((index, character)) = chars.next() {
        if character == '"' {
            if let Some(&(_, '"')) = chars.peek() {
                chars.next();
                result.push('"');
            } else {
                return Ok((result, &input[index + 1..]));
            }
        } else {
            result.push(character);
        }
    }
    Err(lock_error("unterminated quoted identifier"))
}

/// Parses a `("a", "b")` list of quoted identifiers.
fn parse_quoted_list(input: &str) -> Result<(Vec<String>, &str), Error> {
    let mut rest = input
        .strip_prefix('(')
        .ok_or_else(|| lock_error("expected `(` to start an identifier list"))?;
    let mut items = Vec::new();
    let trimmed = rest.trim_start();
    if let Some(after_close) = trimmed.strip_prefix(')') {
        return Ok((items, after_close));
    }
    loop {
        let (name, after) = parse_quoted_prefix(rest)?;
        items.push(name);
        if let Some(after_comma) = after.strip_prefix(", ") {
            rest = after_comma;
        } else {
            rest = after;
            break;
        }
    }
    let rest = rest
        .strip_prefix(')')
        .ok_or_else(|| lock_error("expected `)` to close an identifier list"))?;
    Ok((items, rest))
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

/// Parses the canonical type text of a column back into a [`TypeKind`].
fn parse_type_kind(text: &str) -> Result<TypeKind, Error> {
    let text = text.trim();

    // An array renders its element type followed by `[]`, while the name of an
    // enum may end with the very same characters, so the suffix only marks an
    // array when it does not leave a quoted name open.
    if let Some(element) = text.strip_suffix("[]") {
        if !has_open_quote(element) {
            return Ok(TypeKind::Array(Box::new(parse_type_kind(element)?)));
        }
    }

    if let Some(rest) = text.strip_prefix("enum ") {
        return Ok(TypeKind::Enum(parse_quoted_value(rest.trim_start())?));
    }

    Ok(match text {
        "boolean" => TypeKind::Boolean,
        "int32" => TypeKind::Int32,
        "int64" => TypeKind::Int64,
        "float64" => TypeKind::Float64,
        "numeric" => TypeKind::Numeric,
        "text" => TypeKind::Text,
        "bytes" => TypeKind::Bytes,
        "uuid" => TypeKind::Uuid,
        "json" => TypeKind::Json,
        "timestamp" => TypeKind::Timestamp,
        "date" => TypeKind::Date,
        other => TypeKind::Other(other.to_owned()),
    })
}

/// Reports whether `text` leaves a double quote open.
///
/// A doubled quote inside a name counts as two, so a complete name always has an
/// even number of quotes.
fn has_open_quote(text: &str) -> bool {
    text.chars().filter(|character| *character == '"').count() % 2 == 1
}

/// Parses a leading quoted, escaped string prefix.
fn parse_escaped_quoted_prefix(input: &str) -> Result<(String, &str), Error> {
    let mut characters = input.char_indices();

    match characters.next() {
        Some((_, '"')) => {}
        _ => return Err(lock_error("expected a double-quoted value")),
    }

    let mut value = String::new();
    let mut closing = None;

    while let Some((index, character)) = characters.next() {
        match character {
            '"' => {
                if let Some((_, '"')) = characters.clone().next() {
                    characters.next();
                    value.push('"');
                } else {
                    closing = Some(index);
                    break;
                }
            }
            '\\' => match characters.next() {
                Some((_, 'n')) => value.push('\n'),
                Some((_, 'r')) => value.push('\r'),
                Some((_, '\\')) => value.push('\\'),
                Some((_, other)) => {
                    return Err(lock_error(format!(
                        "the escape `\\{other}` is not recognised in a quoted value"
                    )))
                }
                None => return Err(lock_error("a quoted value ends inside an escape")),
            },
            other => value.push(other),
        }
    }

    let closing = closing.ok_or_else(|| lock_error("unterminated quoted value"))?;
    Ok((value, &input[closing + 1..]))
}

/// Parses a quoted, escaped default value written by a current snapshot.
fn parse_quoted_default(input: &str) -> Result<String, Error> {
    let (value, rest) = parse_escaped_quoted_prefix(input)?;
    if !rest.trim().is_empty() {
        return Err(lock_error("unexpected text after a quoted default value"));
    }
    Ok(value)
}
