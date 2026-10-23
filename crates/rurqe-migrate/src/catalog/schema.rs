//! The engine-neutral schema catalog and the `schema.lock` round trip.

use crate::checksum::Checksum;
use crate::error::Error;

use super::lock;
use super::types::{ColumnDef, EnumDef, ForeignKeyDef, IndexDef, TableDef, TypeKind};

/// The complete schema of a database as an engine-neutral catalog.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SchemaCatalog {
    /// The tables of the schema.
    pub tables: Vec<TableDef>,
    /// The enum types declared by the database.
    pub enums: Vec<EnumDef>,
}

impl SchemaCatalog {
    /// Creates a catalog from the given tables and no enum types.
    pub fn new(tables: Vec<TableDef>) -> Self {
        Self {
            tables,
            enums: Vec::new(),
        }
    }

    /// Attaches the enum types `enums` to the catalog.
    pub fn with_enums(mut self, enums: Vec<EnumDef>) -> Self {
        self.enums = enums;
        self
    }

    /// Renders the canonical `schema.lock` snapshot.
    ///
    /// Enum sections and tables, and the columns, foreign keys, and indexes of
    /// every table, are emitted sorted by name, so the snapshot depends only on
    /// the schema content and never on the declaration order of the catalog.
    pub fn to_lock(&self) -> String {
        lock::render_lock(&self.enums, &self.tables)
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
    pub fn from_lock(lock: &str) -> Result<SchemaCatalog, Error> {
        let mut lines = lock.lines();
        match lines.next() {
            Some(header) if header == lock::LOCK_HEADER => {}
            _ => {
                return Err(lock_error(format!(
                    "expected `{}` header",
                    lock::LOCK_HEADER
                )))
            }
        }

        let lines: Vec<&str> = lines.collect();
        let mut enums: Vec<EnumDef> = Vec::new();
        let mut tables: Vec<TableDef> = Vec::new();
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

            if trimmed.starts_with("table ") {
                let (table, next) = parse_table(&lines, index)?;
                tables.push(table);
                index = next;
                continue;
            }

            return Err(lock_error(
                "expected a `table` declaration or an `[[enum]]` section",
            ));
        }

        enums.sort_by(|left, right| left.name.cmp(&right.name));
        tables.sort_by(|left, right| left.name.cmp(&right.name));

        Ok(SchemaCatalog::new(tables).with_enums(enums))
    }
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
        if trimmed == lock::ENUM_HEADER || trimmed.starts_with("table ") {
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

/// Parses the `table "<name>" { ... }` block starting at `start`.
fn parse_table(lines: &[&str], start: usize) -> Result<(TableDef, usize), Error> {
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
            || trimmed.starts_with("foreign key ")
            || trimmed.starts_with("index ")
            || trimmed.starts_with("unique index ");

        if is_close || is_item {
            if !item_lines.is_empty() {
                apply_table_item(&mut table, &item_lines.join("\n"))?;
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
    foreign_keys: Vec<ForeignKeyDef>,
    indexes: Vec<IndexDef>,
}

impl TableBuilder {
    fn new(name: String) -> Self {
        Self {
            name,
            columns: Vec::new(),
            primary_key: Vec::new(),
            foreign_keys: Vec::new(),
            indexes: Vec::new(),
        }
    }

    /// Finalizes the table, sorting columns, foreign keys, and indexes by name
    /// so the result matches the canonical ordering used by the renderer.
    fn finish(mut self) -> TableDef {
        self.columns
            .sort_by(|left, right| left.name.cmp(&right.name));
        self.foreign_keys
            .sort_by(|left, right| left.name.cmp(&right.name));
        self.indexes.sort_by(|left, right| left.name.cmp(&right.name));
        TableDef {
            name: self.name,
            columns: self.columns,
            primary_key: self.primary_key,
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
fn apply_table_item(table: &mut TableBuilder, item: &str) -> Result<(), Error> {
    let item = item.trim_start();
    if item.starts_with("column ") {
        table.columns.push(parse_column(item)?);
    } else if item.starts_with("primary key ") {
        table.primary_key = parse_primary_key(item)?;
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
fn parse_column(item: &str) -> Result<ColumnDef, Error> {
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
        Some(rest[marker.start + marker.len..].to_owned())
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

/// Parses a `foreign key "<name>" (...) references "<table>" (...)` declaration.
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
    let (referenced_columns, rest) = parse_quoted_list(rest)?;
    if !rest.is_empty() {
        return Err(lock_error(
            "unexpected text after the foreign key declaration",
        ));
    }
    Ok(ForeignKeyDef {
        name,
        columns,
        referenced_table,
        referenced_columns,
    })
}

/// Parses an `index "<name>" (...)` or `unique index "<name>" (...)` declaration,
/// depending on the `unique` keyword detected by the caller.
fn parse_index(item: &str, unique: bool) -> Result<IndexDef, Error> {
    let rest = item
        .strip_prefix(if unique { "unique index " } else { "index " })
        .ok_or_else(|| lock_error("expected `index` keyword"))?;
    let (name, rest) = parse_quoted_prefix(rest)?;
    let rest = rest
        .strip_prefix(' ')
        .ok_or_else(|| lock_error("expected a space before the indexed columns"))?;
    let (columns, rest) = parse_quoted_list(rest)?;
    if !rest.is_empty() {
        return Err(lock_error("unexpected text after the index declaration"));
    }
    Ok(IndexDef::new(name, columns, unique))
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
