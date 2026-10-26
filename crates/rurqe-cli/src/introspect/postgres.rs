//! PostgreSQL catalog introspection through `information_schema` and `pg_catalog`.
//!
//! Every statement binds the introspected schema name as its first parameter, so
//! the same catalog text serves the user-facing `public` schema and the
//! disposable scratch schema the golden equation is verified in.
//!
//! The catalog statements live in [`queries`] next to the doc comments that
//! justify them, so this facade carries the decode path alone.
//!
//! Rows are decoded through `Row::try_get`, so a column whose type the driver
//! cannot read surfaces as a diagnostic naming the catalog query and the column
//! instead of a panic.

use rurqe_migrate::catalog::SchemaCatalog;

use crate::cli::CliError;

use super::types::{
    catalog_from_metadata_with_sections, parse_foreign_key_action, RawCatalogSections,
    RawCheckMetadata, RawColumnMetadata, RawEnumMetadata, RawForeignKeyMetadata, RawIndexMetadata,
    RawPrimaryKeyMetadata, RawTableMetadata, RawTriggerMetadata, RawViewMetadata,
};

mod queries;

#[cfg(test)]
mod tests;

// The catalog statements keep their original path, so a consumer names the query
// through the introspection module whether it reads it from the facade or from
// the module holding the SQL text.
pub(crate) use self::queries::{
    POSTGRES_CHECKS, POSTGRES_COLUMNS, POSTGRES_ENUMS, POSTGRES_FOREIGN_KEYS, POSTGRES_INDEXES,
    POSTGRES_PRIMARY_KEYS, POSTGRES_SCHEMA_NEIGHBOURS, POSTGRES_TABLES, POSTGRES_TRIGGERS,
    POSTGRES_VIEWS,
};

/// Reads the schemas that keep user relations while `schema` keeps none.
///
/// The call answers with an empty list when `schema` describes the database it is
/// asked about, so a caller reports a vacuous selection from one round trip.
pub(crate) fn schema_neighbours(
    client: &mut postgres::Client,
    schema: &str,
) -> Result<Vec<String>, CliError> {
    let rows = client
        .query(POSTGRES_SCHEMA_NEIGHBOURS, &[&schema])
        .map_err(|error| CliError(format!("failed to query PostgreSQL schemas: {error}")))?;

    rows.iter()
        .map(|row| read_column(row, 0, "schemas", "table_schema"))
        .collect()
}

/// Introspects the `public` schema of a live PostgreSQL database client.
pub fn introspect_postgres(client: &mut postgres::Client) -> Result<SchemaCatalog, CliError> {
    introspect_postgres_schema(client, "public")
}

