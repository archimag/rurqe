//! Catalog introspection facade.
//!
//! The engine-independent metadata rows, the SQL type mapping, and the catalog
//! assembly live in [`rurqe_migrate::introspect`]; the PostgreSQL catalog
//! statements and the decoding of the rows they return live in
//! `rurqe_postgres::introspect`, while the SQLite `PRAGMA` reader and its
//! ratified DDL lexer live in `rurqe_sqlite::introspect`. This facade re-exports
//! that shared vocabulary beside the engine entry points and keeps the
//! engine-agnostic helpers (`detect_dialect`, `introspect_database`,
//! `introspect_database_selection`) that open a connection from a database URL.
//!
//! The catalog protocol therefore exists in the workspace exactly once: the
//! driver crates own introspection next to the migration protocol, and the
//! toolchain consumes it. A failure an adapter describes — the statement it ran
//! and the column it could not read — is translated into a [`CliError`] here, at
//! the boundary, so no command restates that wording.

use rurqe_migrate::catalog::SchemaCatalog;

use crate::cli::{connect, CliError, Connection};
use crate::config::SchemaSelection;

#[cfg(test)]
mod tests;

pub use crate::target::{detect_dialect, DatabaseDialect};
pub use rurqe_migrate::introspect::{
    catalog_from_metadata, catalog_from_metadata_with_sections, map_sql_type_to_kind,
    map_sqlite_declared_type_to_kind, parse_foreign_key_action, RawCatalogSections,
    RawCheckMetadata, RawColumnMetadata, RawEnumMetadata, RawForeignKeyMetadata, RawIndexMetadata,
    RawPrimaryKeyMetadata, RawTableMetadata, RawTriggerMetadata, RawViewMetadata,
};
pub use rurqe_project::DEFAULT_SCHEMA;
pub use rurqe_sqlite::introspect::{
    catalog_from_sqlite_metadata, catalog_from_sqlite_metadata_with_sections, introspect_sqlite,
    parse_sqlite_check_enums, RawSqliteColumnMetadata, RawSqliteForeignKeyMetadata,
    RawSqliteTableMetadata,
};

/// Introspects the `public` schema of a live PostgreSQL database client.
pub fn introspect_postgres(client: &mut ::postgres::Client) -> Result<SchemaCatalog, CliError> {
    introspect_postgres_schema(client, DEFAULT_SCHEMA)
}

/// Introspects `schema` over any PostgreSQL connection.
///
/// The single schema is the selection the multi-schema entry point introspects,
/// so a session that reads one schema runs the very catalog statements a session
/// describing several of them runs.
///
/// The connection is named through [`postgres::GenericClient`], so the same
/// catalog statements run both on a bare client and inside an open transaction:
/// a scratch schema can therefore be introspected before its transaction
/// commits, and the golden equation reads it through the same queries.
pub fn introspect_postgres_schema<C>(
    client: &mut C,
    schema: &str,
) -> Result<SchemaCatalog, CliError>
where
    C: postgres::GenericClient,
{
    introspect_postgres_selection(client, &SchemaSelection::single(schema))
}

/// Introspects every schema of `selection` over any PostgreSQL connection.
///
/// The whole selection is bound as one array parameter rather than being rendered
/// into the statement text, so no caller can smuggle SQL into the catalog queries
/// and every catalog statement describes the primary schema and its secondary
/// schemas in one round trip. A row of the primary schema carries no schema
/// attribution while a row of a secondary schema carries its own, so a
/// single-schema introspection reproduces the catalog recorded before schemas
/// were read at all, byte for byte.
pub(crate) fn introspect_postgres_selection<C>(
    client: &mut C,
    selection: &SchemaSelection,
) -> Result<SchemaCatalog, CliError>
where
    C: postgres::GenericClient,
{
    rurqe_postgres::introspect::introspect_postgres_selection(client, selection)
        .map_err(catalog_error)
}

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
/// before the catalog is handed on; see [`introspect_database_selection_notice`]
/// for the analysis.
pub fn introspect_database_schema(url: &str, schema: &str) -> Result<SchemaCatalog, CliError> {
    let (catalog, notice) = introspect_database_schema_notice(url, schema)?;

    // A caller of this entry point receives the catalog alone, so the notice is
    // logged here; a command that builds a report of its own reaches it through
    // [`introspect_database_schema_notice`] and never duplicates it on both
    // streams.
    if let Some(notice) = notice {
        tracing::warn!("{notice}");
    }

    Ok(catalog)
}

