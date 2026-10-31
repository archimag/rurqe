//! Versioned TOML v4 DTOs for the `schema.lock` snapshot.
//!
//! The v4 format replaces the bespoke text grammar of the v1–v3 snapshots with
//! strongly typed TOML: every section is a struct carrying
//! `#[serde(deny_unknown_fields)]`, so an unrecognized key or a misspelled
//! field is rejected instead of silently ignored. [`render_lock_toml`]
//! serializes a [`SchemaCatalog`] into the document and [`parse_lock_toml`]
//! reads one back, and both apply the same canonical ordering — enums, tables,
//! views, and triggers sorted by name, and each table's columns, checks,
//! foreign keys, and indexes sorted by name as well — so a snapshot depends
//! only on the schema content and never on declaration order.

use serde::{Deserialize, Serialize};

use crate::catalog::types::{
    CheckDef, ColumnDef, EnumDef, ForeignKeyAction, ForeignKeyDef, IndexDef, TableDef, TriggerDef,
    TypeKind, ViewDef,
};
use crate::error::Error;

use super::SchemaCatalog;

/// The version of the TOML lock format accepted by this reader.
const LOCK_VERSION: u32 = 4;

/// The root of a v4 `schema.lock` snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LockFile {
    /// The version of the lock format, always `4`.
    version: u32,
    /// The enum types declared by the schema.
    ///
    /// The section collections default to empty, so a snapshot that omits an
    /// empty section instead of writing it out still round-trips.
    #[serde(default)]
    enums: Vec<EnumLock>,
    /// The tables of the schema.
    #[serde(default)]
    tables: Vec<TableLock>,
    /// The views declared by the schema.
    #[serde(default)]
    views: Vec<ViewLock>,
    /// The triggers declared by the schema.
    #[serde(default)]
    triggers: Vec<TriggerLock>,
}

/// A table together with its columns, keys, and constraints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TableLock {
    /// The SQL identifier of the table.
    name: String,
    /// The columns of the table.
    columns: Vec<ColumnLock>,
    /// The columns forming the primary key, empty when the table has none.
    #[serde(default)]
    primary_key: Vec<String>,
    /// The check constraints declared on the table.
    checks: Vec<CheckLock>,
    /// The foreign keys declared by the table.
    foreign_keys: Vec<ForeignKeyLock>,
    /// The secondary and unique indexes declared on the table.
    indexes: Vec<IndexLock>,
}

/// A column of a [`TableLock`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ColumnLock {
    /// The SQL identifier of the column.
    name: String,
    /// The engine-neutral type of the column.
    r#type: TypeLock,
    /// Whether the column accepts `NULL`.
    nullable: bool,
    /// The SQL default expression, when the column declares one.
    default: Option<String>,
}

/// The engine-neutral SQL type of a column, in its TOML representation.
///
/// The built-in kinds render as their canonical names, a database enum as
/// `{ enum = { name = "..." } }`, an array as `{ array = { element = ... } }`,
/// and anything else as `{ other = { name = "..." } }`, so the mapping against
/// [`TypeKind`] is total in both directions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TypeLock {
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
    #[serde(rename = "enum")]
    Enum {
        /// The name of the enum type.
        name: String,
    },
    /// An array whose elements have the wrapped type.
    Array {
        /// The element type of the array.
        element: Box<TypeLock>,
    },
    /// A type outside the built-in set, kept verbatim.
    Other {
        /// The verbatim name of the type.
        name: String,
    },
}

/// A foreign key constraint, in its TOML representation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ForeignKeyLock {
    /// The SQL identifier of the constraint.
    name: String,
    /// The constrained columns of the owning table.
    columns: Vec<String>,
    /// The referenced table.
    referenced_table: String,
    /// The referenced columns of `referenced_table`.
    referenced_columns: Vec<String>,
    /// The action taken when the referenced row is deleted.
    on_delete: FkActionLock,
    /// The action taken when the referenced row is updated.
    on_update: FkActionLock,
}

