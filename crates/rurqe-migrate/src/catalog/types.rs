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
    /// `NUMERIC` / `DECIMAL`.
    Numeric,
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
    /// A database enum type, named after the type it was declared as.
    Enum(String),
    /// An array whose elements have the wrapped type.
    Array(Box<TypeKind>),
    /// A type outside the built-in set, kept verbatim.
    Other(String),
}

impl TypeKind {
    /// The canonical text of the type inside `schema.lock`.
    ///
    /// The built-in variants render their canonical name, an enum renders the
    /// quoted name of its type (`enum "mood"`), and an array renders its element
    /// type followed by `[]`, so `int64[]` and `enum "mood"[]` are distinct.
    pub fn canonical(&self) -> String {
        let mut out = String::new();
        self.write_canonical(&mut out);
        out
    }

    /// Appends the canonical text of the type to `out`.
    fn write_canonical(&self, out: &mut String) {
        match self {
            TypeKind::Boolean => out.push_str("boolean"),
            TypeKind::Int32 => out.push_str("int32"),
            TypeKind::Int64 => out.push_str("int64"),
            TypeKind::Float64 => out.push_str("float64"),
            TypeKind::Numeric => out.push_str("numeric"),
            TypeKind::Text => out.push_str("text"),
            TypeKind::Bytes => out.push_str("bytes"),
            TypeKind::Uuid => out.push_str("uuid"),
            TypeKind::Json => out.push_str("json"),
            TypeKind::Timestamp => out.push_str("timestamp"),
            TypeKind::Date => out.push_str("date"),
            TypeKind::Other(raw) => out.push_str(raw),
            TypeKind::Enum(name) => {
                out.push_str("enum ");
                push_quoted(out, name);
            }
            TypeKind::Array(inner) => {
                inner.write_canonical(out);
                out.push_str("[]");
            }
        }
    }
}

impl fmt::Display for TypeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.canonical())
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

/// The action to perform when a foreign key target is deleted or updated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub enum ForeignKeyAction {
    /// Reject the deletion or update without delay (`NO ACTION`).
    #[default]
    NoAction,
    /// Reject the deletion or update immediately (`RESTRICT`).
    Restrict,
    /// Propagate the deletion or update to the referencing row (`CASCADE`).
    Cascade,
    /// Set the referencing column to NULL (`SET NULL`).
    SetNull,
    /// Set the referencing column to its default value (`SET DEFAULT`).
    SetDefault,
}

impl ForeignKeyAction {
    /// The canonical SQL text of the action.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::NoAction => "no action",
            Self::Restrict => "restrict",
            Self::Cascade => "cascade",
            Self::SetNull => "set null",
            Self::SetDefault => "set default",
        }
    }
}

impl fmt::Display for ForeignKeyAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
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
    /// The SQL schema of [`ForeignKeyDef::referenced_table`].
    ///
    /// `None` means the reference carries no attribution: the referenced table
    /// lives in the primary schema, or the catalog was read from a snapshot
    /// format that carries no schema layer. The distinction matters because a
    /// bare table name is ambiguous across schemas.
    pub referenced_schema: Option<String>,
    /// The action taken when the referenced row is deleted.
    pub on_delete: ForeignKeyAction,
    /// The action taken when the referenced row is updated.
    pub on_update: ForeignKeyAction,
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
            referenced_schema: None,
            on_delete: ForeignKeyAction::NoAction,
            on_update: ForeignKeyAction::NoAction,
        }
    }

    /// Attributes the referenced table to the SQL schema `schema`.
    pub fn with_referenced_schema(mut self, schema: impl Into<String>) -> Self {
        self.referenced_schema = Some(schema.into());
        self
    }

    /// Sets the action taken when the referenced row is deleted.
    pub fn with_on_delete(mut self, action: ForeignKeyAction) -> Self {
        self.on_delete = action;
        self
    }

    /// Sets the action taken when the referenced row is updated.
    pub fn with_on_update(mut self, action: ForeignKeyAction) -> Self {
        self.on_update = action;
        self
    }

    /// Sets both the deletion and update actions of the constraint.
    pub fn with_actions(
        mut self,
        on_delete: ForeignKeyAction,
        on_update: ForeignKeyAction,
    ) -> Self {
        self.on_delete = on_delete;
        self.on_update = on_update;
        self
    }
}

