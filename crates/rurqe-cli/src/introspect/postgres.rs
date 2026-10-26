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
    catalog_from_metadata_with_sections, parse_foreign_key_action, RawCatalogSections,
    RawCheckMetadata, RawColumnMetadata, RawEnumMetadata, RawForeignKeyMetadata, RawIndexMetadata,
    RawPrimaryKeyMetadata, RawTableMetadata, RawTriggerMetadata, RawViewMetadata,
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
///
/// `confdeltype` and `confupdtype` carry the referential actions of the key as
/// single letters, which are spelled out here so the catalog receives the action
/// itself rather than a driver-level character code.
pub(crate) const POSTGRES_FOREIGN_KEYS: &str = r#"
SELECT
  constraint_row.conname,
  owning.relname AS table_name,
  owning_column.attname AS column_name,
  referenced.relname AS referenced_table,
  referenced_column.attname AS referenced_column,
  CASE constraint_row.confdeltype
    WHEN 'c' THEN 'CASCADE'
    WHEN 'r' THEN 'RESTRICT'
    WHEN 'n' THEN 'SET NULL'
    WHEN 'd' THEN 'SET DEFAULT'
    ELSE 'NO ACTION'
  END AS on_delete,
  CASE constraint_row.confupdtype
    WHEN 'c' THEN 'CASCADE'
    WHEN 'r' THEN 'RESTRICT'
    WHEN 'n' THEN 'SET NULL'
    WHEN 'd' THEN 'SET DEFAULT'
    ELSE 'NO ACTION'
  END AS on_update
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
/// and `pg_attribute` matches no row for that entry. The join is therefore an
/// outer one, so the index survives with an unnamed key entry, its expression
/// read from `indexprs` and its partial predicate from `indpred`. Dropping such
/// an index instead would make an expression index invisible to the snapshot and
/// to every drift check.
pub(crate) const POSTGRES_INDEXES: &str = r#"
SELECT
  index_table.relname AS table_name,
  index_class.relname AS index_name,
  index_row.indisunique AS is_unique,
  access_method.amname AS method,
  pg_get_expr(index_row.indpred, index_row.indrelid) AS predicate,
  pg_get_expr(index_row.indexprs, index_row.indrelid) AS expression,
  attribute.attname AS column_name
FROM pg_index index_row
JOIN pg_class index_class ON index_class.oid = index_row.indexrelid
JOIN pg_class index_table ON index_table.oid = index_row.indrelid
JOIN pg_namespace namespace ON namespace.oid = index_table.relnamespace
JOIN pg_am access_method ON access_method.oid = index_class.relam
JOIN generate_subscripts(index_row.indkey::int2[], 1) AS position(index) ON TRUE
LEFT JOIN pg_attribute attribute
  ON attribute.attrelid = index_row.indrelid
 AND attribute.attnum = (index_row.indkey::int2[])[position.index]
WHERE namespace.nspname = $1
  AND NOT index_row.indisprimary
  AND index_row.indnkeyatts >= position.index
ORDER BY index_table.relname, index_class.relname, position.index;
"#;

/// Query to fetch the check constraints of the introspected schema.
///
/// `pg_get_expr` renders the stored predicate without the `CHECK` keyword and
/// its wrapping parentheses, so the catalog carries the boolean expression the
/// constraint enforces, which is exactly what a declarative stage declares.
pub(crate) const POSTGRES_CHECKS: &str = r#"
SELECT
  check_table.relname AS table_name,
  constraint_row.conname AS constraint_name,
  pg_get_expr(constraint_row.conbin, constraint_row.conrelid) AS expression
FROM pg_constraint constraint_row
JOIN pg_class check_table ON check_table.oid = constraint_row.conrelid
JOIN pg_namespace namespace ON namespace.oid = check_table.relnamespace
WHERE constraint_row.contype = 'c'
  AND namespace.nspname = $1
  AND check_table.relname != '__rurqe_schema_history'
ORDER BY check_table.relname, constraint_row.conname;
"#;

/// Query to fetch the views of the introspected schema.
pub(crate) const POSTGRES_VIEWS: &str = r#"
SELECT view_name, view_definition
FROM information_schema.views
WHERE table_schema = $1
ORDER BY view_name;
"#;