/// Introspects `schema` over any PostgreSQL connection.
///
/// The schema name is bound as a parameter rather than being rendered into the
/// statement text, so no caller can smuggle SQL into the catalog queries.
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
    let raw_tables = query_catalog(client, POSTGRES_TABLES, schema, "tables", |row| {
        Ok(RawTableMetadata {
            name: read_column(row, 0, "tables", "table_name")?,
        })
    })?;

    let raw_columns = query_catalog(client, POSTGRES_COLUMNS, schema, "columns", |row| {
        Ok(RawColumnMetadata {
            table_name: read_column(row, 0, "columns", "table_name")?,
            column_name: read_column(row, 1, "columns", "column_name")?,
            data_type: read_column(row, 2, "columns", "data_type")?,
            is_nullable: read_column(row, 3, "columns", "is_nullable")?,
            column_default: read_column(row, 4, "columns", "column_default")?,
            udt_name: read_column(row, 5, "columns", "udt_name")?,
        })
    })?;

    let raw_pks = query_catalog(
        client,
        POSTGRES_PRIMARY_KEYS,
        schema,
        "primary keys",
        |row| {
            Ok(RawPrimaryKeyMetadata {
                table_name: read_column(row, 0, "primary keys", "table_name")?,
                column_name: read_column(row, 1, "primary keys", "column_name")?,
            })
        },
    )?;

    let raw_fks = query_catalog(
        client,
        POSTGRES_FOREIGN_KEYS,
        schema,
        "foreign keys",
        |row| {
            let on_delete: String = read_column(row, 5, "foreign keys", "on_delete")?;
            let on_update: String = read_column(row, 6, "foreign keys", "on_update")?;

            Ok(RawForeignKeyMetadata {
                name: read_column(row, 0, "foreign keys", "conname")?,
                table_name: read_column(row, 1, "foreign keys", "table_name")?,
                column_name: read_column(row, 2, "foreign keys", "column_name")?,
                referenced_table: read_column(row, 3, "foreign keys", "referenced_table")?,
                referenced_column: read_column(row, 4, "foreign keys", "referenced_column")?,
                on_delete: parse_foreign_key_action(&on_delete),
                on_update: parse_foreign_key_action(&on_update),
            })
        },
    )?;

    let raw_indexes = query_catalog(client, POSTGRES_INDEXES, schema, "indexes", |row| {
        Ok(RawIndexMetadata {
            table_name: read_column(row, 0, "indexes", "table_name")?,
            index_name: read_column(row, 1, "indexes", "index_name")?,
            unique: read_column(row, 2, "indexes", "is_unique")?,
            method: read_column(row, 3, "indexes", "method")?,
            predicate: read_column(row, 4, "indexes", "predicate")?,
            expression: read_column(row, 5, "indexes", "expression")?,
            column_name: read_column(row, 6, "indexes", "column_name")?,
        })
    })?;

    let raw_enums = query_catalog(client, POSTGRES_ENUMS, schema, "enums", |row| {
        Ok(RawEnumMetadata {
            name: read_column(row, 0, "enums", "enum_name")?,
            variant: read_column(row, 1, "enums", "variant_name")?,
        })
    })?;

    let raw_checks = query_catalog(client, POSTGRES_CHECKS, schema, "checks", |row| {
        Ok(RawCheckMetadata {
            table_name: read_column(row, 0, "checks", "table_name")?,
            name: read_column(row, 1, "checks", "constraint_name")?,
            expression: read_column(row, 2, "checks", "expression")?,
        })
    })?;

    let raw_views = query_catalog(client, POSTGRES_VIEWS, schema, "views", |row| {
        Ok(RawViewMetadata {
            name: read_column(row, 0, "views", "view_name")?,
            definition: read_column(row, 1, "views", "view_definition")?,
        })
    })?;

    let raw_triggers = query_catalog(client, POSTGRES_TRIGGERS, schema, "triggers", |row| {
        Ok(RawTriggerMetadata {
            name: read_column(row, 0, "triggers", "trigger_name")?,
            table_name: read_column(row, 1, "triggers", "table_name")?,
            definition: read_column(row, 2, "triggers", "definition")?,
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

/// Runs a catalog statement with `schema` bound as its only parameter.
///
/// The schema name is bound rather than rendered, so no caller can smuggle SQL
/// into the catalog queries, and every returned row is decoded by `decode`
/// without panicking on a column type the driver cannot read.
fn query_catalog<T, C>(
    client: &mut C,
    statement: &str,
    schema: &str,
    label: &str,
    decode: impl Fn(&postgres::Row) -> Result<T, CliError>,
) -> Result<Vec<T>, CliError>
where
    C: postgres::GenericClient,
{
    let rows = client
        .query(statement, &[&schema])
        .map_err(|error| CliError(format!("failed to query PostgreSQL {label}: {error}")))?;

    rows.iter().map(decode).collect()
}

/// Reads the column at `index` of `row` as a `T`, naming the query and column.
///
/// A value the driver cannot decode into `T` is reported as a [`CliError`];
/// `Row::get` would panic instead.
fn read_column<'a, T>(
    row: &'a postgres::Row,
    index: usize,
    statement: &str,
    column: &str,
) -> Result<T, CliError>
where
    T: postgres::types::FromSql<'a>,
{
    row.try_get(index)
        .map_err(|error| column_error(statement, column, error))
}

/// Describes a catalog column the driver could not decode.
///
/// The description is reachable from the sibling `tests` module, so the wording
/// that reaches the operator is pinned where the decoder lives.
pub(super) fn column_error(
    statement: &str,
    column: &str,
    error: impl std::fmt::Display,
) -> CliError {
    CliError(format!(
        "failed to read column `{column}` from the PostgreSQL {statement} query: {error}"
    ))
}
