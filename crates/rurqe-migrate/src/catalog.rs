//! Engine-neutral schema catalog and its canonical `schema.lock` snapshot.
//!
//! The catalog is the single source of truth shared by introspection, code
//! generation, and drift verification: it records tables, columns, primary keys,
//! and foreign keys in an engine-independent form and renders them into a
//! deterministic textual snapshot.
//!
//! Determinism is achieved by sorting tables, columns, and foreign keys by name,
//! so two catalogs built in a different declaration order still produce the very
//! same snapshot — and therefore the very same [`SchemaCatalog::schema_hash`].
//!
//! ```
//! use rurqe_migrate::{ColumnDef, SchemaCatalog, TableDef, TypeKind};
//!
//! let catalog = SchemaCatalog::new(vec![TableDef::new(
//!     "users",
//!     vec![
//!         ColumnDef::new("id", TypeKind::Int64, false),
//!         ColumnDef::new("email", TypeKind::Text, false),
//!     ],
//!     vec!["id".to_owned()],
//! )]);
//!
//! assert_eq!(
//!     catalog.to_lock(),
//!     concat!(
//!         "schema lock v1\n",
//!         "\n",
//!         "table \"users\" {\n",
//!         "  column \"email\": text not null\n",
//!         "  column \"id\": int64 not null\n",
//!         "  primary key (\"id\")\n",
//!         "}\n",
//!     )
//! );
//!
//! assert_eq!(catalog.schema_hash().len(), 64);
//! ```

use core::fmt;

use crate::checksum::Checksum;
use crate::error::Error;

/// Version marker written into every `schema.lock` snapshot.
const LOCK_HEADER: &str = "schema lock v1";

/// The engine-neutral SQL type of a column.
///
/// The variants cover the built-in types; anything else is preserved verbatim
/// through [`TypeKind::Other`] so an unfamiliar engine type still reaches the
/// snapshot instead of being silently dropped.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TypeKind {
    /// `BOOLEAN`.
    Boolean,
    /// `INTEGER` / `INT4`.
    Int32,
    /// `BIGINT` / `INT8`.
    Int64,
    /// `DOUBLE PRECISION` / `FLOAT8`.
    Float64,
    /// `TEXT` / `VARCHAR`.
    Text,
    /// `BYTEA` / `BLOB`.
    Bytes,
    /// `UUID`.
    Uuid,
    /// `JSON` / `JSONB`.
    Json,
    /// `TIMESTAMP` / `TIMESTAMPTZ`.
    Timestamp,
    /// `DATE`.
    Date,
    /// A type outside the built-in set, kept verbatim.
    Other(String),
}

impl TypeKind {
    /// The canonical text of the type inside `schema.lock`.
    pub fn as_str(&self) -> &str {
        match self {
            TypeKind::Boolean => "boolean",
            TypeKind::Int32 => "int32",
            TypeKind::Int64 => "int64",
            TypeKind::Float64 => "float64",
            TypeKind::Text => "text",
            TypeKind::Bytes => "bytes",
            TypeKind::Uuid => "uuid",
            TypeKind::Json => "json",
            TypeKind::Timestamp => "timestamp",
            TypeKind::Date => "date",
            TypeKind::Other(raw) => raw,
        }
    }
}

impl fmt::Display for TypeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A column of a [`TableDef`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnDef {
    /// The SQL identifier of the column.
    pub name: String,
    /// The engine-neutral type of the column.
    pub type_kind: TypeKind,
    /// Whether the column accepts `NULL`.
    pub nullable: bool,
    /// The SQL default expression, when the column declares one.
    pub default: Option<String>,
}

impl ColumnDef {
    /// Creates a column definition without a default expression.
    pub fn new(name: impl Into<String>, type_kind: TypeKind, nullable: bool) -> Self {
        Self {
            name: name.into(),
            type_kind,
            nullable,
            default: None,
        }
    }

    /// Attaches the SQL default expression `default` to the column.
    pub fn with_default(mut self, default: impl Into<String>) -> Self {
        self.default = Some(default.into());
        self
    }
}

/// A foreign key constraint of a [`TableDef`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForeignKeyDef {
    /// The SQL identifier of the constraint.
    pub name: String,
    /// The constrained columns of the owning table.
    pub columns: Vec<String>,
    /// The referenced table.
    pub referenced_table: String,
    /// The referenced columns of [`ForeignKeyDef::referenced_table`].
    pub referenced_columns: Vec<String>,
}

