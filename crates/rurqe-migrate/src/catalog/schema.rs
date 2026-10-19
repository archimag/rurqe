//! The engine-neutral schema catalog and the `schema.lock` round trip.

use crate::checksum::Checksum;
use crate::error::Error;

use super::lock;
use super::types::{ColumnDef, ForeignKeyDef, TableDef, TypeKind};

/// The complete schema of a database as an engine-neutral catalog.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SchemaCatalog {
    /// The tables of the schema.
    pub tables: Vec<TableDef>,
}

impl SchemaCatalog {
    /// Creates a catalog from the given tables.
    pub fn new(tables: Vec<TableDef>) -> Self {
        Self { tables }
    }

    /// Renders the canonical `schema.lock` snapshot.
    ///
    /// Tables, columns, and foreign keys are emitted sorted by name, so the
    /// snapshot depends only on the schema content and never on the declaration
    /// order of the catalog.
    pub fn to_lock(&self) -> String {
        lock::render_lock(&self.tables)
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
    /// [`SchemaCatalog::to_lock`] and returns the catalog sorted by table,
    /// column, and foreign-key name, so the result equals the catalog that
    /// generated the snapshot.
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

        let mut tables: Vec<TableDef> = Vec::new();
        let mut current: Option<TableBuilder> = None;
        // Buffer of raw lines belonging to the body item being parsed. A
        // column default may span multiple lines, so items are collected until
        // the next item keyword or the closing brace is reached.
        let mut item_lines: Vec<String> = Vec::new();

        for line in lines {
            let trimmed = line.trim_start();

            if current.is_none() {
                if trimmed.is_empty() {
                    continue;
                }
                if trimmed.starts_with("table ") {
                    current = Some(TableBuilder::new(parse_table_header(trimmed)?));
                    continue;
                }
                return Err(lock_error("expected a `table` declaration between tables"));
            }

            if trimmed.is_empty() {
                if item_lines.is_empty() {
                    return Err(lock_error("unexpected blank line inside a table block"));
                }
                item_lines.push(line.to_owned());
                continue;
            }

            let is_close = trimmed == "}";
            let is_item = trimmed.starts_with("column ")
                || trimmed.starts_with("primary key ")
                || trimmed.starts_with("foreign key ");

            if is_close || is_item {
                if !item_lines.is_empty() {
                    let item = item_lines.join("\n");
                    let table = current.as_mut().ok_or_else(|| {
                        lock_error("expected a `table` declaration before a table-body item")
                    })?;
                    apply_table_item(table, &item)?;
                    item_lines.clear();
                }
                if is_close {
                    let table = current
                        .take()
                        .ok_or_else(|| lock_error("unexpected `}` outside a table block"))?;
                    tables.push(table.finish());
                    continue;
                }
                item_lines.push(line.to_owned());
            } else {
                if item_lines.is_empty() {
                    return Err(lock_error("unexpected line inside a table block"));
                }
                item_lines.push(line.to_owned());
            }
        }

        if current.is_some() {
            return Err(lock_error("unterminated table block: missing `}`"));
        }

        tables.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(SchemaCatalog::new(tables))
    }
}

/// Builds a [`TableDef`] while a `table` block is being parsed.
struct TableBuilder {
    name: String,
    columns: Vec<ColumnDef>,
    primary_key: Vec<String>,
    foreign_keys: Vec<ForeignKeyDef>,
}

impl TableBuilder {
    fn new(name: String) -> Self {
        Self {
            name,
            columns: Vec::new(),
            primary_key: Vec::new(),
            foreign_keys: Vec::new(),
        }
    }

    /// Finalizes the table, sorting columns and foreign keys by name so the
    /// result matches the canonical ordering used by the renderer.
    fn finish(mut self) -> TableDef {
        self.columns
            .sort_by(|left, right| left.name.cmp(&right.name));
        self.foreign_keys
            .sort_by(|left, right| left.name.cmp(&right.name));
        TableDef {
            name: self.name,
            columns: self.columns,
            primary_key: self.primary_key,
            foreign_keys: self.foreign_keys,
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
    let type_kind = type_kind_from_str(&rest[..marker.start]);
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
        return Err(lock_error("unexpected text after the primary key declaration"));
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
        return Err(lock_error("unexpected text after the foreign key declaration"));
    }
    Ok(ForeignKeyDef {
        name,
        columns,
        referenced_table,
        referenced_columns,
    })
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

/// Maps canonical type text back to a [`TypeKind`], falling back to
/// [`TypeKind::Other`] for unknown engines' types.
fn type_kind_from_str(text: &str) -> TypeKind {
    match text {
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
    }
}