/// The action of a foreign key, in its TOML representation.
///
/// The variants are renamed to the canonical strings of
/// [`ForeignKeyAction::as_str`], so `on_delete = "set null"` reads and writes
/// exactly like the legacy snapshot spells it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum FkActionLock {
    /// Reject the deletion or update without delay (`NO ACTION`).
    #[serde(rename = "no action")]
    NoAction,
    /// Reject the deletion or update immediately (`RESTRICT`).
    #[serde(rename = "restrict")]
    Restrict,
    /// Propagate the deletion or update to the referencing row (`CASCADE`).
    #[serde(rename = "cascade")]
    Cascade,
    /// Set the referencing column to NULL (`SET NULL`).
    #[serde(rename = "set null")]
    SetNull,
    /// Set the referencing column to its default value (`SET DEFAULT`).
    #[serde(rename = "set default")]
    SetDefault,
}

/// A check constraint, in its TOML representation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CheckLock {
    /// The SQL identifier of the constraint.
    name: String,
    /// The SQL boolean expression that must evaluate to true.
    expression: String,
}

/// A secondary index, in its TOML representation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct IndexLock {
    /// The SQL identifier of the index.
    name: String,
    /// The indexed columns, in declaration order.
    columns: Vec<String>,
    /// Whether the index is declared `UNIQUE`.
    unique: bool,
    /// The access method (e.g. `btree`), if specified.
    method: Option<String>,
    /// The partial index predicate, if specified.
    predicate: Option<String>,
    /// The expression of an expression-based index, if specified.
    expression: Option<String>,
}

/// An enum type, in its TOML representation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EnumLock {
    /// The SQL identifier of the enum type.
    name: String,
    /// The labels of the enum, in declaration order.
    variants: Vec<String>,
}

/// A view, in its TOML representation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ViewLock {
    /// The SQL identifier of the view.
    name: String,
    /// The SQL query defining the view.
    definition: String,
}

/// A trigger, in its TOML representation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TriggerLock {
    /// The SQL identifier of the trigger.
    name: String,
    /// The table the trigger is attached to.
    table: String,
    /// The SQL definition or body of the trigger.
    definition: String,
}

impl From<&TypeKind> for TypeLock {
    fn from(kind: &TypeKind) -> Self {
        match kind {
            TypeKind::Boolean => Self::Boolean,
            TypeKind::Int32 => Self::Int32,
            TypeKind::Int64 => Self::Int64,
            TypeKind::Float64 => Self::Float64,
            TypeKind::Numeric => Self::Numeric,
            TypeKind::Text => Self::Text,
            TypeKind::Bytes => Self::Bytes,
            TypeKind::Uuid => Self::Uuid,
            TypeKind::Json => Self::Json,
            TypeKind::Timestamp => Self::Timestamp,
            TypeKind::Date => Self::Date,
            TypeKind::Enum(name) => Self::Enum { name: name.clone() },
            TypeKind::Array(element) => Self::Array {
                element: Box::new(TypeLock::from(element.as_ref())),
            },
            TypeKind::Other(name) => Self::Other { name: name.clone() },
        }
    }
}

impl From<TypeLock> for TypeKind {
    fn from(kind: TypeLock) -> Self {
        match kind {
            TypeLock::Boolean => Self::Boolean,
            TypeLock::Int32 => Self::Int32,
            TypeLock::Int64 => Self::Int64,
            TypeLock::Float64 => Self::Float64,
            TypeLock::Numeric => Self::Numeric,
            TypeLock::Text => Self::Text,
            TypeLock::Bytes => Self::Bytes,
            TypeLock::Uuid => Self::Uuid,
            TypeLock::Json => Self::Json,
            TypeLock::Timestamp => Self::Timestamp,
            TypeLock::Date => Self::Date,
            TypeLock::Enum { name } => Self::Enum(name),
            TypeLock::Array { element } => Self::Array(Box::new(TypeKind::from(*element))),
            TypeLock::Other { name } => Self::Other(name),
        }
    }
}

impl From<ForeignKeyAction> for FkActionLock {
    fn from(action: ForeignKeyAction) -> Self {
        match action {
            ForeignKeyAction::NoAction => Self::NoAction,
            ForeignKeyAction::Restrict => Self::Restrict,
            ForeignKeyAction::Cascade => Self::Cascade,
            ForeignKeyAction::SetNull => Self::SetNull,
            ForeignKeyAction::SetDefault => Self::SetDefault,
        }
    }
}

impl From<FkActionLock> for ForeignKeyAction {
    fn from(action: FkActionLock) -> Self {
        match action {
            FkActionLock::NoAction => Self::NoAction,
            FkActionLock::Restrict => Self::Restrict,
            FkActionLock::Cascade => Self::Cascade,
            FkActionLock::SetNull => Self::SetNull,
            FkActionLock::SetDefault => Self::SetDefault,
        }
    }
}

