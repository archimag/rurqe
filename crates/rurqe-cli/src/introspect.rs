//! Catalog introspection queries and metadata normalization.
//!
//! Provides engine-independent metadata structures and PostgreSQL system catalog
//! queries for inspecting `information_schema` and `pg_catalog`.

use std::collections::BTreeMap;
use std::fmt;

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

    /// Query to fetch user tables from the SQLite `sqlite_master` catalog.
    pub const SQLITE_TABLES: &'static str = r#"
SELECT name
FROM sqlite_master
WHERE type = 'table'
  AND name NOT LIKE 'sqlite_%'
  AND name != '__rurqe_schema_history'
ORDER BY name;
"#;

    /// Builds the `PRAGMA table_info` query for `table`.
    pub fn sqlite_table_info(table: &str) -> String {
        format!("PRAGMA table_info(\"{table}\");")
    }

    /// Builds the `PRAGMA foreign_key_list` query for `table`.
    pub fn sqlite_foreign_key_list(table: &str) -> String {
        format!("PRAGMA foreign_key_list(\"{table}\");")
    }
}

/// The database engine a connection string targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatabaseDialect {
    /// PostgreSQL, selected by `postgres://` / `postgresql://` URLs.
    Postgres,
    /// SQLite, selected by `sqlite:` URLs or local `.sqlite` / `.sqlite3` / `.db` files.
    Sqlite,
}

impl fmt::Display for DatabaseDialect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            DatabaseDialect::Postgres => "PostgreSQL",
            DatabaseDialect::Sqlite => "SQLite",
        })
    }
}

/// Detects the target [`DatabaseDialect`] from a connection string.
///
/// Absent URLs default to [`DatabaseDialect::Postgres`], preserving the
/// historical behavior for callers that rely on the default.
pub fn detect_dialect(database_url: Option<&str>) -> DatabaseDialect {
    match database_url {
        Some(url) => {
            let lower = url.trim().to_ascii_lowercase();
            if lower.starts_with("sqlite:")
                || lower.ends_with(".sqlite")
                || lower.ends_with(".sqlite3")
                || lower.ends_with(".db")
            {
                DatabaseDialect::Sqlite
            } else {
                DatabaseDialect::Postgres
            }
        }
        None => DatabaseDialect::Postgres,
    }
}

/// Raw SQLite table metadata row read from `sqlite_master`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawSqliteTableMetadata {
    pub name: String,
}

/// Raw SQLite column metadata row read from `PRAGMA table_info`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawSqliteColumnMetadata {
    pub table_name: String,
    pub column_name: String,
    pub data_type: String,
    pub not_null: bool,
    pub pk_position: i64,
    pub default_value: Option<String>,
}

/// Raw SQLite foreign key row read from `PRAGMA foreign_key_list`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawSqliteForeignKeyMetadata {
    pub table_name: String,
    pub id: i64,
    pub from_column: String,
    pub to_column: String,
    pub referenced_table: String,
}

