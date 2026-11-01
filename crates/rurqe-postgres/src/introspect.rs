//! PostgreSQL catalog introspection through `information_schema` and `pg_catalog`.
//!
//! Every statement binds the introspected schema list as its first parameter, so
//! the same catalog text serves the user-facing schemas and the disposable
//! scratch schema a stage verification builds. The statements live in the
//! [`queries`] module next to the doc comments that justify them, so this facade
//! carries the decode path alone.
//!
//! Rows are decoded through `Row::try_get`, so a column whose type the driver
//! cannot read surfaces as a [`PgError::Catalog`] naming the catalog query and the
//! column instead of a panic.

use rurqe_migrate::catalog::SchemaCatalog;
use rurqe_migrate::introspect::{
    catalog_from_metadata_with_sections, parse_foreign_key_action, RawCatalogSections,
    RawCheckMetadata, RawColumnMetadata, RawEnumMetadata, RawForeignKeyMetadata, RawIndexMetadata,
    RawPrimaryKeyMetadata, RawTableMetadata, RawTriggerMetadata, RawViewMetadata, SchemaSelection,
};

use crate::backend::types::PgError;

mod queries;

pub use self::queries::{
    POSTGRES_CHECKS, POSTGRES_COLUMNS, POSTGRES_ENUMS, POSTGRES_FOREIGN_KEYS, POSTGRES_INDEXES,
    POSTGRES_PRIMARY_KEYS, POSTGRES_SCHEMA_NEIGHBOURS, POSTGRES_TABLES, POSTGRES_TRIGGERS,
    POSTGRES_VIEWS,
};

/// Name of the schema introspected when the caller names none.
///
/// A PostgreSQL database keeps its tables in named schemas, and `public` is the
/// one an ordinary single-schema deployment uses. A project keeping its relations
/// elsewhere names its own schemas through a [`SchemaSelection`].
pub const DEFAULT_SCHEMA: &str = "public";

/// Reads the schemas that keep user relations while none of `schemas` keeps one.
///
/// The call answers with an empty list as soon as one selected schema describes
/// the database it is asked about, so a caller reports a vacuous selection from
/// one round trip.
pub fn schema_neighbours(
    client: &mut postgres::Client,
    schemas: &[String],
) -> Result<Vec<String>, PgError> {
    let rows = client
        .query(POSTGRES_SCHEMA_NEIGHBOURS, &[&schemas])
        .map_err(|error| query_error("schemas", error))?;

    rows.iter()
        .map(|row| read_column(row, 0, "schemas", "table_schema"))
        .collect()
}

/// Introspects the `public` schema of a live PostgreSQL database client.
pub fn introspect_postgres(client: &mut postgres::Client) -> Result<SchemaCatalog, PgError> {
    introspect_postgres_schema(client, DEFAULT_SCHEMA)
}

