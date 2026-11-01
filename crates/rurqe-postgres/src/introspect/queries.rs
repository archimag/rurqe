//! The catalog statements PostgreSQL introspection runs.
//!
//! Every statement binds the introspected schema list as its first parameter and
//! filters the catalog through `ANY`, so the same catalog text serves the
//! user-facing schemas and the disposable scratch schema a stage verification is
//! verified in. The normative doc comments of each statement stay next to the SQL
//! it describes, so the reasoning behind a query is never separated from the text
//! it justifies.

/// Query to fetch all user tables of the introspected schemas.
pub const POSTGRES_TABLES: &str = r#"
SELECT table_schema, table_name
FROM information_schema.tables
WHERE table_schema::text = ANY($1)
  AND table_type = 'BASE TABLE'
  AND table_name != '__rurqe_schema_history'
ORDER BY table_schema, table_name;
"#;

/// Query to fetch all columns of the introspected schemas.
///
/// A domain is a type of its own, while a column declared over it stores the
/// values of the type the domain wraps. `information_schema.columns` names the
/// domain instead of that base type, so the declared type is resolved through
/// `pg_type.typbasetype` and the base type is reported in its place: the catalog
/// then carries the type the column really holds rather than a name no
/// introspection can classify. A column of a plain type is reported as it is,
/// and a domain declared over another domain is followed to its end.
///
/// The schema of the type follows the same resolution: a domain declared over a
/// type of another schema reports the schema of the type the column really
/// holds, so a column is attributed to the schema of the type it stores.
pub const POSTGRES_COLUMNS: &str = r#"
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
  column_row.table_schema,
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
  END AS udt_name,
  CASE
    WHEN resolved_type.resolved IS NULL OR resolved_type.resolved = declared_type.oid
      THEN column_row.udt_schema
    ELSE base_type_namespace.nspname
  END AS udt_schema
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
LEFT JOIN pg_namespace base_type_namespace
  ON base_type_namespace.oid = base_type.typnamespace
WHERE column_row.table_schema::text = ANY($1)
  AND column_row.table_name != '__rurqe_schema_history'
ORDER BY column_row.table_schema, column_row.table_name, column_row.ordinal_position;
"#;

/// Query to fetch the user enum types of the introspected schemas and their
/// variants in declaration order.
pub const POSTGRES_ENUMS: &str = r#"
SELECT
  n.nspname AS schema_name,
  t.typname AS enum_name,
  e.enumlabel AS variant_name
FROM pg_type t
JOIN pg_enum e ON t.oid = e.enumtypid
JOIN pg_namespace n ON n.oid = t.typnamespace
WHERE n.nspname::text = ANY($1)
ORDER BY n.nspname, t.typname, e.enumsortorder;
"#;

/// Query to fetch all primary key constraint columns.
pub const POSTGRES_PRIMARY_KEYS: &str = r#"
SELECT kcu.table_schema, kcu.table_name, kcu.column_name
FROM information_schema.table_constraints tc
JOIN information_schema.key_column_usage kcu
  ON tc.constraint_name = kcu.constraint_name
 AND tc.table_schema = kcu.table_schema
 AND tc.table_name = kcu.table_name
WHERE tc.constraint_type = 'PRIMARY KEY'
  AND tc.table_schema::text = ANY($1)
ORDER BY kcu.table_schema, kcu.table_name, kcu.ordinal_position;
"#;

/// Query to fetch the foreign key columns of the introspected schemas in key order.
///
/// `information_schema.constraint_column_usage` loses the positional
/// relationship between referencing and referenced columns, so the catalog is
/// read through `pg_constraint` and the `conkey` / `confkey` arrays are
/// unnested together with `generate_subscripts` to keep composite keys aligned.
///
/// `confdeltype` and `confupdtype` carry the referential actions of the key as
/// single letters, which are spelled out here so the catalog receives the action
/// itself rather than a driver-level character code.
///
/// The schema of the referenced table is reported for every reference, and the
/// decoder reduces the primary schema to no attribution. A key held inside the
/// primary schema would otherwise name that schema on every constraint, and the
/// scratch schema a stage verification builds would record a name the live schema
/// never carries, so the golden equation could not compare the two catalogs. A
/// reference that crosses into another schema keeps the name of the schema it
/// points into, which is the part of the reference the constraint really
/// declares.
pub const POSTGRES_FOREIGN_KEYS: &str = r#"
SELECT
  namespace.nspname AS table_schema,
  constraint_row.conname,
  owning.relname AS table_name,
  owning_column.attname AS column_name,
  referenced.relname AS referenced_table,
  referenced_column.attname AS referenced_column,
  referenced_namespace.nspname AS referenced_schema,
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
JOIN pg_namespace referenced_namespace
  ON referenced_namespace.oid = referenced.relnamespace