/// A check constraint of a [`TableDef`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckDef {
    /// The SQL identifier of the constraint.
    pub name: String,
    /// The SQL boolean expression that must evaluate to true.
    pub expression: String,
}

impl CheckDef {
    /// Creates a check constraint definition from its name and SQL expression.
    pub fn new(name: impl Into<String>, expression: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            expression: expression.into(),
        }
    }
}

/// A secondary index of a [`TableDef`].
///
/// Both plain and `UNIQUE` secondary indexes share this representation, so the
/// snapshot distinguishes them solely by [`IndexDef::unique`]. Primary keys are
/// not indexes in this sense and stay in [`TableDef::primary_key`].
///
/// An index over expressions records what it computes rather than the text it was
/// declared with: [`IndexDef::expression`] carries the functional expressions
/// alone, so the plain columns of a mixed index stay in [`IndexDef::columns`] and
/// never repeat as expressions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexDef {
    /// The SQL identifier of the index.
    pub name: String,
    /// The indexed columns, in declaration order.
    pub columns: Vec<String>,
    /// Whether the index is declared `UNIQUE`.
    pub unique: bool,
    /// The access method (e.g. `btree`, `gin`, `gist`), if specified.
    pub method: Option<String>,
    /// The partial index predicate (WHERE clause expression), if specified.
    pub predicate: Option<String>,
    /// The functional expressions an expression-based index computes.
    ///
    /// The plain columns of a mixed index remain in [`IndexDef::columns`] and
    /// never repeat here, and the placeholder an engine renders for such a column
    /// is dropped. An index computing several expressions keeps them in key order,
    /// separated by `, `.
    pub expression: Option<String>,
}

impl IndexDef {
    /// Creates an index definition from its name and indexed columns.
    pub fn new(name: impl Into<String>, columns: Vec<String>, unique: bool) -> Self {
        Self {
            name: name.into(),
            columns,
            unique,
            method: None,
            predicate: None,
            expression: None,
        }
    }

    /// Sets the access method of the index (e.g. `"btree"`, `"gin"`).
    pub fn with_method(mut self, method: impl Into<String>) -> Self {
        self.method = Some(method.into());
        self
    }

    /// Sets the partial index predicate (WHERE clause expression).
    pub fn with_predicate(mut self, predicate: impl Into<String>) -> Self {
        self.predicate = Some(predicate.into());
        self
    }

    /// Attaches the functional expressions an expression-based index computes.
    pub fn with_expression(mut self, expression: impl Into<String>) -> Self {
        self.expression = Some(expression.into());
        self
    }
}

/// A table of the schema together with its columns, keys, and constraints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableDef {
    /// The SQL identifier of the table.
    pub name: String,
    /// The SQL schema the table belongs to.
    ///
    /// `None` means the catalog records no attribution: the table belongs to the
    /// primary schema, or it was read from a snapshot format that carries no
    /// schema layer.
    pub schema: Option<String>,
    /// The columns of the table.
    pub columns: Vec<ColumnDef>,
    /// The columns forming the primary key, empty when the table has none.
    pub primary_key: Vec<String>,
    /// The check constraints declared on the table.
    pub checks: Vec<CheckDef>,
    /// The foreign keys declared by the table.
    pub foreign_keys: Vec<ForeignKeyDef>,
    /// The secondary and unique indexes declared on the table.
    pub indexes: Vec<IndexDef>,
}

impl TableDef {
    /// Creates a table definition without foreign keys, indexes, and checks.
    pub fn new(name: impl Into<String>, columns: Vec<ColumnDef>, primary_key: Vec<String>) -> Self {
        Self {
            name: name.into(),
            schema: None,
            columns,
            primary_key,
            checks: Vec::new(),
            foreign_keys: Vec::new(),
            indexes: Vec::new(),
        }
    }