impl From<&ColumnDef> for ColumnLock {
    fn from(column: &ColumnDef) -> Self {
        Self {
            name: column.name.clone(),
            r#type: TypeLock::from(&column.type_kind),
            nullable: column.nullable,
            default: column.default.clone(),
        }
    }
}

impl From<&CheckDef> for CheckLock {
    fn from(check: &CheckDef) -> Self {
        Self {
            name: check.name.clone(),
            expression: check.expression.clone(),
        }
    }
}

impl From<&ForeignKeyDef> for ForeignKeyLock {
    fn from(foreign_key: &ForeignKeyDef) -> Self {
        Self {
            name: foreign_key.name.clone(),
            columns: foreign_key.columns.clone(),
            referenced_table: foreign_key.referenced_table.clone(),
            referenced_columns: foreign_key.referenced_columns.clone(),
            on_delete: FkActionLock::from(foreign_key.on_delete),
            on_update: FkActionLock::from(foreign_key.on_update),
        }
    }
}

impl From<&IndexDef> for IndexLock {
    fn from(index: &IndexDef) -> Self {
        Self {
            name: index.name.clone(),
            columns: index.columns.clone(),
            unique: index.unique,
            method: index.method.clone(),
            predicate: index.predicate.clone(),
            expression: index.expression.clone(),
        }
    }
}

impl From<&EnumDef> for EnumLock {
    fn from(definition: &EnumDef) -> Self {
        Self {
            name: definition.name.clone(),
            variants: definition.variants.clone(),
        }
    }
}

impl From<&ViewDef> for ViewLock {
    fn from(view: &ViewDef) -> Self {
        Self {
            name: view.name.clone(),
            definition: view.definition.clone(),
        }
    }
}

impl From<&TriggerDef> for TriggerLock {
    fn from(trigger: &TriggerDef) -> Self {
        Self {
            name: trigger.name.clone(),
            table: trigger.table_name.clone(),
            definition: trigger.definition.clone(),
        }
    }
}

impl From<&TableDef> for TableLock {
    /// Converts a table into its TOML DTO, sorting the columns, checks,
    /// foreign keys, and indexes by name — the canonical order the reader
    /// applies as well.
    fn from(table: &TableDef) -> Self {
        let mut columns: Vec<ColumnLock> = table.columns.iter().map(ColumnLock::from).collect();
        columns.sort_by(|left, right| left.name.cmp(&right.name));

        let mut checks: Vec<CheckLock> = table.checks.iter().map(CheckLock::from).collect();
        checks.sort_by(|left, right| left.name.cmp(&right.name));

        let mut foreign_keys: Vec<ForeignKeyLock> = table
            .foreign_keys
            .iter()
            .map(ForeignKeyLock::from)
            .collect();
        foreign_keys.sort_by(|left, right| left.name.cmp(&right.name));

        let mut indexes: Vec<IndexLock> = table.indexes.iter().map(IndexLock::from).collect();
        indexes.sort_by(|left, right| left.name.cmp(&right.name));

        Self {
            name: table.name.clone(),
            columns,
            primary_key: table.primary_key.clone(),
            checks,
            foreign_keys,
            indexes,
        }
    }
}

impl From<&SchemaCatalog> for LockFile {
    /// Converts a catalog into its TOML DTO, sorting the enums, tables, views,
    /// and triggers by name — the canonical order the reader applies as well.
    /// The labels of an enum keep their declaration order, because the order
    /// of the labels is part of the enum type itself.
    fn from(catalog: &SchemaCatalog) -> Self {
        let mut enums: Vec<EnumLock> = catalog.enums.iter().map(EnumLock::from).collect();
        enums.sort_by(|left, right| left.name.cmp(&right.name));

        let mut tables: Vec<TableLock> = catalog.tables.iter().map(TableLock::from).collect();
        tables.sort_by(|left, right| left.name.cmp(&right.name));

        let mut views: Vec<ViewLock> = catalog.views.iter().map(ViewLock::from).collect();
        views.sort_by(|left, right| left.name.cmp(&right.name));

        let mut triggers: Vec<TriggerLock> =
            catalog.triggers.iter().map(TriggerLock::from).collect();
        triggers.sort_by(|left, right| left.name.cmp(&right.name));

        Self {
            version: LOCK_VERSION,
            enums,
            tables,
            views,
            triggers,
        }
    }
}