/// Introspects `schema` over any PostgreSQL connection.
///
/// The single schema is the selection [`introspect_postgres_selection`]
/// introspects, so a session that reads one schema runs the very catalog
/// statements the multi-schema entry point runs.
///
/// The connection is named through [`postgres::GenericClient`], so the same
/// catalog statements run both on a bare client and inside an open transaction:
/// a scratch schema can therefore be introspected before its transaction
/// commits, and the golden equation reads it through the same queries.
pub fn introspect_postgres_schema<C>(
    client: &mut C,
    schema: &str,
) -> Result<SchemaCatalog, PgError>
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
/// schemas in one round trip.
///
/// A schema name a row reports is reduced to the catalog convention while it is
/// decoded: a row of the primary schema carries no attribution, and a row of a
/// secondary schema carries that schema. A single-schema introspection therefore
/// reproduces the catalog recorded before schemas were read at all, byte for byte.
///
/// The connection is named through [`postgres::GenericClient`], so the same
/// catalog statements run both on a bare client and inside an open transaction:
/// a scratch schema can therefore be introspected before its transaction
/// commits, and the golden equation reads it through the same queries.
pub fn introspect_postgres_selection<C>(
    client: &mut C,
    selection: &SchemaSelection,
) -> Result<SchemaCatalog, PgError>
where
    C: postgres::GenericClient,
{
    // The catalog statements take the whole selection as one array parameter, and
    // the primary schema is the one whose rows carry no attribution.
    let schemas = selection.all();
    let schema = selection.primary();
    let raw_tables = query_catalog(client, POSTGRES_TABLES, &schemas, "tables", |row| {
        Ok(RawTableMetadata {
            schema: read_schema(row, 0, schema, "tables", "table_schema")?,
            name: read_column(row, 1, "tables", "table_name")?,
        })
    })?;

    let raw_columns = query_catalog(client, POSTGRES_COLUMNS, &schemas, "columns", |row| {
        Ok(RawColumnMetadata {
            schema: read_schema(row, 0, schema, "columns", "table_schema")?,
            table_name: read_column(row, 1, "columns", "table_name")?,
            column_name: read_column(row, 2, "columns", "column_name")?,
            data_type: read_column(row, 3, "columns", "data_type")?,
            is_nullable: read_column(row, 4, "columns", "is_nullable")?,
            column_default: read_column(row, 5, "columns", "column_default")?,
            udt_name: read_column(row, 6, "columns", "udt_name")?,
            udt_schema: read_optional_schema(row, 7, schema, "columns", "udt_schema")?,
        })
    })?;

    let raw_pks = query_catalog(
        client,
        POSTGRES_PRIMARY_KEYS,
        &schemas,
        "primary keys",
        |row| {
            Ok(RawPrimaryKeyMetadata {
                schema: read_schema(row, 0, schema, "primary keys", "table_schema")?,
                table_name: read_column(row, 1, "primary keys", "table_name")?,
                column_name: read_column(row, 2, "primary keys", "column_name")?,
            })
        },
    )?;

    let raw_fks = query_catalog(
        client,
        POSTGRES_FOREIGN_KEYS,
        &schemas,
        "foreign keys",
        |row| {
            let referenced_schema: String =
                read_column(row, 6, "foreign keys", "referenced_schema")?;
            let on_delete: String = read_column(row, 7, "foreign keys", "on_delete")?;
            let on_update: String = read_column(row, 8, "foreign keys", "on_update")?;

            Ok(RawForeignKeyMetadata {
                schema: read_schema(row, 0, schema, "foreign keys", "table_schema")?,
                name: read_column(row, 1, "foreign keys", "conname")?,
                table_name: read_column(row, 2, "foreign keys", "table_name")?,
                column_name: read_column(row, 3, "foreign keys", "column_name")?,
                referenced_table: read_column(row, 4, "foreign keys", "referenced_table")?,
                referenced_column: read_column(row, 5, "foreign keys", "referenced_column")?,
                // A reference into the primary schema records no attribution of
                // its own, exactly as the owning table records none.
                referenced_schema: normalize_schema(&referenced_schema, schema),
                on_delete: parse_foreign_key_action(&on_delete),
                on_update: parse_foreign_key_action(&on_update),
            })
        },
    )?;

    let raw_indexes = query_catalog(client, POSTGRES_INDEXES, &schemas, "indexes", |row| {
        Ok(RawIndexMetadata {
            schema: read_schema(row, 0, schema, "indexes", "table_schema")?,
            table_name: read_column(row, 1, "indexes", "table_name")?,
            index_name: read_column(row, 2, "indexes", "index_name")?,
            unique: read_column(row, 3, "indexes", "is_unique")?,
            method: read_column(row, 4, "indexes", "method")?,
            predicate: read_column(row, 5, "indexes", "predicate")?,
            expression: read_column(row, 6, "indexes", "expression")?,
            column_name: read_column(row, 7, "indexes", "column_name")?,
        })
    })?;

    let raw_enums = query_catalog(client, POSTGRES_ENUMS, &schemas, "enums", |row| {
        Ok(RawEnumMetadata {
            schema: read_schema(row, 0, schema, "enums", "schema_name")?,
            name: read_column(row, 1, "enums", "enum_name")?,
            variant: read_column(row, 2, "enums", "variant_name")?,
        })
    })?;

    let raw_checks = query_catalog(client, POSTGRES_CHECKS, &schemas, "checks", |row| {
        Ok(RawCheckMetadata {
            schema: read_schema(row, 0, schema, "checks", "table_schema")?,
            table_name: read_column(row, 1, "checks", "table_name")?,
            name: read_column(row, 2, "checks", "constraint_name")?,
            expression: read_column(row, 3, "checks", "expression")?,
        })
    })?;

    let raw_views = query_catalog(client, POSTGRES_VIEWS, &schemas, "views", |row| {
        Ok(RawViewMetadata {
            schema: read_schema(row, 0, schema, "views", "table_schema")?,
            name: read_column(row, 1, "views", "view_name")?,
            definition: read_column(row, 2, "views", "view_definition")?,
        })
    })?;

    let raw_triggers = query_catalog(client, POSTGRES_TRIGGERS, &schemas, "triggers", |row| {
        Ok(RawTriggerMetadata {
            schema: read_schema(row, 0, schema, "triggers", "table_schema")?,
            name: read_column(row, 1, "triggers", "trigger_name")?,
            table_name: read_column(row, 2, "triggers", "table_name")?,
            definition: read_column(row, 3, "triggers", "definition")?,
        })
    })?;

    Ok(catalog_from_metadata_with_sections(
        &raw_tables,
        &raw_columns,
        &raw_pks,
        &raw_fks,
        &raw_indexes,
        &raw_enums,
        &RawCatalogSections {
            checks: &raw_checks,
            views: &raw_views,
            triggers: &raw_triggers,
        },
    ))
}