/// Introspects every schema of `selection` in the live database identified by `url`.
///
/// The whole selection is bound as one array parameter of the catalog statements,
/// so the returned catalog describes the primary schema and every secondary schema
/// the project declares in one round trip per statement. A SQLite database holds a
/// single unnamed catalog, so the selection is ignored there rather than rejected:
/// the same project then introspects both engines without branching on the dialect.
///
/// A selection that describes nothing is reported through the `tracing` facade
/// before the catalog is handed on; see [`introspect_database_selection_notice`]
/// for the analysis.
pub fn introspect_database_selection(
    url: &str,
    selection: &SchemaSelection,
) -> Result<SchemaCatalog, CliError> {
    let (catalog, notice) = introspect_database_selection_notice(url, selection)?;

    // A caller of this entry point receives the catalog alone, so the notice is
    // logged here; a command that builds a report of its own reaches it through
    // [`introspect_database_selection_notice`] and never duplicates it on both
    // streams.
    if let Some(notice) = notice {
        tracing::warn!("{notice}");
    }

    Ok(catalog)
}

/// Introspects `schema` of the live database identified by `url`, reporting a
/// selection that describes nothing.
///
/// The notice is returned instead of being only logged, so a command puts it in
/// the report it prints and the hazard reaches the operator whether or not a
/// `tracing` sink is installed. The single schema is the selection
/// [`introspect_database_selection_notice`] describes, so both entry points run
/// the same detection over the same catalog statements.
pub(crate) fn introspect_database_schema_notice(
    url: &str,
    schema: &str,
) -> Result<(SchemaCatalog, Option<String>), CliError> {
    let mut connection = connect(url)?;
    let notice = report_vacuous_schema(&mut connection, schema)?;

    introspect_connection_schema(&mut connection, schema).map(|catalog| (catalog, notice))
}

/// Introspects every schema of `selection`, reporting a selection that describes
/// nothing.
///
/// The notice is returned instead of being only logged, so a command puts it in
/// the report it prints and the hazard reaches the operator whether or not a
/// `tracing` sink is installed. See [`vacuous_selection_notice`] for the analysis.
pub(crate) fn introspect_database_selection_notice(
    url: &str,
    selection: &SchemaSelection,
) -> Result<(SchemaCatalog, Option<String>), CliError> {
    let mut connection = connect(url)?;
    let notice = report_vacuous_selection(&mut connection, selection)?;

    introspect_connection_selection(&mut connection, selection).map(|catalog| (catalog, notice))
}

/// Introspects `schema` of an already opened [`Connection`].
///
/// Engine dispatch lives next to the catalog logic, so callers (including tests)
/// can hand in an open SQLite or PostgreSQL handle without going through URL
/// parsing. The schema name reaches only the PostgreSQL side; a SQLite connection
/// has no schema layer, so its single catalog is read whatever name the caller
/// passes.
pub(crate) fn introspect_connection_schema(
    connection: &mut Connection,
    schema: &str,
) -> Result<SchemaCatalog, CliError> {
    introspect_connection_selection(connection, &SchemaSelection::single(schema))
}