    /// Attributes the table to the SQL schema `schema`.
    pub fn with_schema(mut self, schema: impl Into<String>) -> Self {
        self.schema = Some(schema.into());
        self
    }

    /// Appends `check` constraint to the table.
    pub fn with_check(mut self, check: CheckDef) -> Self {
        self.checks.push(check);
        self
    }

    /// Appends `foreign_key` to the table.
    pub fn with_foreign_key(mut self, foreign_key: ForeignKeyDef) -> Self {
        self.foreign_keys.push(foreign_key);
        self
    }

    /// Appends `index` to the table.
    pub fn with_index(mut self, index: IndexDef) -> Self {
        self.indexes.push(index);
        self
    }
}

/// An enum type declared by the database.
///
/// The labels keep their declared order, because the order of the labels is part
/// of the enum type itself: PostgreSQL orders the values of an enum by it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumDef {
    /// The SQL identifier of the enum type.
    pub name: String,
    /// The SQL schema the enum type belongs to.
    ///
    /// `None` means the catalog records no attribution: the enum belongs to the
    /// primary schema, or it was read from a snapshot format that carries no
    /// schema layer.
    pub schema: Option<String>,
    /// The labels of the enum, in declaration order.
    pub variants: Vec<String>,
}

impl EnumDef {
    /// Creates an enum definition from its type name and labels.
    pub fn new(name: impl Into<String>, variants: Vec<String>) -> Self {
        Self {
            name: name.into(),
            schema: None,
            variants,
        }
    }

    /// Attributes the enum type to the SQL schema `schema`.
    pub fn with_schema(mut self, schema: impl Into<String>) -> Self {
        self.schema = Some(schema.into());
        self
    }
}

/// A database view defined by an SQL query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewDef {
    /// The SQL identifier of the view.
    pub name: String,
    /// The SQL schema the view belongs to.
    ///
    /// `None` means the catalog records no attribution: the view belongs to the
    /// primary schema, or it was read from a snapshot format that carries no
    /// schema layer.
    pub schema: Option<String>,
    /// The SQL query defining the view.
    pub definition: String,
}

impl ViewDef {
    /// Creates a view definition from its name and SQL query.
    pub fn new(name: impl Into<String>, definition: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            schema: None,
            definition: definition.into(),
        }
    }

    /// Attributes the view to the SQL schema `schema`.
    pub fn with_schema(mut self, schema: impl Into<String>) -> Self {
        self.schema = Some(schema.into());
        self
    }
}

/// A database trigger attached to a table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TriggerDef {
    /// The SQL identifier of the trigger.
    pub name: String,
    /// The SQL schema the trigger belongs to.
    ///
    /// `None` means the catalog records no attribution: the trigger belongs to
    /// the primary schema, or it was read from a snapshot format that carries no
    /// schema layer.
    pub schema: Option<String>,
    /// The table the trigger is attached to.
    pub table_name: String,
    /// The SQL definition or body of the trigger.
    pub definition: String,
}

impl TriggerDef {
    /// Creates a trigger definition from its name, owning table name, and definition.
    pub fn new(
        name: impl Into<String>,
        table_name: impl Into<String>,
        definition: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            schema: None,
            table_name: table_name.into(),
            definition: definition.into(),
        }
    }

    /// Attributes the trigger to the SQL schema `schema`.
    pub fn with_schema(mut self, schema: impl Into<String>) -> Self {
        self.schema = Some(schema.into());
        self
    }
}

/// Appends `ident` to `out` as a double-quoted SQL identifier, doubling any
/// embedded quote so the rendering stays unambiguous.
pub(super) fn push_quoted(out: &mut String, ident: &str) {
    out.push('"');
    for character in ident.chars() {
        if character == '"' {
            out.push('"');
        }
        out.push(character);
    }
    out.push('"');
}
