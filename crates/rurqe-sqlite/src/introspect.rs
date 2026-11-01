//! SQLite catalog introspection through `PRAGMA` queries and `sqlite_master`.
//!
//! The module owns the pragma statements, the raw metadata rows they report, and
//! the [`introspect_sqlite`] entry point that assembles them into a
//! [`SchemaCatalog`]. The `CHECK (<column> IN (...))` constraints SQLite spells
//! enums with are parsed in `check_enums`, the checks a table declares are
//! collected in `checks`, the secondary indexes, views, and triggers in
//! `indexes`, and the normalized catalog is assembled in `assembly`.
//!
//! The shape mirrors the PostgreSQL adapter of the workspace: an engine reports
//! its catalog as plain rows, and the rows are folded into the engine-neutral
//! catalog of the migration crate, so a catalog read from a SQLite file compares,
//! snapshots, and verifies through the same vocabulary as one read from
//! PostgreSQL.

mod assembly;
mod check_enums;
mod checks;
mod indexes;
pub(crate) mod lexer;

#[cfg(test)]
mod tests;

use core::fmt;

use rurqe_migrate::catalog::{ForeignKeyAction, SchemaCatalog};
use rurqe_migrate::introspect::{parse_foreign_key_action, RawCatalogSections};

pub use assembly::{catalog_from_sqlite_metadata, catalog_from_sqlite_metadata_with_sections};
pub use check_enums::parse_sqlite_check_enums;

use assembly::catalog_from_sqlite_metadata_with_enums;

use checks::sqlite_checks;
use indexes::{sqlite_indexes, sqlite_triggers, sqlite_views};

/// An error raised while reading the catalog of a SQLite database.
///
/// A catalog build issues several statements, so the description keeps the
/// operation that failed: the operator reads which pragma or `sqlite_master`
/// query broke instead of a driver error stripped of its context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SqliteCatalogError {
    /// A catalog statement was refused or reported a value that could not be read.
    Catalog(String),
}

impl fmt::Display for SqliteCatalogError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Catalog(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for SqliteCatalogError {}

/// Describes the failure of the catalog `operation` that was attempted.
fn query_error(operation: impl fmt::Display, error: impl fmt::Display) -> SqliteCatalogError {
    SqliteCatalogError::Catalog(format!("{operation}: {error}"))
}

/// Query to fetch user tables and their DDL from the SQLite `sqlite_master` catalog.
const SQLITE_TABLES: &str = r#"
SELECT name, sql
FROM sqlite_master
WHERE type = 'table'
  AND name NOT LIKE 'sqlite_%'
  AND name != '__rurqe_schema_history'
ORDER BY name;
"#;

/// Query to fetch the views of the schema together with their DDL.
const SQLITE_VIEWS: &str = r#"
SELECT name, sql
FROM sqlite_master
WHERE type = 'view'
  AND name NOT LIKE 'sqlite_%'
ORDER BY name;
"#;

/// Query to fetch the triggers of the schema together with their DDL.
const SQLITE_TRIGGERS: &str = r#"
SELECT name, tbl_name, sql
FROM sqlite_master
WHERE type = 'trigger'
  AND name NOT LIKE 'sqlite_%'
ORDER BY name;
"#;

/// Query to fetch the DDL of every index declared on one table.
///
/// The statement reports one row per index declared on the table, and an index
/// SQLite builds implicitly for a `UNIQUE` constraint or a primary key carries no
/// DDL and reports a `NULL` definition for the row it occupies.
const SQLITE_INDEX_DDL: &str =
    "SELECT name, sql FROM sqlite_master WHERE type = 'index' AND tbl_name = ?1;";

/// Builds the `PRAGMA table_info` query for `table`.
///
/// The table name is wrapped in double quotes and every embedded `"` is
/// doubled, so legal-but-unusual table names neither break the statement nor
/// smuggle additional SQL into it.
fn sqlite_table_info(table: &str) -> String {
    format!(
        "PRAGMA table_info(\"{}\");",
        escape_sqlite_identifier(table)
    )
}

/// Builds the `PRAGMA foreign_key_list` query for `table`.
fn sqlite_foreign_key_list(table: &str) -> String {
    format!(
        "PRAGMA foreign_key_list(\"{}\");",
        escape_sqlite_identifier(table)
    )
}

/// Builds the `PRAGMA index_list` query for `table`.
fn sqlite_index_list(table: &str) -> String {
    format!(
        "PRAGMA index_list(\"{}\");",
        escape_sqlite_identifier(table)
    )
}

/// Builds the `PRAGMA index_info` query for `index`.
fn sqlite_index_info(index: &str) -> String {
    format!(
        "PRAGMA index_info(\"{}\");",
        escape_sqlite_identifier(index)
    )
}

/// Escapes a SQLite identifier for use inside a double-quoted name literal.
fn escape_sqlite_identifier(identifier: &str) -> String {
    identifier.replace('"', "\"\"")
}

/// Raw SQLite table metadata row read from `sqlite_master`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawSqliteTableMetadata {
    /// The name of the table.
    pub name: String,
    /// The DDL the table was created with, absent for a table without one.
    pub sql: Option<String>,
}

/// Raw SQLite column metadata row read from `PRAGMA table_info`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawSqliteColumnMetadata {
    /// The table declaring the column.
    pub table_name: String,
    /// The name of the column.
    pub column_name: String,
    /// The declared type, verbatim.
    pub data_type: String,
    /// Whether the column is declared `NOT NULL`.
    pub not_null: bool,
    /// The position of the column inside the primary key, zero when it is not one.
    pub pk_position: i64,
    /// The declared default value, absent when the column declares none.
    pub default_value: Option<String>,
}