/// Renders the canonical versioned TOML v4 `schema.lock` snapshot.
///
/// The catalog is serialized through the strongly typed DTOs above, which
/// apply the same canonical ordering [`parse_lock_toml`] applies while
/// reading, so two catalogs that differ only in declaration order render the
/// very same document — and therefore the very same
/// [`SchemaCatalog::schema_hash`].
///
/// Serializing the DTOs is total in practice: every value is a string, a
/// number, a boolean, or a table of those, none of which the `toml` writer
/// refuses. Should a serialization failure surface anyway, the fallback
/// document below is emitted instead — a comment-only document the reader
/// rejects, so a failed render can never silently publish an empty-but-valid
/// catalog.
pub(super) fn render_lock_toml(catalog: &SchemaCatalog) -> String {
    let file = LockFile::from(catalog);

    toml::to_string(&file)
        .unwrap_or_else(|error| format!("# schema lock rendering failed: {error}\n"))
}

/// Parses a versioned TOML v4 `schema.lock` snapshot into a catalog.
///
/// The snapshot is deserialized through the strongly typed DTOs above, so a
/// key the format does not define is rejected by
/// `#[serde(deny_unknown_fields)]` instead of being silently ignored, and the
/// positioned diagnostic of the `toml` reader is preserved inside
/// [`Error::InvalidLockFormat`]. The built catalog applies the same canonical
/// ordering the legacy reader produces: enums, tables, views, and triggers are
/// sorted by name, and each table's columns, checks, foreign keys, and indexes
/// are sorted by name as well.
pub(super) fn parse_lock_toml(contents: &str) -> Result<SchemaCatalog, Error> {
    let file: LockFile = toml::from_str(contents).map_err(|error| Error::InvalidLockFormat {
        message: error.to_string(),
    })?;

    if file.version != LOCK_VERSION {
        return Err(Error::InvalidLockFormat {
            message: format!(
                "unsupported schema lock version `{}`: expected `version = {LOCK_VERSION}`",
                file.version
            ),
        });
    }

    let mut enums: Vec<EnumDef> = file
        .enums
        .into_iter()
        .map(|parsed| EnumDef::new(parsed.name, parsed.variants))
        .collect();
    enums.sort_by(|left, right| left.name.cmp(&right.name));

    let mut tables: Vec<TableDef> = file.tables.into_iter().map(table_def).collect();
    tables.sort_by(|left, right| left.name.cmp(&right.name));

    let mut views: Vec<ViewDef> = file
        .views
        .into_iter()
        .map(|parsed| ViewDef::new(parsed.name, parsed.definition))
        .collect();
    views.sort_by(|left, right| left.name.cmp(&right.name));

    let mut triggers: Vec<TriggerDef> = file
        .triggers
        .into_iter()
        .map(|parsed| TriggerDef::new(parsed.name, parsed.table, parsed.definition))
        .collect();
    triggers.sort_by(|left, right| left.name.cmp(&right.name));

    Ok(SchemaCatalog::new(tables)
        .with_enums(enums)
        .with_views(views)
        .with_triggers(triggers))
}

/// Converts a parsed table into its catalog definition, sorting the columns,
/// checks, foreign keys, and indexes by name.
fn table_def(table: TableLock) -> TableDef {
    let mut columns: Vec<ColumnDef> = table
        .columns
        .into_iter()
        .map(|column| ColumnDef {
            name: column.name,
            type_kind: column.r#type.into(),
            nullable: column.nullable,
            default: column.default,
        })
        .collect();
    columns.sort_by(|left, right| left.name.cmp(&right.name));

    let mut checks: Vec<CheckDef> = table
        .checks
        .into_iter()
        .map(|check| CheckDef::new(check.name, check.expression))
        .collect();
    checks.sort_by(|left, right| left.name.cmp(&right.name));

    let mut foreign_keys: Vec<ForeignKeyDef> = table
        .foreign_keys
        .into_iter()
        .map(foreign_key_def)
        .collect();
    foreign_keys.sort_by(|left, right| left.name.cmp(&right.name));

    let mut indexes: Vec<IndexDef> = table.indexes.into_iter().map(index_def).collect();
    indexes.sort_by(|left, right| left.name.cmp(&right.name));

    TableDef {
        name: table.name,
        schema: None,
        columns,
        primary_key: table.primary_key,
        checks,
        foreign_keys,
        indexes,
    }
}

