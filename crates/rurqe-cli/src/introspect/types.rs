//! Engine-independent raw metadata rows, SQL type mapping, and catalog assembly.

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
///
/// Foreign key rows sharing a constraint name on the same table are folded into a
/// single [`ForeignKeyDef`], preserving the order of the columns inside the key.
pub fn catalog_from_metadata(
    tables: &[RawTableMetadata],
    columns: &[RawColumnMetadata],
    pks: &[RawPrimaryKeyMetadata],
    fks: &[RawForeignKeyMetadata],
) -> SchemaCatalog {
    let mut columns_by_table: BTreeMap<String, Vec<ColumnDef>> = BTreeMap::new();
    let mut pks_by_table: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut fks_by_table: BTreeMap<String, BTreeMap<String, ForeignKeyDef>> = BTreeMap::new();

    for col in columns {
        let kind = map_sql_type_to_kind(&col.data_type);
        let mut def = ColumnDef::new(&col.column_name, kind, col.is_nullable);
        if let Some(ref default) = col.column_default {
            def = def.with_default(default);
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
        let constraint = fks_by_table
            .entry(fk.table_name.clone())
            .or_default()
            .entry(fk.name.clone())
            .or_insert_with(|| {
                ForeignKeyDef::new(&fk.name, Vec::new(), &fk.referenced_table, Vec::new())
            });
        constraint.columns.push(fk.column_name.clone());
        constraint.referenced_columns.push(fk.referenced_column.clone());
    }

    let mut result_tables = Vec::new();
    for table in tables {
        let table_cols = columns_by_table.remove(&table.name).unwrap_or_default();
        let table_pk = pks_by_table.remove(&table.name).unwrap_or_default();
        let table_fks: Vec<ForeignKeyDef> = fks_by_table
            .remove(&table.name)
            .map(|constraints| constraints.into_values().collect())
            .unwrap_or_default();

        let mut def = TableDef::new(&table.name, table_cols, table_pk);
        for fk in table_fks {
            def = def.with_foreign_key(fk);
        }
        result_tables.push(def);
    }

    SchemaCatalog::new(result_tables)
}
