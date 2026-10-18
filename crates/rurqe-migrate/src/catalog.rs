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
}
