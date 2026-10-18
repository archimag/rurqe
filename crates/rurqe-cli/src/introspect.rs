//! Catalog introspection queries and metadata normalization.
//!
//! Provides engine-independent metadata structures and PostgreSQL system catalog
//! queries for inspecting `information_schema` and `pg_catalog`.

use std::collections::BTreeMap;

use rurqe_migrate::catalog::{ColumnDef, ForeignKeyDef, SchemaCatalog, TableDef, TypeKind};

/// Raw table metadata row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawTableMetadata {
    pub name: String,
}

/// Raw column metadata row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawColumnMetadata {
    pub table_name: String,
    pub column_name: String,
    pub data_type: String,
    pub is_nullable: bool,
    pub column_default: Option<String>,
}

/// Raw primary key constraint row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawPrimaryKeyMetadata {
    pub table_name: String,
    pub column_name: String,
}

/// Raw foreign key constraint row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawForeignKeyMetadata {
    pub name: String,
    pub table_name: String,
    pub column_name: String,
    pub referenced_table: String,
    pub referenced_column: String,
}

/// Canonical PostgreSQL catalog introspection queries.
pub struct IntrospectQueries;

impl IntrospectQueries {
    /// Query to fetch all user tables in `public` schema.
    pub const POSTGRES_TABLES: &'static str = r#"
SELECT table_name
FROM information_schema.tables
WHERE table_schema = 'public'
  AND table_type = 'BASE TABLE'
  AND table_name != '__rurqe_schema_history'
ORDER BY table_name;
"#;

    /// Query to fetch all columns in user tables.
    pub const POSTGRES_COLUMNS: &'static str = r#"
SELECT table_name, column_name, data_type, (is_nullable = 'YES') as is_nullable, column_default
FROM information_schema.columns
WHERE table_schema = 'public'
  AND table_name != '__rurqe_schema_history'
ORDER BY table_name, ordinal_position;
"#;

    /// Query to fetch all primary key constraint columns.
    pub const POSTGRES_PRIMARY_KEYS: &'static str = r#"
SELECT kcu.table_name, kcu.column_name
FROM information_schema.table_constraints tc
JOIN information_schema.key_column_usage kcu
  ON tc.constraint_name = kcu.constraint_name
 AND tc.table_schema = kcu.table_schema
WHERE tc.constraint_type = 'PRIMARY KEY'
  AND tc.table_schema = 'public'
ORDER BY kcu.table_name, kcu.ordinal_position;
"#;

    /// Query to fetch all foreign key constraints.
    pub const POSTGRES_FOREIGN_KEYS: &'static str = r#"
SELECT
  tc.constraint_name,
  kcu.table_name,
  kcu.column_name,
  ccu.table_name AS referenced_table,
  ccu.column_name AS referenced_column
FROM information_schema.table_constraints tc
JOIN information_schema.key_column_usage kcu
  ON tc.constraint_name = kcu.constraint_name
 AND tc.table_schema = kcu.table_schema
JOIN information_schema.constraint_column_usage ccu
  ON ccu.constraint_name = tc.constraint_name
 AND ccu.table_schema = tc.table_schema
WHERE tc.constraint_type = 'FOREIGN KEY'
  AND tc.table_schema = 'public'
ORDER BY tc.constraint_name, kcu.ordinal_position;
"#;
}

/// Maps a raw SQL database data type string to an engine-neutral [`TypeKind`].
pub fn map_sql_type_to_kind(sql_type: &str) -> TypeKind {
    let lower = sql_type.trim().to_ascii_lowercase();

    match lower.as_str() {
        "boolean" | "bool" => TypeKind::Boolean,
        "integer" | "int" | "int4" | "serial" => TypeKind::Int32,
        "bigint" | "int8" | "bigserial" => TypeKind::Int64,
        "double precision" | "float8" | "real" | "float4" => TypeKind::Float64,
        "text" | "character varying" | "varchar" | "char" | "character" | "citext" => {
            TypeKind::Text
        }
        "bytea" | "blob" => TypeKind::Bytes,
        "uuid" => TypeKind::Uuid,
        "json" | "jsonb" => TypeKind::Json,
        "timestamp without time zone"
        | "timestamp with time zone"
        | "timestamp"
        | "timestamptz" => TypeKind::Timestamp,
        "date" => TypeKind::Date,
        other => TypeKind::Other(other.to_owned()),
    }
}

/// Assembles a normalized [`SchemaCatalog`] from introspected metadata collections.
pub fn catalog_from_metadata(
    tables: &[RawTableMetadata],
    columns: &[RawColumnMetadata],
    pks: &[RawPrimaryKeyMetadata],
    fks: &[RawForeignKeyMetadata],
) -> SchemaCatalog {
    let mut columns_by_table: BTreeMap<String, Vec<ColumnDef>> = BTreeMap::new();
    let mut pks_by_table: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut fks_by_table: BTreeMap<String, Vec<ForeignKeyDef>> = BTreeMap::new();

    for col in columns {
        let kind = map_sql_type_to_kind(&col.data_type);
        let mut def = ColumnDef::new(&col.column_name, kind, col.is_nullable);
        if let Some(ref def_expr) = col.column_default {
            def = def.with_default(def_expr);
        }
        columns_by_table
            .entry(col.table_name.clone())
            .or_default()
            .push(def);
    }

    for pk in pks {
        pks_by_table
            .entry(pk.table_name.clone())
            .or_default()
            .push(pk.column_name.clone());
    }

    for fk in fks {
        fks_by_table
            .entry(fk.table_name.clone())
            .or_default()
            .push(ForeignKeyDef::new(
                &fk.name,
                vec![fk.column_name.clone()],
                &fk.referenced_table,
                vec![fk.referenced_column.clone()],
            ));
    }

    let mut result_tables = Vec::new();
    for table in tables {
        let table_cols = columns_by_table.remove(&table.name).unwrap_or_default();
        let table_pk = pks_by_table.remove(&table.name).unwrap_or_default();
        let table_fks = fks_by_table.remove(&table.name).unwrap_or_default();

        let mut def = TableDef::new(&table.name, table_cols, table_pk);
        for fk in table_fks {
            def = def.with_foreign_key(fk);
        }
        result_tables.push(def);
    }

    SchemaCatalog::new(result_tables)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_assembly_builds_catalog() {
        let tables = vec![RawTableMetadata {
            name: "users".to_owned(),
        }];
        let cols = vec![
            RawColumnMetadata {
                table_name: "users".to_owned(),
                column_name: "id".to_owned(),
                data_type: "bigint".to_owned(),
                is_nullable: false,
                column_default: None,
            },
            RawColumnMetadata {
                table_name: "users".to_owned(),
                column_name: "email".to_owned(),
                data_type: "character varying".to_owned(),
                is_nullable: true,
                column_default: None,
            },
        ];
        let pks = vec![RawPrimaryKeyMetadata {
            table_name: "users".to_owned(),
            column_name: "id".to_owned(),
        }];

        let catalog = catalog_from_metadata(&tables, &cols, &pks, &[]);

        assert_eq!(catalog.tables.len(), 1);
        assert_eq!(catalog.tables[0].name, "users");
        assert_eq!(catalog.tables[0].columns.len(), 2);
        assert_eq!(catalog.tables[0].columns[0].type_kind, TypeKind::Int64);
        assert_eq!(catalog.tables[0].columns[1].type_kind, TypeKind::Text);
        assert_eq!(catalog.tables[0].primary_key, vec!["id"]);
    }
}
