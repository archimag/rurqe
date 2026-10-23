//! PostgreSQL catalog introspection through `information_schema` and `pg_catalog`.
//!
//! Every statement binds the introspected schema name as its first parameter, so
//! the same catalog text serves the user-facing `public` schema and the
//! disposable scratch schema the golden equation is verified in.
//!
//! Rows are decoded through `Row::try_get`, so a column whose type the driver
//! cannot read surfaces as a diagnostic naming the catalog query and the column
//! instead of a panic.

use rurqe_migrate::catalog::SchemaCatalog;

use crate::cli::CliError;

use super::types::{
    catalog_from_metadata, RawColumnMetadata, RawEnumMetadata, RawForeignKeyMetadata,
    RawIndexMetadata, RawPrimaryKeyMetadata, RawTableMetadata,
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
///
/// A domain is a type of its own, while a column declared over it stores the
/// values of the type the domain wraps. `information_schema.columns` names the
/// domain instead of that base type, so the declared type is resolved through
/// `pg_type.typbasetype` and the base type is reported in its place: the catalog
/// then carries the type the column really holds rather than a name no
/// introspection can classify. A column of a plain type is reported as it is,
/// and a domain declared over another domain is followed to its end.
pub(crate) const POSTGRES_COLUMNS: &str = r#"
WITH RECURSIVE type_chain AS (
    SELECT declared.oid AS declared, declared.oid AS resolved, 0 AS depth
    FROM pg_type declared
    UNION ALL
    SELECT chain.declared, base.oid, chain.depth + 1
    FROM type_chain chain
    JOIN pg_type domain_type
      ON domain_type.oid = chain.resolved
     AND domain_type.typtype = 'd'
    JOIN pg_type base ON base.oid = domain_type.typbasetype
),
resolved_type AS (
    SELECT DISTINCT ON (declared) declared, resolved
    FROM type_chain
    ORDER BY declared, depth DESC
)
SELECT
  column_row.table_name,
  column_row.column_name,
  CASE
    WHEN resolved_type.resolved IS NULL OR resolved_type.resolved = declared_type.oid
      THEN column_row.data_type
    ELSE format_type(resolved_type.resolved, NULL)
  END AS data_type,
  (column_row.is_nullable = 'YES') AS is_nullable,
  column_row.column_default,
  CASE
    WHEN resolved_type.resolved IS NULL OR resolved_type.resolved = declared_type.oid
      THEN column_row.udt_name
    ELSE base_type.typname
  END AS udt_name
FROM information_schema.columns column_row
LEFT JOIN pg_namespace type_namespace
  ON type_namespace.nspname = column_row.udt_schema
LEFT JOIN pg_type declared_type
  ON declared_type.typname = column_row.udt_name
 AND declared_type.typnamespace = type_namespace.oid
LEFT JOIN resolved_type
  ON resolved_type.declared = declared_type.oid
LEFT JOIN pg_type base_type
  ON base_type.oid = resolved_type.resolved
WHERE column_row.table_schema = $1
  AND column_row.table_name != '__rurqe_schema_history'
ORDER BY column_row.table_name, column_row.ordinal_position;
"#;

/// Query to fetch all user enum types and their variants in sort order.
pub(crate) const POSTGRES_ENUMS: &str = r#"
SELECT
  t.typname AS enum_name,
  e.enumlabel AS variant_name
FROM pg_type t
JOIN pg_enum e ON t.oid = e.enumtypid
JOIN pg_namespace n ON n.oid = t.typnamespace
WHERE n.nspname = $1
ORDER BY t.typname, e.enumsortorder;
"#;

/// Query to fetch all primary key constraint columns.
pub(crate) const POSTGRES_PRIMARY_KEYS: &str = r#"
SELECT kcu.table_name, kcu.column_name
FROM information_schema.table_constraints tc
JOIN information_schema.key_column_usage kcu
  ON tc.constraint_name = kcu.constraint_name
 AND tc.table_schema = kcu.table_schema
 AND tc.table_name = kcu.table_name
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

/// Query to fetch the secondary indexes of the introspected schema.
///
/// `pg_index.indkey` is an `int2vector`, so it is cast to `int2[]` before its
/// subscripts are generated: an ordinary SQL array is one-based, which keeps the
/// positional lookup of the indexed attribute aligned with the generated
/// subscript. Primary-key indexes are excluded through `indisprimary`, because a
/// primary key is already described by the key columns of its table.
///
/// Only the key attributes describe what an index searches: `indkey` continues
/// with the non-key `INCLUDE` columns, which an index does not order or restrict
/// rows by, so the subscripts are bounded by `indnkeyatts` and the trailing
/// attributes never reach the catalog.
///
/// An index carrying an expression stores a zero attribute number in its place,
/// and an expression has no column name to report. Such an index is skipped as a
/// whole, exactly like the SQLite introspection skips an expression index, so a
/// catalog never describes a mixed index by the columns alone.
pub(crate) const POSTGRES_INDEXES: &str = r#"
SELECT
  index_table.relname AS table_name,
  index_class.relname AS index_name,
  index_row.indisunique AS is_unique,
  attribute.attname AS column_name
FROM pg_index index_row
JOIN pg_class index_class ON index_class.oid = index_row.indexrelid
JOIN pg_class index_table ON index_table.oid = index_row.indrelid
JOIN pg_namespace namespace ON namespace.oid = index_table.relnamespace
JOIN generate_subscripts(index_row.indkey::int2[], 1) AS position(index) ON TRUE
JOIN pg_attribute attribute
  ON attribute.attrelid = index_row.indrelid
 AND attribute.attnum = (index_row.indkey::int2[])[position.index]
WHERE namespace.nspname = $1
  AND NOT index_row.indisprimary
  AND index_row.indnkeyatts >= position.index
  AND NOT (0 = ANY(index_row.indkey::int2[]))
ORDER BY index_table.relname, index_class.relname, position.index;
"#;

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
pub fn introspect_postgres_schema<C>(client: &mut C, schema: &str) -> Result<SchemaCatalog, CliError>
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

    let raw_pks = query_catalog(client, POSTGRES_PRIMARY_KEYS, schema, "primary keys", |row| {
        Ok(RawPrimaryKeyMetadata {
            table_name: read_column(row, 0, "primary keys", "table_name")?,
            column_name: read_column(row, 1, "primary keys", "column_name")?,
        })
    })?;

    let raw_fks = query_catalog(client, POSTGRES_FOREIGN_KEYS, schema, "foreign keys", |row| {
        Ok(RawForeignKeyMetadata {
            name: read_column(row, 0, "foreign keys", "conname")?,
            table_name: read_column(row, 1, "foreign keys", "table_name")?,
            column_name: read_column(row, 2, "foreign keys", "column_name")?,
            referenced_table: read_column(row, 3, "foreign keys", "referenced_table")?,
            referenced_column: read_column(row, 4, "foreign keys", "referenced_column")?,
        })
    })?;

    let raw_indexes = query_catalog(client, POSTGRES_INDEXES, schema, "indexes", |row| {
        Ok(RawIndexMetadata {
            table_name: read_column(row, 0, "indexes", "table_name")?,
            index_name: read_column(row, 1, "indexes", "index_name")?,
            unique: read_column(row, 2, "indexes", "is_unique")?,
            column_name: read_column(row, 3, "indexes", "column_name")?,
        })
    })?;

    let raw_enums = query_catalog(client, POSTGRES_ENUMS, schema, "enums", |row| {
        Ok(RawEnumMetadata {
            name: read_column(row, 0, "enums", "enum_name")?,
            variant: read_column(row, 1, "enums", "variant_name")?,
        })
    })?;

    Ok(catalog_from_metadata(
        &raw_tables,
        &raw_columns,
        &raw_pks,
        &raw_fks,
        &raw_indexes,
        &raw_enums,
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
fn column_error(statement: &str, column: &str, error: impl std::fmt::Display) -> CliError {
    CliError(format!(
        "failed to read column `{column}` from the PostgreSQL {statement} query: {error}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_primary_key_statement_matches_columns_within_their_table() {
        // A constraint name is only unique per table, so a schema-wide match on the
        // name alone folds the columns of a foreign key or a unique constraint of
        // another table into the primary key it shares the name with.
        assert!(
            POSTGRES_PRIMARY_KEYS.contains("tc.table_name = kcu.table_name"),
            "the primary key query must constrain the join to one table"
        );
    }

    #[test]
    fn the_index_statement_reads_only_the_key_attributes() {
        assert!(
            POSTGRES_INDEXES.contains("index_row.indnkeyatts >= position.index"),
            "the index query must stop before the non-key INCLUDE columns"
        );
    }

    #[test]
    fn the_index_statement_skips_expression_indexes() {
        assert!(
            POSTGRES_INDEXES.contains("NOT (0 = ANY(index_row.indkey::int2[]))"),
            "the index query must skip an index carrying an expression"
        );
    }

    #[test]
    fn a_undecodable_column_names_the_query_and_the_column() {
        let error = column_error("columns", "column_default", "unexpected type");

        let message = error.to_string();
        assert!(
            message.contains("`column_default`"),
            "unexpected message: {message}"
        );
        assert!(
            message.contains("PostgreSQL columns query"),
            "unexpected message: {message}"
        );
        assert!(
            message.contains("unexpected type"),
            "unexpected message: {message}"
        );
    }
}
