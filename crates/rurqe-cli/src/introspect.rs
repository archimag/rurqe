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