/// Introspects every schema of `selection` over an already opened [`Connection`].
///
/// Engine dispatch lives next to the catalog logic, so callers (including tests)
/// can hand in an open SQLite or PostgreSQL handle without going through URL
/// parsing. The selection reaches only the PostgreSQL side; a SQLite connection
/// has no schema layer, so its single catalog is read whatever schemas the
/// selection names.
pub(crate) fn introspect_connection_selection(
    connection: &mut Connection,
    selection: &SchemaSelection,
) -> Result<SchemaCatalog, CliError> {
    match connection {
        Connection::Sqlite(conn) => introspect_sqlite(conn).map_err(catalog_error),
        Connection::Postgres(client) => introspect_postgres_selection(client.as_mut(), selection),
    }
}

/// Reports a single selected schema that describes nothing while the database
/// keeps user relations in other schemas.
///
/// The single schema is the selection [`report_vacuous_selection`] analyses, so
/// the entry points that still name one schema run the very detection the
/// multi-schema ones run.
pub(crate) fn report_vacuous_schema(
    connection: &mut Connection,
    schema: &str,
) -> Result<Option<String>, CliError> {
    report_vacuous_selection(connection, &SchemaSelection::single(schema))
}

/// Reports a selection that describes nothing while the database keeps user
/// relations in other schemas.
///
/// A PostgreSQL project may keep its relations in schemas of its own, and a
/// catalog read from the wrong ones is empty rather than wrong: every digest
/// computed from it — the codegen snapshot, the drift gate, the cold-start
/// marker, and the checkpoint hash — then compares nothing against nothing and
/// passes vacuously. The notice names the schemas that do hold relations, so the
/// operator selects one with `--schema` (or the `primary_schema` key of
/// `rurqe.toml`) instead of trusting a verification that never looked at their
/// tables.
pub(crate) fn report_vacuous_selection(
    connection: &mut Connection,
    selection: &SchemaSelection,
) -> Result<Option<String>, CliError> {
    match connection {
        // A SQLite database holds one unnamed catalog, so the selection always
        // describes it whatever schemas it names.
        Connection::Sqlite(_) => Ok(None),
        Connection::Postgres(client) => report_vacuous_postgres_selection(client, selection),
    }
}

/// Reports a selected PostgreSQL selection whose schemas hold no relation while
/// relations live elsewhere.
///
/// The neighbour query answers with the schemas that hold relations outside the
/// selection, so one round trip both proves that no selected schema describes the
/// database and names what the operator selects instead.
pub(crate) fn report_vacuous_postgres_selection(
    client: &mut ::postgres::Client,
    selection: &SchemaSelection,
) -> Result<Option<String>, CliError> {
    let neighbours = rurqe_postgres::introspect::schema_neighbours(client, &selection.all())
        .map_err(catalog_error)?;

    Ok(vacuous_selection_notice(selection, &neighbours))
}

/// The notice a selection that describes nothing produces, when it describes nothing.
///
/// A selection holding a relation of its own describes the database it was asked
/// about, and one asked about a database that keeps no user relation anywhere else
/// either is empty rather than wrong, so both produce no notice at all. The wording
/// names the whole selection and the neighbours, because the neighbours are the
/// schemas the operator selects instead.
pub(crate) fn vacuous_selection_notice(
    selection: &SchemaSelection,
    neighbours: &[String],
) -> Option<String> {
    if neighbours.is_empty() {
        return None;
    }

    let selected = selection.all().join(", ");
    let names = neighbours.join(", ");

    Some(format!(
        "warning: none of the selected schemas ({selected}) holds a table or view while the database \
         keeps user relations in {names}; the catalog digest computed from the selection describes \
         nothing, so pass `--schema <name>` or set `primary_schema = \"<name>\"` in `rurqe.toml`"
    ))
}

/// Moves a catalog failure of an engine adapter into the CLI error type.
///
/// The adapter already names the statement it ran and the column it could not
/// read, so the wording is carried over unchanged and only the error type changes
/// at the boundary.
fn catalog_error(error: impl std::fmt::Display) -> CliError {
    CliError(error.to_string())
}
