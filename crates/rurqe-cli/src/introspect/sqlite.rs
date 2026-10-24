//! SQLite catalog introspection through `PRAGMA` queries and `sqlite_master`.
//!
//! This module owns the pragma statements, the raw metadata rows they report, and
//! the [`introspect_sqlite`] entry point. The `CHECK (<column> IN (...))`
//! constraints SQLite spells enums with are parsed in [`check_enums`], the checks
//! a table declares are collected in [`checks`], the secondary indexes, views, and
//! triggers in [`indexes`], and the normalized catalog is assembled in
//! [`assembly`].

mod assembly;
mod check_enums;
mod checks;
mod indexes;
pub(crate) mod lexer;

#[cfg(test)]
mod tests;

use rurqe_migrate::catalog::SchemaCatalog;

use crate::cli::CliError;

use super::types::{parse_foreign_key_action, RawCatalogSections};

pub use assembly::{catalog_from_sqlite_metadata, catalog_from_sqlite_metadata_with_sections};
pub use check_enums::parse_sqlite_check_enums;

use checks::sqlite_checks;
use indexes::{sqlite_indexes, sqlite_triggers, sqlite_views};

/// Query to fetch user tables and their DDL from the SQLite `sqlite_master` catalog.
pub(crate) const SQLITE_TABLES: &str = r#"
SELECT name, sql
FROM sqlite_master
WHERE type = 'table'
  AND name NOT LIKE 'sqlite_%'
  AND name != '__rurqe_schema_history'
ORDER BY name;
"#;

/// Query to fetch the views of the schema together with their DDL.
pub(crate) const SQLITE_VIEWS: &str = r#"
SELECT name, sql
FROM sqlite_master
WHERE type = 'view'
  AND name NOT LIKE 'sqlite_%'
ORDER BY name;
"#;

/// Query to fetch the triggers of the schema together with their DDL.
pub(crate) const SQLITE_TRIGGERS: &str = r#"
SELECT name, tbl_name, sql
FROM sqlite_master
WHERE type = 'trigger'
  AND name NOT LIKE 'sqlite_%'
ORDER BY name;
"#;

/// Query to fetch the DDL an index was declared with.
///
/// An index SQLite builds implicitly for a `UNIQUE` constraint carries no DDL, and
/// the statement then reports no row at all.
const SQLITE_INDEX_DDL: &str = "SELECT sql FROM sqlite_master WHERE type = 'index' AND name = ?1;";

/// Builds the `PRAGMA table_info` query for `table`.
///
/// The table name is wrapped in double quotes and every embedded `"` is
/// doubled, so legal-but-unusual table names neither break the statement nor
/// smuggle additional SQL into it.
pub(crate) fn sqlite_table_info(table: &str) -> String {
    format!(
        "PRAGMA table_info(\"{}\");",
        escape_sqlite_identifier(table)
    )
}

/// Builds the `PRAGMA foreign_key_list` query for `table`.
pub(crate) fn sqlite_foreign_key_list(table: &str) -> String {
    format!(
        "PRAGMA foreign_key_list(\"{}\");",
        escape_sqlite_identifier(table)
    )
}

/// Builds the `PRAGMA index_list` query for `table`.
pub(crate) fn sqlite_index_list(table: &str) -> String {
    format!(
        "PRAGMA index_list(\"{}\");",
        escape_sqlite_identifier(table)
    )
}

/// Builds the `PRAGMA index_info` query for `index`.
pub(crate) fn sqlite_index_info(index: &str) -> String {
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
    pub name: String,
    pub sql: Option<String>,
}

/// Raw SQLite column metadata row read from `PRAGMA table_info`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawSqliteColumnMetadata {
    pub table_name: String,
    pub column_name: String,
    pub data_type: String,
    pub not_null: bool,
    pub pk_position: i64,
    pub default_value: Option<String>,
}

/// Raw SQLite foreign key row read from `PRAGMA foreign_key_list`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawSqliteForeignKeyMetadata {
    pub table_name: String,
    pub id: i64,
    pub from_column: String,
    pub to_column: String,
    pub referenced_table: String,
    pub on_delete: rurqe_migrate::catalog::ForeignKeyAction,
    pub on_update: rurqe_migrate::catalog::ForeignKeyAction,
}

/// Introspects a live SQLite database connection and returns the normalized [`SchemaCatalog`].
pub fn introspect_sqlite(conn: &rusqlite::Connection) -> Result<SchemaCatalog, CliError> {
    let mut stmt = conn
        .prepare(SQLITE_TABLES)
        .map_err(|e| CliError(format!("failed to prepare SQLite table query: {e}")))?;
    let raw_tables = stmt
        .query_map([], |row| {
            Ok(RawSqliteTableMetadata {
                name: row.get::<_, String>(0)?,
                sql: row.get::<_, Option<String>>(1)?,
            })
        })
        .map_err(|e| CliError(format!("failed to query SQLite tables: {e}")))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| CliError(format!("failed to read SQLite table row: {e}")))?;

    let table_names: Vec<String> = raw_tables.iter().map(|t| t.name.clone()).collect();

    let mut raw_columns = Vec::new();
    let mut raw_foreign_keys = Vec::new();
    let mut raw_indexes = Vec::new();

    for table in &table_names {
        let col_sql = sqlite_table_info(table);
        let mut col_stmt = conn
            .prepare(&col_sql)
            .map_err(|e| CliError(format!("failed to prepare table_info for `{table}`: {e}")))?;

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
            .map_err(|e| CliError(format!("failed to inspect columns for `{table}`: {e}")))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| CliError(format!("failed to read column for `{table}`: {e}")))?;

        raw_columns.extend(cols);

        let fk_sql = sqlite_foreign_key_list(table);
        let mut fk_stmt = conn.prepare(&fk_sql).map_err(|e| {
            CliError(format!(
                "failed to prepare foreign_key_list for `{table}`: {e}"
            ))
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
            .map_err(|e| CliError(format!("failed to inspect foreign keys for `{table}`: {e}")))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| CliError(format!("failed to read foreign key for `{table}`: {e}")))?;

        // `PRAGMA foreign_key_list` reports one row per key column, and `seq`
        // carries the position of the column inside its constraint. Ordering by
        // it keeps a composite key assembled in the order it was declared in,
        // even when the pragma emits its rows in another one.
        fks.sort_by_key(|(position, _)| *position);
        raw_foreign_keys.extend(fks.into_iter().map(|(_, row)| row));

        raw_indexes.extend(sqlite_indexes(conn, table)?);
    }

    let raw_checks = sqlite_checks(&raw_tables, &raw_columns);
    let raw_views = sqlite_views(conn)?;
    let raw_triggers = sqlite_triggers(conn)?;

    Ok(catalog_from_sqlite_metadata_with_sections(
        &raw_tables,
        &raw_columns,
        &raw_foreign_keys,
        &raw_indexes,
        &RawCatalogSections {
            checks: &raw_checks,
            views: &raw_views,
            triggers: &raw_triggers,
        },
    ))
}
