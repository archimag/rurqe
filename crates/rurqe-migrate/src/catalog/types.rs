//! Engine-neutral column, table, and key definitions.

use core::fmt;

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