JOIN generate_subscripts(constraint_row.conkey, 1) AS position(index) ON TRUE
JOIN pg_attribute owning_column
  ON owning_column.attrelid = constraint_row.conrelid
 AND owning_column.attnum = constraint_row.conkey[position.index]
JOIN pg_attribute referenced_column
  ON referenced_column.attrelid = constraint_row.confrelid
 AND referenced_column.attnum = constraint_row.confkey[position.index]
WHERE constraint_row.contype = 'f'
  AND namespace.nspname::text = ANY($1)
ORDER BY namespace.nspname, owning.relname, constraint_row.conname, position.index;
"#;

/// Query to fetch the secondary indexes of the introspected schemas.
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
pub const POSTGRES_INDEXES: &str = r#"
SELECT
  namespace.nspname AS table_schema,
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
WHERE namespace.nspname::text = ANY($1)
  AND NOT index_row.indisprimary
  AND index_row.indnkeyatts >= position.index
ORDER BY namespace.nspname, index_table.relname, index_class.relname, position.index;
"#;

/// Query to fetch the check constraints of the introspected schemas.
///
/// `pg_get_expr` renders the stored predicate without the `CHECK` keyword and
/// its wrapping parentheses, so the catalog carries the boolean expression the
/// constraint enforces, which is exactly what a declarative stage declares.
pub const POSTGRES_CHECKS: &str = r#"
SELECT
  namespace.nspname AS table_schema,
  check_table.relname AS table_name,
  constraint_row.conname AS constraint_name,
  pg_get_expr(constraint_row.conbin, constraint_row.conrelid) AS expression
FROM pg_constraint constraint_row
JOIN pg_class check_table ON check_table.oid = constraint_row.conrelid
JOIN pg_namespace namespace ON namespace.oid = check_table.relnamespace
WHERE constraint_row.contype = 'c'
  AND namespace.nspname::text = ANY($1)
  AND check_table.relname != '__rurqe_schema_history'
ORDER BY namespace.nspname, check_table.relname, constraint_row.conname;
"#;

/// Query to fetch the views of the introspected schemas.
pub const POSTGRES_VIEWS: &str = r#"
SELECT table_schema, view_name, view_definition
FROM information_schema.views
WHERE table_schema::text = ANY($1)
ORDER BY table_schema, view_name;
"#;

/// Query to fetch the user-defined triggers of the introspected schemas.
///
/// `tgisinternal` marks the triggers PostgreSQL creates for a constraint (a
/// foreign key check, for instance); those are already described by the
/// constraint itself, so only triggers a schema declares by hand are read.
/// `pg_get_triggerdef` renders the complete `CREATE TRIGGER` statement.
pub const POSTGRES_TRIGGERS: &str = r#"
SELECT
  namespace.nspname AS table_schema,
  trigger_row.tgname AS trigger_name,
  trigger_table.relname AS table_name,
  pg_get_triggerdef(trigger_row.oid) AS definition
FROM pg_trigger trigger_row
JOIN pg_class trigger_table ON trigger_table.oid = trigger_row.tgrelid
JOIN pg_namespace namespace ON namespace.oid = trigger_table.relnamespace
WHERE namespace.nspname::text = ANY($1)
  AND NOT trigger_row.tgisinternal
  AND trigger_table.relname != '__rurqe_schema_history'
ORDER BY namespace.nspname, trigger_table.relname, trigger_row.tgname;
"#;

/// Query naming the schemas that hold user relations while none of the
/// introspected ones does.
///
/// The statement answers with nothing as soon as one selected schema carries a
/// table or a view of its own, and with nothing at all on a database that keeps
/// no relation anywhere else either: only a selection describing an empty catalog
/// while the database holds relations elsewhere is reported. The history table the
/// migration engine creates is never counted, and a selected schema is never
/// proposed as its own neighbour.
pub const POSTGRES_SCHEMA_NEIGHBOURS: &str = r#"
WITH selected AS (
    SELECT count(*) AS relations
    FROM information_schema.tables
    WHERE table_schema::text = ANY($1)
      AND table_type IN ('BASE TABLE', 'VIEW')
      AND table_name != '__rurqe_schema_history'
)
SELECT DISTINCT table_row.table_schema
FROM information_schema.tables table_row
CROSS JOIN selected
WHERE selected.relations = 0
  AND table_row.table_type IN ('BASE TABLE', 'VIEW')
  AND table_row.table_schema::text <> ALL($1)
  AND table_row.table_schema NOT IN ('pg_catalog', 'information_schema')
  AND table_row.table_name != '__rurqe_schema_history'
ORDER BY table_row.table_schema;
"#;