/// Converts a parsed foreign key into its catalog definition.
fn foreign_key_def(foreign_key: ForeignKeyLock) -> ForeignKeyDef {
    ForeignKeyDef {
        name: foreign_key.name,
        columns: foreign_key.columns,
        referenced_table: foreign_key.referenced_table,
        referenced_columns: foreign_key.referenced_columns,
        referenced_schema: None,
        on_delete: foreign_key.on_delete.into(),
        on_update: foreign_key.on_update.into(),
    }
}

/// Converts a parsed index into its catalog definition.
fn index_def(index: IndexLock) -> IndexDef {
    let mut definition = IndexDef::new(index.name, index.columns, index.unique);
    if let Some(method) = index.method {
        definition = definition.with_method(method);
    }
    if let Some(expression) = index.expression {
        definition = definition.with_expression(expression);
    }
    if let Some(predicate) = index.predicate {
        definition = definition.with_predicate(predicate);
    }
    definition
}

#[cfg(test)]
mod tests {
    use super::*;

    const V4_SNAPSHOT: &str = r#"version = 4

[[enums]]
name = "mood"
variants = ["happy", "sad"]

[[tables]]
name = "orders"
columns = [
  { name = "id", type = "int64", nullable = false },
  { name = "jdoc", type = "json", nullable = true, default = "'{\"a\": \"b\"}'" },
  { name = "moods", type = { array = { element = { enum = { name = "mood" } } } }, nullable = false },
  { name = "path", type = "text", nullable = true, default = "C:\\dir" },
  { name = "payload", type = "text", nullable = true, default = "line one\nline two" },
  { name = "raw", type = { other = { name = "citext" } }, nullable = true },
]
primary_key = ["id"]
checks = [{ name = "positive_id", expression = "id > 0" }]
foreign_keys = [
  { name = "orders_user_id_fkey", columns = ["user_id"], referenced_table = "users", referenced_columns = ["id"], on_delete = "cascade", on_update = "restrict" },
]
indexes = [
  { name = "idx_expr", columns = [], unique = false, method = "btree", expression = "lower(name)", predicate = "id > 10" },
]

[[views]]
name = "expensive_orders"
definition = "SELECT * FROM orders WHERE total > 100"

[[triggers]]
name = "audit_orders"
table = "orders"
definition = "EXECUTE FUNCTION log_order_update()"
"#;

    #[test]
    fn type_lock_conversions_round_trip_every_kind() {
        let kinds = [
            TypeKind::Boolean,
            TypeKind::Int32,
            TypeKind::Int64,
            TypeKind::Float64,
            TypeKind::Numeric,
            TypeKind::Text,
            TypeKind::Bytes,
            TypeKind::Uuid,
            TypeKind::Json,
            TypeKind::Timestamp,
            TypeKind::Date,
            TypeKind::Enum("mood".to_owned()),
            TypeKind::Array(Box::new(TypeKind::Enum("mood".to_owned()))),
            TypeKind::Array(Box::new(TypeKind::Array(Box::new(TypeKind::Text)))),
            TypeKind::Other("citext".to_owned()),
        ];

        for kind in kinds {
            let lock: TypeLock = (&kind).into();
            assert_eq!(TypeKind::from(lock), kind);
        }
    }

    #[test]
    fn fk_actions_round_trip_and_keep_their_canonical_strings() {
        let cases = [
            (ForeignKeyAction::NoAction, "no action"),
            (ForeignKeyAction::Restrict, "restrict"),
            (ForeignKeyAction::Cascade, "cascade"),
            (ForeignKeyAction::SetNull, "set null"),
            (ForeignKeyAction::SetDefault, "set default"),
        ];

        for (action, canonical) in cases {
            let lock = FkActionLock::from(action);
            let rendered = toml::Value::try_from(lock).expect("an action serializes");
            assert_eq!(rendered.as_str(), Some(canonical));
            assert_eq!(ForeignKeyAction::from(lock), action);
        }
    }

