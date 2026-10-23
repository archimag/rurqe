//! Catalog introspection facade.
//!
//! Engine-independent metadata handling lives in [`types`], SQLite `PRAGMA`
//! introspection in [`sqlite`], and PostgreSQL `information_schema` /
//! `pg_catalog` introspection in [`postgres`]. This facade re-exports the public
//! surface and keeps the engine-agnostic entry points (`detect_dialect`,
//! `introspect_database`, `introspect_connection`) plus the backward-compatible
//! [`IntrospectQueries`] container.

use rurqe_migrate::catalog::SchemaCatalog;

use crate::cli::{connect, CliError, Connection};

mod postgres;
mod sqlite;
mod types;

#[cfg(test)]
mod tests;

pub use crate::target::{detect_dialect, DatabaseDialect};
pub use postgres::{introspect_postgres, introspect_postgres_schema};
pub use sqlite::{
    catalog_from_sqlite_metadata, introspect_sqlite, RawSqliteColumnMetadata,
    RawSqliteForeignKeyMetadata, RawSqliteTableMetadata,
};
pub use types::{
    catalog_from_metadata, map_sql_type_to_kind, map_sqlite_declared_type_to_kind,
    RawColumnMetadata, RawEnumMetadata, RawForeignKeyMetadata, RawIndexMetadata,
    RawPrimaryKeyMetadata, RawTableMetadata,
};

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
    /// Query to fetch user tables from the SQLite `sqlite_master` catalog.
    pub const SQLITE_TABLES: &'static str = sqlite::SQLITE_TABLES;

    /// Query to fetch all user tables in `public` schema.
    pub const POSTGRES_TABLES: &'static str = postgres::POSTGRES_TABLES;

    /// Query to fetch all columns in user tables.
    pub const POSTGRES_COLUMNS: &'static str = postgres::POSTGRES_COLUMNS;

    /// Query to fetch all primary key constraint columns.
    pub const POSTGRES_PRIMARY_KEYS: &'static str = postgres::POSTGRES_PRIMARY_KEYS;

    /// Query to fetch all foreign key columns in key order.
    pub const POSTGRES_FOREIGN_KEYS: &'static str = postgres::POSTGRES_FOREIGN_KEYS;

    /// Query to fetch the secondary indexes of user tables, one row per column.
    pub const POSTGRES_INDEXES: &'static str = postgres::POSTGRES_INDEXES;

    /// Query to fetch all user enum types and their variants in sort order.
    pub const POSTGRES_ENUMS: &'static str = postgres::POSTGRES_ENUMS;

    /// Builds the `PRAGMA table_info` query for `table`.
    pub fn sqlite_table_info(table: &str) -> String {
        sqlite::sqlite_table_info(table)
    }

    /// Builds the `PRAGMA foreign_key_list` query for `table`.
    pub fn sqlite_foreign_key_list(table: &str) -> String {
        sqlite::sqlite_foreign_key_list(table)
    }

    /// Builds the `PRAGMA index_list` query for `table`.
    pub fn sqlite_index_list(table: &str) -> String {
        sqlite::sqlite_index_list(table)
    }

    /// Builds the `PRAGMA index_info` query for `index`.
    pub fn sqlite_index_info(index: &str) -> String {
        sqlite::sqlite_index_info(index)
    }
}
