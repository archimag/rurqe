//! Catalog introspection facade.
//!
//! Engine-independent metadata handling lives in [`types`], SQLite `PRAGMA`
//! introspection in [`sqlite`], and PostgreSQL `information_schema` /
//! `pg_catalog` introspection in [`postgres`]. This facade re-exports the public
//! surface and keeps the engine-agnostic entry points (`detect_dialect`,
//! `introspect_database`, `introspect_database_schema`) that open a connection
//! from a database URL.

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
    catalog_from_sqlite_metadata, catalog_from_sqlite_metadata_with_sections, introspect_sqlite,
    parse_sqlite_check_enums, RawSqliteColumnMetadata, RawSqliteForeignKeyMetadata,
    RawSqliteTableMetadata,
};
pub use types::{
    catalog_from_metadata, catalog_from_metadata_with_sections, map_sql_type_to_kind,
    map_sqlite_declared_type_to_kind, parse_foreign_key_action, RawCatalogSections,
    RawCheckMetadata, RawColumnMetadata, RawEnumMetadata, RawForeignKeyMetadata, RawIndexMetadata,
    RawPrimaryKeyMetadata, RawTableMetadata, RawTriggerMetadata, RawViewMetadata,
};

/// Name of the schema introspected when the caller names none.
///
/// A PostgreSQL database keeps its tables in named schemas, and `public` is the
/// one an ordinary single-schema deployment uses. SQLite has no schema layer at
/// all, so the name is ignored there and the single catalog of the opened file is
/// read.
pub const DEFAULT_SCHEMA: &str = "public";

/// Introspects the schema catalog of a live database identified by its connection URL or path.
///
/// The engine-agnostic entry point reads [`DEFAULT_SCHEMA`]; use
/// [`introspect_database_schema`] to name another PostgreSQL schema.
pub fn introspect_database(url: &str) -> Result<SchemaCatalog, CliError> {
    introspect_database_schema(url, DEFAULT_SCHEMA)
}

/// Introspects `schema` of the live database identified by `url`.
///
/// A PostgreSQL catalog is read from the named schema, so a deployment keeping
/// its tables outside `public` is described by the very tables it declares
/// there. A SQLite database holds a single unnamed catalog, so the name is
/// ignored rather than rejected: the same project then introspects both engines
/// without branching on the dialect.
///
/// A selection that describes nothing is reported through the `tracing` facade
/// before the catalog is handed on; see [`report_vacuous_schema`].
pub fn introspect_database_schema(url: &str, schema: &str) -> Result<SchemaCatalog, CliError> {
    let mut connection = connect(url)?;
    report_vacuous_schema(&mut connection, schema)?;
    introspect_connection_schema(&mut connection, schema)
}

/// Introspects `schema` of an already opened [`Connection`].
///
/// Engine dispatch lives next to the catalog logic, so callers (including tests)
/// can hand in an open SQLite or PostgreSQL handle without going through URL
/// parsing. The schema name reaches only the PostgreSQL side; a SQLite
/// connection has no schema layer, so its single catalog is read whatever name
/// the caller passes.
pub(crate) fn introspect_connection_schema(
    connection: &mut Connection,
    schema: &str,
) -> Result<SchemaCatalog, CliError> {
    match connection {
        Connection::Sqlite(conn) => introspect_sqlite(conn),
        Connection::Postgres(client) => introspect_postgres_schema(client, schema),
    }
}

/// Reports a selected schema that describes nothing while the database keeps
/// user relations in other schemas.
///
/// A PostgreSQL project may keep its relations in a schema of its own, and a
/// catalog read from the wrong one is empty rather than wrong: every digest
/// computed from it — the codegen snapshot, the drift gate, the cold-start
/// marker, and the checkpoint hash — then compares nothing against nothing and
/// passes vacuously. The notice names the schemas that do hold relations, so the
/// operator selects one with `--schema` (or the `schema` key of `rurqe.toml`)
/// instead of trusting a verification that never looked at their tables.
pub(crate) fn report_vacuous_schema(
    connection: &mut Connection,
    schema: &str,
) -> Result<(), CliError> {
    match connection {
        // A SQLite database holds one unnamed catalog, so the selection always
        // describes it whatever name the caller passed.
        Connection::Sqlite(_) => Ok(()),
        Connection::Postgres(client) => report_vacuous_postgres_schema(client, schema),
    }
}

/// Reports a selected PostgreSQL schema that holds no relation while relations
/// live elsewhere.
pub(crate) fn report_vacuous_postgres_schema(
    client: &mut ::postgres::Client,
    schema: &str,
) -> Result<(), CliError> {
    let neighbours = postgres::schema_neighbours(client, schema)?;

    if neighbours.is_empty() {
        return Ok(());
    }

    let names = neighbours.join(", ");
    tracing::warn!(
        schema,
        schemas = %names,
        "the selected schema `{schema}` holds no table or view while the database keeps user \
         relations in {names}; the catalog digest computed from it describes nothing, so pass \
         `--schema <name>` or set `schema = \"<name>\"` in `rurqe.toml`"
    );

    Ok(())
}
