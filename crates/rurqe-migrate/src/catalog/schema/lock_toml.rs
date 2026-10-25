//! Versioned TOML v4 DTOs for the `schema.lock` snapshot.
//!
//! The v4 format replaces the bespoke text grammar of the v1–v3 snapshots with
//! strongly typed TOML: every section is a struct carrying
//! `#[serde(deny_unknown_fields)]`, so an unrecognized key or a misspelled
//! field is rejected instead of silently ignored. [`parse_lock_toml`] reads a
//! snapshot into a [`SchemaCatalog`] and applies the same canonical ordering
//! the legacy reader produces, so both readers yield identical catalogs.

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
    /// The version of the lock format, always `4` for this reader.
    version: u32,
    /// The enum types declared by the schema.
    enums: Vec<EnumLock>,
    /// The tables of the schema.
    tables: Vec<TableLock>,
    /// The views declared by the schema.
    views: Vec<ViewLock>,
    /// The triggers declared by the schema.
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
            TypeKind::Enum(name) => Self::Enum {
                name: name.clone(),
            },
            TypeKind::Array(element) => Self::Array {
                element: Box::new(TypeLock::from(element.as_ref())),
            },
            TypeKind::Other(name) => Self::Other {
                name: name.clone(),
            },
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
                ColumnDef::new("payload", TypeKind::Text, true)
                    .with_default("line one\nline two"),
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
}