/// Raw SQLite foreign key row read from `PRAGMA foreign_key_list`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawSqliteForeignKeyMetadata {
    /// The table declaring the constraint.
    pub table_name: String,
    /// The identifier of the constraint inside its table.
    pub id: i64,
    /// The referencing column.
    pub from_column: String,
    /// The referenced column, empty when the key targets the parent primary key.
    pub to_column: String,
    /// The referenced table.
    pub referenced_table: String,
    /// The action the key takes when its parent row is deleted.
    pub on_delete: ForeignKeyAction,
    /// The action the key takes when its parent key is updated.
    pub on_update: ForeignKeyAction,
}

/// Introspects a live SQLite connection and returns the normalized [`SchemaCatalog`].
///
/// Every table of the file is read through `PRAGMA table_info`,
/// `PRAGMA foreign_key_list`, and `PRAGMA index_list`, while the views, the
/// triggers, and the index definitions come from `sqlite_master`. The DDL of a
/// table is lexed once, and the enum columns, the check constraints, and the
/// index terms of that statement are recovered from the single pass.
///
/// # Errors
///
/// Returns [`SqliteCatalogError::Catalog`] when a catalog statement cannot be
/// prepared, executed, or read, describing the operation that failed.
pub fn introspect_sqlite(
    connection: &::rusqlite::Connection,
) -> Result<SchemaCatalog, SqliteCatalogError> {
    let mut statement = connection
        .prepare(SQLITE_TABLES)
        .map_err(|error| query_error("failed to prepare SQLite table query", error))?;
    let raw_tables = statement
        .query_map([], |row| {
            Ok(RawSqliteTableMetadata {
                name: row.get::<_, String>(0)?,
                sql: row.get::<_, Option<String>>(1)?,
            })
        })
        .map_err(|error| query_error("failed to query SQLite tables", error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| query_error("failed to read SQLite table row", error))?;

    let table_names: Vec<String> = raw_tables.iter().map(|table| table.name.clone()).collect();

    let mut raw_columns = Vec::new();
    let mut raw_foreign_keys = Vec::new();
    let mut raw_indexes = Vec::new();

    for table in &table_names {
        let col_sql = sqlite_table_info(table);
        let mut col_stmt = connection.prepare(&col_sql).map_err(|error| {
            query_error(format!("failed to prepare table_info for `{table}`"), error)
        })?;

        let cols = col_stmt
            .query_map([], |row| {
                let notnull: i64 = row.get(3)?;
                let pk: i64 = row.get(5)?;
                Ok(RawSqliteColumnMetadata {
                    table_name: table.clone(),
                    column_name: row.get(1)?,
                    data_type: row.get(2)?,
                    not_null: notnull != 0,
                    pk_position: pk,
                    default_value: row.get(4)?,
                })
            })
            .map_err(|error| {
                query_error(format!("failed to inspect columns for `{table}`"), error)
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| query_error(format!("failed to read column for `{table}`"), error))?;

        raw_columns.extend(cols);

        let fk_sql = sqlite_foreign_key_list(table);
        let mut fk_stmt = connection.prepare(&fk_sql).map_err(|error| {
            query_error(
                format!("failed to prepare foreign_key_list for `{table}`"),
                error,
            )
        })?;

        let mut fks = fk_stmt
            .query_map([], |row| {
                let id: i64 = row.get(0)?;
                // `seq` positions the column inside its constraint.
                let position: i64 = row.get(1)?;
                let referenced_table: String = row.get(2)?;
                let from_column: String = row.get(3)?;
                let to_column: Option<String> = row.get(4)?;
                let on_update: String = row.get(5)?;
                let on_delete: String = row.get(6)?;
                Ok((
                    (id, position),
                    RawSqliteForeignKeyMetadata {
                        table_name: table.clone(),
                        id,
                        from_column,
                        to_column: to_column.unwrap_or_default(),
                        referenced_table,
                        on_delete: parse_foreign_key_action(&on_delete),
                        on_update: parse_foreign_key_action(&on_update),
                    },
                ))
            })
            .map_err(|error| {
                query_error(
                    format!("failed to inspect foreign keys for `{table}`"),
                    error,
                )
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| {
                query_error(format!("failed to read foreign key for `{table}`"), error)
            })?;

        // `PRAGMA foreign_key_list` reports one row per key column, and `seq`
        // carries the position of the column inside its constraint. Ordering by
        // it keeps a composite key assembled in the order it was declared in,
        // even when the pragma emits its rows in another one.
        fks.sort_by_key(|(position, _)| *position);
        raw_foreign_keys.extend(fks.into_iter().map(|(_, row)| row));

        raw_indexes.extend(sqlite_indexes(connection, table)?);
    }

    let (raw_checks, enum_columns) = sqlite_checks(&raw_tables, &raw_columns);
    let raw_views = sqlite_views(connection)?;
    let raw_triggers = sqlite_triggers(connection)?;

    Ok(catalog_from_sqlite_metadata_with_enums(
        &raw_tables,
        &raw_columns,
        &raw_foreign_keys,
        &raw_indexes,
        &RawCatalogSections {
            checks: &raw_checks,
            views: &raw_views,
            triggers: &raw_triggers,
        },
        &enum_columns,
    ))
}
