//! PostgreSQL catalog introspection through `information_schema` and `pg_catalog`.
//!
//! Every statement binds the introspected schema name as its first parameter, so
//! the same catalog text serves the user-facing `public` schema and the
//! disposable scratch schema the golden equation is verified in.

use rurqe_migrate::catalog::SchemaCatalog;

use crate::cli::CliError;

use super::types::{
    catalog_from_metadata, RawColumnMetadata, RawForeignKeyMetadata, RawPrimaryKeyMetadata,
    RawTableMetadata,
};

/// Query to fetch all user tables of the introspected schema.
pub(crate) const POSTGRES_TABLES: &str = r#"
SELECT table_name
FROM information_schema.tables
WHERE table_schema = $1
  AND table_type = 'BASE TABLE'
  AND table_name != '__rurqe_schema_history'
ORDER BY table_name;
"#;

/// Query to fetch all columns of the introspected schema.
pub(crate) const POSTGRES_COLUMNS: &str = r#"
SELECT table_name, column_name, data_type, (is_nullable = 'YES') as is_nullable, column_default
FROM information_schema.columns
WHERE table_schema = $1
  AND table_name != '__rurqe_schema_history'
ORDER BY table_name, ordinal_position;
"#;

/// Query to fetch all primary key constraint columns.
pub(crate) const POSTGRES_PRIMARY_KEYS: &str = r#"
SELECT kcu.table_name, kcu.column_name
FROM information_schema.table_constraints tc
JOIN information_schema.key_column_usage kcu
  ON tc.constraint_name = kcu.constraint_name
 AND tc.table_schema = kcu.table_schema
WHERE tc.constraint_type = 'PRIMARY KEY'
  AND tc.table_schema = $1
ORDER BY kcu.table_name, kcu.ordinal_position;
"#;

/// Query to fetch all foreign key columns in key order.
///
/// `information_schema.constraint_column_usage` loses the positional
/// relationship between referencing and referenced columns, so the catalog is
/// read through `pg_constraint` and the `conkey` / `confkey` arrays are
/// unnested together with `generate_subscripts` to keep composite keys aligned.
pub(crate) const POSTGRES_FOREIGN_KEYS: &str = r#"
SELECT
  constraint_row.conname,
  owning.relname AS table_name,
  owning_column.attname AS column_name,
  referenced.relname AS referenced_table,
  referenced_column.attname AS referenced_column
FROM pg_constraint constraint_row
JOIN pg_class owning ON owning.oid = constraint_row.conrelid
JOIN pg_namespace namespace ON namespace.oid = owning.relnamespace
JOIN pg_class referenced ON referenced.oid = constraint_row.confrelid
JOIN generate_subscripts(constraint_row.conkey, 1) AS position(index) ON TRUE
JOIN pg_attribute owning_column
  ON owning_column.attrelid = constraint_row.conrelid
 AND owning_column.attnum = constraint_row.conkey[position.index]
JOIN pg_attribute referenced_column
  ON referenced_column.attrelid = constraint_row.confrelid
 AND referenced_column.attnum = constraint_row.confkey[position.index]
WHERE constraint_row.contype = 'f'
  AND namespace.nspname = $1
ORDER BY owning.relname, constraint_row.conname, position.index;
"#;

/// Introspects the `public` schema of a live PostgreSQL database client.
pub fn introspect_postgres(client: &mut postgres::Client) -> Result<SchemaCatalog, CliError> {
    introspect_postgres_schema(client, "public")
}

/// Introspects `schema` of a live PostgreSQL database client.
///
/// The schema name is bound as a parameter rather than being rendered into the
/// statement text, so no caller can smuggle SQL into the catalog queries.
pub fn introspect_postgres_schema(
    client: &mut postgres::Client,
    schema: &str,
) -> Result<SchemaCatalog, CliError> {
    let table_rows = client
        .query(POSTGRES_TABLES, &[&schema])
        .map_err(|e| CliError(format!("failed to query PostgreSQL tables: {e}")))?;
    let raw_tables: Vec<RawTableMetadata> = table_rows
        .into_iter()
        .map(|row| RawTableMetadata { name: row.get(0) })
        .collect();

    let col_rows = client
        .query(POSTGRES_COLUMNS, &[&schema])
        .map_err(|e| CliError(format!("failed to query PostgreSQL columns: {e}")))?;
    let raw_columns: Vec<RawColumnMetadata> = col_rows
        .into_iter()
        .map(|row| RawColumnMetadata {
            table_name: row.get(0),
            column_name: row.get(1),
            data_type: row.get(2),
            is_nullable: row.get(3),
            column_default: row.get(4),
        })
        .collect();

    let pk_rows = client
        .query(POSTGRES_PRIMARY_KEYS, &[&schema])
        .map_err(|e| CliError(format!("failed to query PostgreSQL primary keys: {e}")))?;
    let raw_pks: Vec<RawPrimaryKeyMetadata> = pk_rows
        .into_iter()
        .map(|row| RawPrimaryKeyMetadata {
            table_name: row.get(0),
            column_name: row.get(1),
        })
        .collect();

    let fk_rows = client
        .query(POSTGRES_FOREIGN_KEYS, &[&schema])
        .map_err(|e| CliError(format!("failed to query PostgreSQL foreign keys: {e}")))?;
    let raw_fks: Vec<RawForeignKeyMetadata> = fk_rows
        .into_iter()
        .map(|row| RawForeignKeyMetadata {
            name: row.get(0),
            table_name: row.get(1),
            column_name: row.get(2),
            referenced_table: row.get(3),
            referenced_column: row.get(4),
        })
        .collect();

    Ok(catalog_from_metadata(
        &raw_tables,
        &raw_columns,
        &raw_pks,
        &raw_fks,
    ))
}