impl ForeignKeyDef {
    /// Creates a foreign key definition from its constraint name and column pairs.
    pub fn new(
        name: impl Into<String>,
        columns: Vec<String>,
        referenced_table: impl Into<String>,
        referenced_columns: Vec<String>,
    ) -> Self {
        Self {
            name: name.into(),
            columns,
            referenced_table: referenced_table.into(),
            referenced_columns,
        }
    }
}

/// A table of the schema together with its columns and keys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableDef {
    /// The SQL identifier of the table.
    pub name: String,
    /// The columns of the table.
    pub columns: Vec<ColumnDef>,
    /// The columns forming the primary key, empty when the table has none.
    pub primary_key: Vec<String>,
    /// The foreign keys declared by the table.
    pub foreign_keys: Vec<ForeignKeyDef>,
}

impl TableDef {
    /// Creates a table definition without foreign keys.
    pub fn new(
        name: impl Into<String>,
        columns: Vec<ColumnDef>,
        primary_key: Vec<String>,
    ) -> Self {
        Self {
            name: name.into(),
            columns,
            primary_key,
            foreign_keys: Vec::new(),
        }
    }

    /// Appends `foreign_key` to the table.
    pub fn with_foreign_key(mut self, foreign_key: ForeignKeyDef) -> Self {
        self.foreign_keys.push(foreign_key);
        self
    }
}

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
        let mut out = String::new();
        out.push_str(LOCK_HEADER);
        out.push('\n');

        let mut tables: Vec<&TableDef> = self.tables.iter().collect();
        tables.sort_by(|left, right| left.name.cmp(&right.name));

        for table in tables {
            out.push('\n');
            out.push_str("table ");
            push_quoted(&mut out, &table.name);
            out.push_str(" {\n");

            let mut columns: Vec<&ColumnDef> = table.columns.iter().collect();
            columns.sort_by(|left, right| left.name.cmp(&right.name));
            for column in columns {
                out.push_str("  column ");
                push_quoted(&mut out, &column.name);
                out.push_str(": ");
                out.push_str(column.type_kind.as_str());
                out.push_str(if column.nullable { " null" } else { " not null" });
                if let Some(default) = &column.default {
                    out.push_str(" default ");
                    out.push_str(default);
                }
                out.push('\n');
            }

            if !table.primary_key.is_empty() {
                out.push_str("  primary key ");
                push_quoted_list(&mut out, &table.primary_key);
                out.push('\n');
            }

            let mut foreign_keys: Vec<&ForeignKeyDef> = table.foreign_keys.iter().collect();
            foreign_keys.sort_by(|left, right| left.name.cmp(&right.name));
            for foreign_key in foreign_keys {
                out.push_str("  foreign key ");
                push_quoted(&mut out, &foreign_key.name);
                out.push(' ');
                push_quoted_list(&mut out, &foreign_key.columns);
                out.push_str(" references ");
                push_quoted(&mut out, &foreign_key.referenced_table);
                out.push(' ');
                push_quoted_list(&mut out, &foreign_key.referenced_columns);
                out.push('\n');
            }

            out.push_str("}\n");
        }

        out
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
            Some(header) if header == LOCK_HEADER => {}
            _ => return Err(lock_error(format!("expected `{LOCK_HEADER}` header"))),
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
                    apply_table_item(current.as_mut().expect("open table"), &item)?;
                    item_lines.clear();
                }
                if is_close {
                    tables.push(current.take().expect("open table").finish());
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
    /// result matches the canonical ordering used by [`SchemaCatalog::to_lock`].
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

/// Appends `ident` to `out` as a double-quoted SQL identifier, doubling any
/// embedded quote so the rendering stays unambiguous.
fn push_quoted(out: &mut String, ident: &str) {
    out.push('"');
    for character in ident.chars() {
        if character == '"' {
            out.push('"');
        }
        out.push(character);
    }
    out.push('"');
}

/// Appends a parenthesised, comma-separated list of quoted identifiers.
fn push_quoted_list(out: &mut String, idents: &[String]) {
    out.push('(');
    for (index, ident) in idents.iter().enumerate() {
        if index > 0 {
            out.push_str(", ");
        }
        push_quoted(out, ident);
    }
    out.push(')');
}

#[cfg(test)]
mod tests {
    use super::*;

    fn users() -> TableDef {
        TableDef::new(
            "users",
            vec![
                ColumnDef::new("id", TypeKind::Int64, false),
                ColumnDef::new("email", TypeKind::Text, false),
            ],
            vec!["id".to_owned()],
        )
    }

    fn orders() -> TableDef {
        TableDef::new(
            "orders",
            vec![
                ColumnDef::new("id", TypeKind::Int64, false),
                ColumnDef::new("total", TypeKind::Float64, true).with_default("0"),
                ColumnDef::new("user_id", TypeKind::Int64, false),
            ],
            vec!["id".to_owned()],
        )
        .with_foreign_key(ForeignKeyDef::new(
            "orders_user_id_fkey",
            vec!["user_id".to_owned()],
            "users",
            vec!["id".to_owned()],
        ))
    }

    const EXPECTED_LOCK: &str = concat!(
        "schema lock v1\n",
        "\n",
        "table \"orders\" {\n",
        "  column \"id\": int64 not null\n",
        "  column \"total\": float64 null default 0\n",
        "  column \"user_id\": int64 not null\n",
        "  primary key (\"id\")\n",
        "  foreign key \"orders_user_id_fkey\" (\"user_id\") references \"users\" (\"id\")\n",
        "}\n",
        "\n",
        "table \"users\" {\n",
        "  column \"email\": text not null\n",
        "  column \"id\": int64 not null\n",
        "  primary key (\"id\")\n",
        "}\n",
    );

    #[test]
    fn lock_renders_tables_columns_and_keys() {
        let catalog = SchemaCatalog::new(vec![users(), orders()]);

        assert_eq!(catalog.to_lock(), EXPECTED_LOCK);
    }

    #[test]
    fn lock_is_independent_of_declaration_order() {
        let reordered_orders = TableDef::new(
            "orders",
            vec![
                ColumnDef::new("user_id", TypeKind::Int64, false),
                ColumnDef::new("total", TypeKind::Float64, true).with_default("0"),
                ColumnDef::new("id", TypeKind::Int64, false),
            ],
            vec!["id".to_owned()],
        )
        .with_foreign_key(ForeignKeyDef::new(
            "orders_user_id_fkey",
            vec!["user_id".to_owned()],
            "users",
            vec!["id".to_owned()],
        ));

        let catalog = SchemaCatalog::new(vec![reordered_orders, users()]);

        assert_eq!(catalog.to_lock(), EXPECTED_LOCK);
        assert_eq!(catalog.schema_hash(), SchemaCatalog::new(vec![users(), orders()]).schema_hash());
    }

    #[test]
    fn identifiers_are_quoted_and_escaped() {
        let catalog = SchemaCatalog::new(vec![TableDef::new(
            "we\"ird",
            vec![ColumnDef::new("select", TypeKind::Other("citext".to_owned()), true)],
            Vec::new(),
        )]);

        assert_eq!(
            catalog.to_lock(),
            concat!(
                "schema lock v1\n",
                "\n",
                "table \"we\"\"ird\" {\n",
                "  column \"select\": citext null\n",
                "}\n",
            )
        );
    }

    #[test]
    fn empty_catalog_renders_only_the_header() {
        assert_eq!(SchemaCatalog::default().to_lock(), "schema lock v1\n");
    }

    #[test]
    fn schema_hash_follows_the_canonical_snapshot() {
        let catalog = SchemaCatalog::new(vec![users()]);

        assert_eq!(catalog.checksum(), Checksum::of(catalog.to_lock().as_bytes()));
        assert_eq!(catalog.schema_hash(), catalog.checksum().to_hex());
        assert_eq!(catalog.schema_hash().len(), 64);
    }

    #[test]
    fn hash_const_embeds_the_schema_hash() {
        let catalog = SchemaCatalog::new(vec![users()]);

        assert_eq!(
            catalog.hash_const(),
            format!("pub const SCHEMA_HASH: &str = \"{}\";", catalog.schema_hash())
        );
    }

    #[test]
    fn type_kinds_render_canonical_names() {
        let kinds = [
            TypeKind::Boolean,
            TypeKind::Int32,
            TypeKind::Int64,
            TypeKind::Float64,
            TypeKind::Text,
            TypeKind::Bytes,
            TypeKind::Uuid,
            TypeKind::Json,
            TypeKind::Timestamp,
            TypeKind::Date,
        ];

        let rendered: Vec<&str> = kinds.iter().map(TypeKind::as_str).collect();

        assert_eq!(
            rendered,
            vec![
                "boolean", "int32", "int64", "float64", "text", "bytes", "uuid", "json",
                "timestamp", "date",
            ]
        );
        assert_eq!(TypeKind::Uuid.to_string(), "uuid");
        assert_eq!(TypeKind::Other("inet".to_owned()).to_string(), "inet");
    }

    #[test]
    fn from_lock_round_trips_the_canonical_snapshot() {
        let catalog = SchemaCatalog::new(vec![
            TableDef::new(
                "orders",
                vec![
                    ColumnDef::new("id", TypeKind::Int64, false),
                    ColumnDef::new("total", TypeKind::Float64, true).with_default("0"),
                    ColumnDef::new("user_id", TypeKind::Int64, false),
                ],
                vec!["id".to_owned()],
            )
            .with_foreign_key(ForeignKeyDef::new(
                "orders_user_id_fkey",
                vec!["user_id".to_owned()],
                "users",
                vec!["id".to_owned()],
            )),
            TableDef::new(
                "users",
                vec![
                    ColumnDef::new("email", TypeKind::Text, false),
                    ColumnDef::new("id", TypeKind::Int64, false),
                ],
                vec!["id".to_owned()],
            ),
        ]);

        let reparsed = SchemaCatalog::from_lock(&catalog.to_lock()).unwrap();

        assert_eq!(reparsed, catalog);
    }

    #[test]
    fn from_lock_decodes_escaped_identifiers() {
        let catalog = SchemaCatalog::new(vec![TableDef::new(
            "we\"ird",
            vec![ColumnDef::new(
                "select",
                TypeKind::Other("citext".to_owned()),
                true,
            )],
            Vec::new(),
        )]);

        let reparsed = SchemaCatalog::from_lock(&catalog.to_lock()).unwrap();

        assert_eq!(reparsed, catalog);
    }

    #[test]
    fn from_lock_round_trips_multi_line_defaults() {
        let catalog = SchemaCatalog::new(vec![TableDef::new(
            "events",
            vec![ColumnDef::new("payload", TypeKind::Text, true)
                .with_default("line one\nline two")],
            Vec::new(),
        )]);

        let reparsed = SchemaCatalog::from_lock(&catalog.to_lock()).unwrap();

        assert_eq!(reparsed, catalog);
    }

    #[test]
    fn from_lock_round_trips_defaults_containing_keywords() {
        let catalog = SchemaCatalog::new(vec![TableDef::new(
            "settings",
            vec![
                ColumnDef::new("alpha", TypeKind::Text, false).with_default("'default null'"),
                ColumnDef::new("beta", TypeKind::Text, true).with_default("null default"),
            ],
            Vec::new(),
        )]);

        let reparsed = SchemaCatalog::from_lock(&catalog.to_lock()).unwrap();

        assert_eq!(reparsed, catalog);
    }

    #[test]
    fn from_lock_rejects_a_missing_or_wrong_header() {
        assert!(SchemaCatalog::from_lock("").is_err());
        assert!(SchemaCatalog::from_lock("schema lock v2\n").is_err());
        assert!(SchemaCatalog::from_lock("schema lock v1").is_ok());
    }

    #[test]
    fn from_lock_rejects_an_unterminated_table_block() {
        let lock = "schema lock v1\n\ntable \"t\" {\n  column \"id\": int64 not null\n";
        assert!(SchemaCatalog::from_lock(lock).is_err());
    }

    #[test]
    fn from_lock_rejects_unknown_table_body_lines() {
        let lock = "schema lock v1\n\ntable \"t\" {\n  nonsense\n}\n";
        assert!(SchemaCatalog::from_lock(lock).is_err());
    }

    #[test]
    fn from_lock_rejects_columns_without_a_nullability_marker() {
        let lock = "schema lock v1\n\ntable \"t\" {\n  column \"id\": int64\n}\n";
        assert!(SchemaCatalog::from_lock(lock).is_err());
    }
}