    #[test]
    fn a_v4_snapshot_parses_into_the_expected_catalog() {
        let catalog = parse_lock_toml(V4_SNAPSHOT).expect("the fixture parses");

        let expected = SchemaCatalog::new(vec![TableDef::new(
            "orders",
            vec![
                ColumnDef::new("id", TypeKind::Int64, false),
                ColumnDef::new("jdoc", TypeKind::Json, true).with_default(r#"'{"a": "b"}'"#),
                ColumnDef::new(
                    "moods",
                    TypeKind::Array(Box::new(TypeKind::Enum("mood".to_owned()))),
                    false,
                ),
                ColumnDef::new("path", TypeKind::Text, true).with_default("C:\\dir"),
                ColumnDef::new("payload", TypeKind::Text, true).with_default("line one\nline two"),
                ColumnDef::new("raw", TypeKind::Other("citext".to_owned()), true),
            ],
            vec!["id".to_owned()],
        )
        .with_check(CheckDef::new("positive_id", "id > 0"))
        .with_foreign_key(
            ForeignKeyDef::new(
                "orders_user_id_fkey",
                vec!["user_id".to_owned()],
                "users",
                vec!["id".to_owned()],
            )
            .with_actions(ForeignKeyAction::Cascade, ForeignKeyAction::Restrict),
        )
        .with_index(
            IndexDef::new("idx_expr", Vec::new(), false)
                .with_method("btree")
                .with_expression("lower(name)")
                .with_predicate("id > 10"),
        )])
        .with_enums(vec![EnumDef::new(
            "mood",
            vec!["happy".to_owned(), "sad".to_owned()],
        )])
        .with_views(vec![ViewDef::new(
            "expensive_orders",
            "SELECT * FROM orders WHERE total > 100",
        )])
        .with_triggers(vec![TriggerDef::new(
            "audit_orders",
            "orders",
            "EXECUTE FUNCTION log_order_update()",
        )]);

        assert_eq!(catalog, expected);
    }

    #[test]
    fn parsing_applies_the_canonical_ordering() {
        let snapshot = concat!(
            "version = 4\n",
            "\n",
            "enums = []\n",
            "views = []\n",
            "triggers = []\n",
            "[[tables]]\n",
            "name = \"t\"\n",
            "columns = [\n",
            "  { name = \"b\", type = \"int64\", nullable = false },\n",
            "  { name = \"a\", type = \"int64\", nullable = false },\n",
            "]\n",
            "checks = []\n",
            "foreign_keys = []\n",
            "indexes = []\n",
        );

        let catalog = parse_lock_toml(snapshot).expect("the snapshot parses");
        let names: Vec<&str> = catalog.tables[0]
            .columns
            .iter()
            .map(|column| column.name.as_str())
            .collect();

        assert_eq!(names, ["a", "b"]);
    }

    #[test]
    fn unknown_fields_are_rejected() {
        let snapshot = concat!(
            "version = 4\n",
            "\n",
            "enums = []\n",
            "views = []\n",
            "triggers = []\n",
            "[[tables]]\n",
            "name = \"users\"\n",
            "nonsense = 1\n",
            "columns = [{ name = \"id\", type = \"int64\", nullable = false }]\n",
            "checks = []\n",
            "foreign_keys = []\n",
            "indexes = []\n",
        );

        let error = parse_lock_toml(snapshot).expect_err("unknown keys are rejected");
        assert!(error.to_string().contains("unknown field `nonsense`"));

        let root_level = parse_lock_toml(concat!(
            "version = 4\n",
            "mystery = 1\n",
            "enums = []\n",
            "tables = []\n",
            "views = []\n",
            "triggers = []\n",
        ))
        .expect_err("unknown root keys are rejected");
        assert!(root_level.to_string().contains("mystery"));
    }

    #[test]
    fn snapshots_with_an_unsupported_version_are_rejected() {
        let snapshot = "version = 3\n\nenums = []\ntables = []\nviews = []\ntriggers = []\n";
        let error = parse_lock_toml(snapshot).expect_err("an unsupported version is rejected");

        assert!(error
            .to_string()
            .contains("unsupported schema lock version `3`"));
        assert!(error.to_string().contains("version = 4"));
    }

    #[test]
    fn a_snapshot_without_a_version_is_rejected() {
        let error = parse_lock_toml("enums = []\ntables = []\nviews = []\ntriggers = []\n")
            .expect_err("the version is required");

        assert!(error.to_string().contains("version"));
    }

    #[test]
    fn malformed_toml_is_reported_as_a_lock_format_error() {
        let error = parse_lock_toml("version = = 4\n").expect_err("invalid TOML is rejected");

        assert!(error.to_string().starts_with("invalid schema lock"));
    }

    #[test]
    fn the_dtos_survive_a_serialization_round_trip() {
        let file: LockFile = toml::from_str(V4_SNAPSHOT).expect("the fixture parses");
        let rendered = toml::to_string(&file).expect("the DTOs serialize as TOML");
        let reparsed: LockFile = toml::from_str(&rendered).expect("the rendering reparses");

        assert_eq!(reparsed, file);
    }

    #[test]
    fn rendering_round_trips_a_catalog_exercising_every_kind() {
        // Declared in the canonical order the writer sorts into, so the
        // reparsed snapshot equals the catalog that rendered it.
        let orders = TableDef::new(
            "orders",
            vec![
                ColumnDef::new("active", TypeKind::Boolean, false),
                ColumnDef::new("amount", TypeKind::Float64, false),
                ColumnDef::new("big", TypeKind::Int64, false),
                ColumnDef::new("day", TypeKind::Date, false),
                ColumnDef::new("doc", TypeKind::Bytes, true),
                ColumnDef::new("flags", TypeKind::Array(Box::new(TypeKind::Boolean)), true),
                ColumnDef::new("id", TypeKind::Int32, false),
                ColumnDef::new("jdoc", TypeKind::Json, true).with_default(r#"'{"a": "b"}'"#),
                ColumnDef::new("mood", TypeKind::Enum("mood".to_owned()), false),
                ColumnDef::new(
                    "moods",
                    TypeKind::Array(Box::new(TypeKind::Enum("mood".to_owned()))),
                    true,
                ),
                ColumnDef::new(
                    "nested",
                    TypeKind::Array(Box::new(TypeKind::Array(Box::new(TypeKind::Text)))),
                    true,
                ),
                ColumnDef::new("notes", TypeKind::Text, true).with_default("line one\nline two"),
                ColumnDef::new("path", TypeKind::Text, true).with_default("C:\\dir"),
                ColumnDef::new("price", TypeKind::Numeric, false),
                ColumnDef::new("raw", TypeKind::Other("citext".to_owned()), true),
                ColumnDef::new("stamp", TypeKind::Timestamp, false),
                ColumnDef::new("uid", TypeKind::Uuid, false),
            ],
            vec!["id".to_owned()],
        )
        .with_check(CheckDef::new("chk_positive", "amount >= 0"))
        .with_check(CheckDef::new("chk_range", "amount < 1000000"))
        .with_foreign_key(
            ForeignKeyDef::new(
                "fk_cascade",
                vec!["big".to_owned()],
                "users",
                vec!["id".to_owned()],
            )
            .with_on_delete(ForeignKeyAction::Cascade),
        )
        .with_foreign_key(
            ForeignKeyDef::new(
                "fk_default",
                vec!["day".to_owned()],
                "users",
                vec!["id".to_owned()],
            )
            .with_on_delete(ForeignKeyAction::SetDefault),
        )
        .with_foreign_key(
            ForeignKeyDef::new(
                "fk_restrict",
                vec!["doc".to_owned()],
                "users",
                vec!["id".to_owned()],
            )
            .with_on_update(ForeignKeyAction::Restrict),
        )
        .with_foreign_key(
            ForeignKeyDef::new(
                "fk_set_null",
                vec!["flags".to_owned()],
                "users",
                vec!["id".to_owned()],
            )
            .with_actions(ForeignKeyAction::SetNull, ForeignKeyAction::NoAction),
        )
        .with_index(IndexDef::new(
            "idx_columns",
            vec!["amount".to_owned()],
            true,
        ))
        .with_index(
            IndexDef::new("idx_expression", Vec::new(), false).with_expression("lower(notes)"),
        )
        .with_index(
            IndexDef::new("idx_partial", vec!["active".to_owned()], false)
                .with_method("btree")
                .with_predicate("active = 1"),
        );

        let catalog = SchemaCatalog::new(vec![orders])
            .with_enums(vec![EnumDef::new(
                "mood",
                vec!["happy".to_owned(), "sad".to_owned()],
            )])
            .with_views(vec![ViewDef::new(
                "expensive_orders",
                "SELECT * FROM orders WHERE amount > 100",
            )])
            .with_triggers(vec![TriggerDef::new(
                "audit_orders",
                "orders",
                "EXECUTE FUNCTION log_order_update()",
            )]);

        let lock = render_lock_toml(&catalog);

        assert!(lock.starts_with("version = 4"), "got: {lock}");
        assert_eq!(
            SchemaCatalog::from_lock(&lock).expect("the rendering reparses"),
            catalog
        );
    }
}
