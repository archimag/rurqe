//! Catalog introspection facade.
//!
//! Engine-independent metadata handling lives in [`types`], SQLite `PRAGMA`
//! introspection in [`sqlite`], and PostgreSQL `information_schema` /
//! `pg_catalog` introspection in [`postgres`]. This facade re-exports the public
//! surface and keeps the engine-agnostic entry points (`detect_dialect`,
//! `introspect_database`, `introspect_connection`) plus the backward-compatible
//! [`IntrospectQueries`] container.

use std::fmt;

use rurqe_migrate::catalog::SchemaCatalog;

use crate::cli::{connect, CliError, Connection};

mod postgres;
mod sqlite;
mod types;

#[cfg(test)]
mod tests;

pub use postgres::{introspect_postgres, introspect_postgres_schema};
pub use sqlite::{
    catalog_from_sqlite_metadata, introspect_sqlite, RawSqliteColumnMetadata,
    RawSqliteForeignKeyMetadata, RawSqliteTableMetadata,
};
pub use types::{
    catalog_from_metadata, map_sql_type_to_kind, RawColumnMetadata, RawForeignKeyMetadata,
    RawPrimaryKeyMetadata, RawTableMetadata,
};

/// The database engine a connection string targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatabaseDialect {
    /// PostgreSQL, selected by `postgres://` / `postgresql://` URLs.
    Postgres,
    /// SQLite, selected by `sqlite:` URLs or local `.sqlite` / `.sqlite3` / `.db` files.
    Sqlite,
}

impl fmt::Display for DatabaseDialect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            DatabaseDialect::Postgres => "PostgreSQL",
            DatabaseDialect::Sqlite => "SQLite",
        })
    }
}

/// Detects the target [`DatabaseDialect`] from a connection string.
///
/// Absent URLs default to [`DatabaseDialect::Postgres`], preserving the
/// historical behavior for callers that rely on the default.
pub fn detect_dialect(database_url: Option<&str>) -> DatabaseDialect {
    match database_url {
        Some(url) => {
            let lower = url.trim().to_ascii_lowercase();
            if lower.starts_with("sqlite:")
                || lower == ":memory:"
                || lower.ends_with(".sqlite")
                || lower.ends_with(".sqlite3")
                || lower.ends_with(".db")
            {
                DatabaseDialect::Sqlite
            } else {
                DatabaseDialect::Postgres
            }
        }
        None => DatabaseDialect::Postgres,
    }
}

/// Introspects the schema catalog of a live database identified by its connection URL or path.
pub fn introspect_database(url: &str) -> Result<SchemaCatalog, CliError> {
    let mut connection = connect(url)?;
    introspect_connection(&mut connection)
}

/// Introspects the schema catalog of an already opened [`Connection`].
///
/// Engine dispatch lives next to the catalog logic, so callers (including tests)
/// can hand in an open SQLite or PostgreSQL handle without going through URL parsing.
pub(crate) fn introspect_connection(
    connection: &mut Connection,
) -> Result<SchemaCatalog, CliError> {
    match connection {
        Connection::Sqlite(conn) => introspect_sqlite(conn),
        Connection::Postgres(client) => introspect_postgres(client),
    }
}

/// Canonical PostgreSQL and SQLite catalog introspection queries.
pub struct IntrospectQueries;

impl IntrospectQueries {
    /// Query to fetch all user tables in `public` schema.
    pub const POSTGRES_TABLES: &'static str = postgres::POSTGRES_TABLES;

    /// Query to fetch all columns in user tables.
    pub const POSTGRES_COLUMNS: &'static str = postgres::POSTGRES_COLUMNS;

    /// Query to fetch all primary key constraint columns.
    pub const POSTGRES_PRIMARY_KEYS: &'static str = postgres::POSTGRES_PRIMARY_KEYS;

    /// Query to fetch all foreign key columns in key order.
    pub const POSTGRES_FOREIGN_KEYS: &'static str = postgres::POSTGRES_FOREIGN_KEYS;

    /// Query to fetch user tables from the SQLite `sqlite_master` catalog.
    pub const SQLITE_TABLES: &'static str = sqlite::SQLITE_TABLES;

    /// Builds the `PRAGMA table_info` query for `table`.
    pub fn sqlite_table_info(table: &str) -> String {
        sqlite::sqlite_table_info(table)
    }

    /// Builds the `PRAGMA foreign_key_list` query for `table`.
    pub fn sqlite_foreign_key_list(table: &str) -> String {
        sqlite::sqlite_foreign_key_list(table)
    }
}