/// Assembles a normalized [`SchemaCatalog`] from SQLite introspection results.
///
/// Columns whose `pk_position` is greater than zero form the primary key, ordered
/// by their position in the key, and each `PRAGMA foreign_key_list` row becomes a
/// single-column [`ForeignKeyDef`] named after the owning table and the constraint id.
pub fn catalog_from_sqlite_metadata(
    tables: &[RawSqliteTableMetadata],
    columns: &[RawSqliteColumnMetadata],
    foreign_keys: &[RawSqliteForeignKeyMetadata],
) -> SchemaCatalog {
    let mut columns_by_table: BTreeMap<String, Vec<ColumnDef>> = BTreeMap::new();
    let mut pk_by_table: BTreeMap<String, Vec<(i64, String)>> = BTreeMap::new();
    let mut fks_by_table: BTreeMap<String, Vec<ForeignKeyDef>> = BTreeMap::new();

    for column in columns {
        let kind = map_sql_type_to_kind(&column.data_type);
        let mut def = ColumnDef::new(&column.column_name, kind, !column.not_null);
        if let Some(ref default) = column.default_value {
            def = def.with_default(default);
        }
        columns_by_table
            .entry(column.table_name.clone())
            .or_default()
            .push(def);

        if column.pk_position > 0 {
            pk_by_table
                .entry(column.table_name.clone())
                .or_default()
                .push((column.pk_position, column.column_name.clone()));
        }
    }

    for fk in foreign_keys {
        fks_by_table
            .entry(fk.table_name.clone())
            .or_default()
            .push(ForeignKeyDef::new(
                format!("{}_{}_fkey", fk.table_name, fk.id),
                vec![fk.from_column.clone()],
                &fk.referenced_table,
                vec![fk.to_column.clone()],
            ));
    }

    let mut result_tables = Vec::new();
    for table in tables {
        let table_columns = columns_by_table.remove(&table.name).unwrap_or_default();
        let mut key_columns = pk_by_table.remove(&table.name).unwrap_or_default();
        key_columns.sort_by_key(|(position, _)| *position);
        let primary_key = key_columns.into_iter().map(|(_, name)| name).collect();
        let table_foreign_keys = fks_by_table.remove(&table.name).unwrap_or_default();

        let mut def = TableDef::new(&table.name, table_columns, primary_key);
        for fk in table_foreign_keys {
            def = def.with_foreign_key(fk);
        }
        result_tables.push(def);
    }

    SchemaCatalog::new(result_tables)
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

    #[test]
    fn sqlite_metadata_assembly_builds_catalog_with_keys() {
        let tables = vec![
            RawSqliteTableMetadata {
                name: "users".to_owned(),
            },
            RawSqliteTableMetadata {
                name: "orders".to_owned(),
            },
        ];
        let columns = vec![
            RawSqliteColumnMetadata {
                table_name: "users".to_owned(),
                column_name: "id".to_owned(),
                data_type: "INTEGER".to_owned(),
                not_null: true,
                pk_position: 1,
                default_value: None,
            },
            RawSqliteColumnMetadata {
                table_name: "users".to_owned(),
                column_name: "email".to_owned(),
                data_type: "TEXT".to_owned(),
                not_null: false,
                pk_position: 0,
                default_value: None,
            },
            RawSqliteColumnMetadata {
                table_name: "orders".to_owned(),
                column_name: "id".to_owned(),
                data_type: "INTEGER".to_owned(),
                not_null: true,
                pk_position: 1,
                default_value: None,
            },
            RawSqliteColumnMetadata {
                table_name: "orders".to_owned(),
                column_name: "user_id".to_owned(),
                data_type: "INTEGER".to_owned(),
                not_null: true,
                pk_position: 0,
                default_value: None,
            },
        ];
        let fks = vec![RawSqliteForeignKeyMetadata {
            table_name: "orders".to_owned(),
            id: 0,
            from_column: "user_id".to_owned(),
            to_column: "id".to_owned(),
            referenced_table: "users".to_owned(),
        }];

        let catalog = catalog_from_sqlite_metadata(&tables, &columns, &fks);

        assert_eq!(catalog.tables.len(), 2);
        let users = catalog.tables.iter().find(|t| t.name == "users").unwrap();
        assert_eq!(users.primary_key, vec!["id"]);
        assert_eq!(users.columns.len(), 2);
        assert_eq!(
            users
                .columns
                .iter()
                .find(|c| c.name == "email")
                .unwrap()
                .type_kind,
            TypeKind::Text
        );
        assert!(
            users
                .columns
                .iter()
                .find(|c| c.name == "email")
                .unwrap()
                .nullable
        );

        let orders = catalog.tables.iter().find(|t| t.name == "orders").unwrap();
        assert_eq!(orders.foreign_keys.len(), 1);
        assert_eq!(orders.foreign_keys[0].referenced_table, "users");
        assert_eq!(orders.foreign_keys[0].columns, vec!["user_id"]);
    }

    #[test]
    fn dialect_detection_recognises_sqlite_and_postgres() {
        assert_eq!(
            detect_dialect(Some("postgres://localhost/app")),
            DatabaseDialect::Postgres
        );
        assert_eq!(
            detect_dialect(Some("sqlite://local.db")),
            DatabaseDialect::Sqlite
        );
        assert_eq!(
            detect_dialect(Some("app.sqlite3")),
            DatabaseDialect::Sqlite
        );
        assert_eq!(
            detect_dialect(Some("data/app.db")),
            DatabaseDialect::Sqlite
        );
        assert_eq!(detect_dialect(None), DatabaseDialect::Postgres);
        assert_eq!(DatabaseDialect::Sqlite.to_string(), "SQLite");
    }
}