/// Runs a catalog statement with the schema list bound as its only parameter.
///
/// The list is bound rather than rendered, so no caller can smuggle SQL into the
/// catalog queries, and every returned row is decoded by `decode` without
/// panicking on a column type the driver cannot read.
fn query_catalog<T, C>(
    client: &mut C,
    statement: &str,
    schemas: &[String],
    label: &str,
    decode: impl Fn(&postgres::Row) -> Result<T, PgError>,
) -> Result<Vec<T>, PgError>
where
    C: postgres::GenericClient,
{
    let rows = client
        .query(statement, &[&schemas])
        .map_err(|error| query_error(label, error))?;

    rows.iter().map(decode).collect()
}

/// Reads the column at `index` of `row` as a `T`, naming the query and column.
///
/// A value the driver cannot decode into `T` is reported as a [`PgError::Catalog`];
/// `Row::get` would panic instead.
fn read_column<'a, T>(
    row: &'a postgres::Row,
    index: usize,
    statement: &str,
    column: &str,
) -> Result<T, PgError>
where
    T: postgres::types::FromSql<'a>,
{
    row.try_get(index)
        .map_err(|error| column_error(statement, column, error))
}

/// Reduces a schema name reported by the catalog to the catalog convention.
///
/// The catalog records a schema attribute only for a secondary schema, so the
/// name of the primary schema decodes to no attribution at all: a single-schema
/// introspection then reproduces the catalog of an older snapshot byte for byte.
/// The name of any other schema is kept as the catalog reported it.
fn normalize_schema(name: &str, primary: &str) -> Option<String> {
    (name != primary).then(|| name.to_owned())
}

/// Reads the schema at `index` of `row`, reducing the primary schema to `None`.
fn read_schema(
    row: &postgres::Row,
    index: usize,
    primary: &str,
    statement: &str,
    column: &str,
) -> Result<Option<String>, PgError> {
    let name: String = read_column(row, index, statement, column)?;
    Ok(normalize_schema(&name, primary))
}

/// Reads an optional schema at `index` of `row` under the same reduction.
fn read_optional_schema(
    row: &postgres::Row,
    index: usize,
    primary: &str,
    statement: &str,
    column: &str,
) -> Result<Option<String>, PgError> {
    let name: Option<String> = read_column(row, index, statement, column)?;
    Ok(name
        .as_deref()
        .and_then(|name| normalize_schema(name, primary)))
}

/// Describes a catalog column the driver could not decode.
///
/// The description names the catalog query and the column, so the operator knows
/// which introspection statement produced the failure instead of reading a
/// driver error stripped of its context.
pub fn column_error(statement: &str, column: &str, error: impl std::fmt::Display) -> PgError {
    PgError::Catalog(format!(
        "failed to read column `{column}` from the {statement} query: {error}"
    ))
}

/// Describes a catalog statement the driver refused.
fn query_error(label: &str, error: impl std::fmt::Display) -> PgError {
    PgError::Catalog(format!("the {label} query failed: {error}"))
}