/// Query to fetch the user-defined triggers of the introspected schema.
///
/// `tgisinternal` marks the triggers PostgreSQL creates for a constraint (a
/// foreign key check, for instance); those are already described by the
/// constraint itself, so only triggers a schema declares by hand are read.
/// `pg_get_triggerdef` renders the complete `CREATE TRIGGER` statement.
pub(crate) const POSTGRES_TRIGGERS: &str = r#"
SELECT
  trigger_row.tgname AS trigger_name,
  trigger_table.relname AS table_name,
  pg_get_triggerdef(trigger_row.oid) AS definition
FROM pg_trigger trigger_row
JOIN pg_class trigger_table ON trigger_table.oid = trigger_row.tgrelid
JOIN pg_namespace namespace ON namespace.oid = trigger_table.relnamespace
WHERE namespace.nspname = $1
  AND NOT trigger_row.tgisinternal
  AND trigger_table.relname != '__rurqe_schema_history'
ORDER BY trigger_table.relname, trigger_row.tgname;
"#;

/// Query naming the schemas that hold user relations while the introspected one
/// holds none.
///
/// The statement answers with nothing as soon as the selected schema carries a
/// table or a view of its own, and with nothing at all on a database that keeps
/// no relation anywhere else either: only a selection describing an empty catalog
/// while the database holds relations elsewhere is reported. The history table the
/// migration engine creates is never counted, so a project whose relations live in
/// another schema is still reported after a run created
/// `__rurqe_schema_history` in `public`.
pub(crate) const POSTGRES_SCHEMA_NEIGHBOURS: &str = r#"
WITH selected AS (
    SELECT count(*) AS relations
    FROM information_schema.tables
    WHERE table_schema = $1
      AND table_type IN ('BASE TABLE', 'VIEW')
      AND table_name != '__rurqe_schema_history'
)
SELECT DISTINCT table_row.table_schema
FROM information_schema.tables table_row
CROSS JOIN selected
WHERE selected.relations = 0
  AND table_row.table_type IN ('BASE TABLE', 'VIEW')
  AND table_row.table_schema != $1
  AND table_row.table_schema NOT IN ('pg_catalog', 'information_schema')
  AND table_row.table_name != '__rurqe_schema_history'
ORDER BY table_row.table_schema;
"#;

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
    fn the_index_statement_keeps_expression_indexes() {
        // An expression occupies a zero attribute number, which matches no
        // `pg_attribute` row, so only an outer join keeps the index in the catalog.
        assert!(
            POSTGRES_INDEXES.contains("LEFT JOIN pg_attribute attribute"),
            "the index query must keep an index declared over an expression"
        );
        assert!(
            !POSTGRES_INDEXES.contains("NOT (0 = ANY"),
            "the index query must not drop the indexes it cannot name by column"
        );
    }

    #[test]
    fn the_index_statement_reads_the_index_attributes() {
        assert!(POSTGRES_INDEXES.contains("access_method.amname"));
        assert!(POSTGRES_INDEXES.contains("pg_get_expr(index_row.indpred"));
        assert!(POSTGRES_INDEXES.contains("pg_get_expr(index_row.indexprs"));
    }

    #[test]
    fn the_foreign_key_statement_reads_the_referential_actions() {
        assert!(POSTGRES_FOREIGN_KEYS.contains("constraint_row.confdeltype"));
        assert!(POSTGRES_FOREIGN_KEYS.contains("constraint_row.confupdtype"));
    }

    #[test]
    fn the_check_statement_reads_only_check_constraints() {
        assert!(
            POSTGRES_CHECKS.contains("constraint_row.contype = 'c'"),
            "the check query must read the check constraints alone"
        );
        assert!(POSTGRES_CHECKS.contains("pg_get_expr"));
    }

    #[test]
    fn the_schema_neighbour_statement_ignores_a_populated_selection() {
        assert!(
            POSTGRES_SCHEMA_NEIGHBOURS.contains("selected.relations = 0"),
            "the statement only answers for a selection that holds no relation"
        );
        assert!(
            POSTGRES_SCHEMA_NEIGHBOURS.contains("table_name != '__rurqe_schema_history'"),
            "the history table never makes a selection look populated"
        );
        assert!(
            POSTGRES_SCHEMA_NEIGHBOURS
                .contains("NOT IN ('pg_catalog', 'information_schema')"),
            "a system schema is never proposed as a target"
        );
    }

    #[test]
    fn the_trigger_statement_skips_the_internal_triggers() {
        assert!(
            POSTGRES_TRIGGERS.contains("NOT trigger_row.tgisinternal"),
            "the trigger query must skip the triggers backing a constraint"
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
