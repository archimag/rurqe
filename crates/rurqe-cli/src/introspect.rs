//! Catalog introspection queries and metadata normalization.
//!
//! Provides engine-independent metadata structures and PostgreSQL system catalog
//! queries for inspecting `information_schema` and `pg_catalog`.

use std::collections::BTreeMap;
use std::fmt;

use rurqe_migrate::catalog::{ColumnDef, ForeignKeyDef, SchemaCatalog, TableDef, TypeKind};

use crate::cli::CliError;

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

    /// Query to fetch all foreign key columns in key order.
    ///
    /// `information_schema.constraint_column_usage` loses the positional
    /// relationship between referencing and referenced columns, so the catalog is
    /// read through `pg_constraint` and the `conkey` / `confkey` arrays are
    /// unnested together with `generate_subscripts` to keep composite keys aligned.
    pub const POSTGRES_FOREIGN_KEYS: &'static str = r#"
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
  AND namespace.nspname = 'public'
ORDER BY owning.relname, constraint_row.conname, position.index;
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
                || lower == ":memory:"
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

/// Introspects a live SQLite database connection and returns the normalized [`SchemaCatalog`].
pub fn introspect_sqlite(conn: &rusqlite::Connection) -> Result<SchemaCatalog, CliError> {
    let mut stmt = conn
        .prepare(IntrospectQueries::SQLITE_TABLES)
        .map_err(|e| CliError(format!("failed to prepare SQLite table query: {e}")))?;
    let table_names = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|e| CliError(format!("failed to query SQLite tables: {e}")))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| CliError(format!("failed to read SQLite table row: {e}")))?;

    let raw_tables: Vec<RawSqliteTableMetadata> = table_names
        .iter()
        .map(|name| RawSqliteTableMetadata { name: name.clone() })
        .collect();

    let mut raw_columns = Vec::new();
    let mut raw_foreign_keys = Vec::new();

    for table in &table_names {
        let col_sql = IntrospectQueries::sqlite_table_info(table);
        let mut col_stmt = conn
            .prepare(&col_sql)
            .map_err(|e| CliError(format!("failed to prepare table_info for `{table}`: {e}")))?;

        let cols = col_stmt
            .query_map([], |row| {
                let notnull: i64 = row.get(3)?;
                let pk: i64 = row.get(5)?;
                Ok(RawSqliteColumnMetadata {
                    table_name: table.clone(),
                    column_name: row.get(1)?,
                    data_type: row.get(2)?,
                    not_null: notnull != 0,
                    pk_position: pk,
                    default_value: row.get(4)?,
                })
            })
            .map_err(|e| CliError(format!("failed to inspect columns for `{table}`: {e}")))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| CliError(format!("failed to read column for `{table}`: {e}")))?;

        raw_columns.extend(cols);

        let fk_sql = IntrospectQueries::sqlite_foreign_key_list(table);
        let mut fk_stmt = conn.prepare(&fk_sql).map_err(|e| {
            CliError(format!(
                "failed to prepare foreign_key_list for `{table}`: {e}"
            ))
        })?;

        let fks = fk_stmt
            .query_map([], |row| {
                let id: i64 = row.get(0)?;
                let referenced_table: String = row.get(2)?;
                let from_column: String = row.get(3)?;
                let to_column: Option<String> = row.get(4)?;
                Ok(RawSqliteForeignKeyMetadata {
                    table_name: table.clone(),
                    id,
                    from_column,
                    to_column: to_column.unwrap_or_default(),
                    referenced_table,
                })
            })
            .map_err(|e| CliError(format!("failed to inspect foreign keys for `{table}`: {e}")))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| CliError(format!("failed to read foreign key for `{table}`: {e}")))?;

        raw_foreign_keys.extend(fks);
    }

    Ok(catalog_from_sqlite_metadata(
        &raw_tables,
        &raw_columns,
        &raw_foreign_keys,
    ))
}

/// Introspects a live PostgreSQL database client and returns the normalized [`SchemaCatalog`].
pub fn introspect_postgres(client: &mut postgres::Client) -> Result<SchemaCatalog, CliError> {
    let table_rows = client
        .query(IntrospectQueries::POSTGRES_TABLES, &[])
        .map_err(|e| CliError(format!("failed to query PostgreSQL tables: {e}")))?;
    let raw_tables: Vec<RawTableMetadata> = table_rows
        .into_iter()
        .map(|row| RawTableMetadata { name: row.get(0) })
        .collect();

    let col_rows = client
        .query(IntrospectQueries::POSTGRES_COLUMNS, &[])
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
        .query(IntrospectQueries::POSTGRES_PRIMARY_KEYS, &[])
        .map_err(|e| CliError(format!("failed to query PostgreSQL primary keys: {e}")))?;
    let raw_pks: Vec<RawPrimaryKeyMetadata> = pk_rows
        .into_iter()
        .map(|row| RawPrimaryKeyMetadata {
            table_name: row.get(0),
            column_name: row.get(1),
        })
        .collect();

    let fk_rows = client
        .query(IntrospectQueries::POSTGRES_FOREIGN_KEYS, &[])
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

/// Introspects the schema catalog of a live database identified by its connection URL or path.
pub fn introspect_database(url: &str) -> Result<SchemaCatalog, CliError> {
    match detect_dialect(Some(url)) {
        DatabaseDialect::Sqlite => {
            let path = if let Some(stripped) = url.strip_prefix("sqlite://") {
                stripped
            } else if let Some(stripped) = url.strip_prefix("sqlite:") {
                stripped
            } else {
                url
            };

            let conn = if path == ":memory:" || path.is_empty() {
                rusqlite::Connection::open_in_memory()
            } else {
                rusqlite::Connection::open(path)
            }
            .map_err(|e| CliError(format!("failed to open SQLite database at `{url}`: {e}")))?;

            introspect_sqlite(&conn)
        }
        DatabaseDialect::Postgres => {
            let mut client = postgres::Client::connect(url, postgres::NoTls).map_err(|e| {
                CliError(format!("failed to connect to PostgreSQL at `{url}`: {e}"))
            })?;
            introspect_postgres(&mut client)
        }
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
/// by their position in the key. `PRAGMA foreign_key_list` rows sharing a constraint
/// id are folded into one [`ForeignKeyDef`] named after the owning table and that id,
/// so composite foreign keys stay a single constraint.
pub fn catalog_from_sqlite_metadata(
    tables: &[RawSqliteTableMetadata],
    columns: &[RawSqliteColumnMetadata],
    foreign_keys: &[RawSqliteForeignKeyMetadata],
) -> SchemaCatalog {
    let mut columns_by_table: BTreeMap<String, Vec<ColumnDef>> = BTreeMap::new();
    let mut pk_by_table: BTreeMap<String, Vec<(i64, String)>> = BTreeMap::new();
    let mut fks_by_table: BTreeMap<String, BTreeMap<i64, ForeignKeyDef>> = BTreeMap::new();

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
        let constraint = fks_by_table
            .entry(fk.table_name.clone())
            .or_default()
            .entry(fk.id)
            .or_insert_with(|| {
                ForeignKeyDef::new(
                    format!("{}_{}_fkey", fk.table_name, fk.id),
                    Vec::new(),
                    &fk.referenced_table,
                    Vec::new(),
                )
            });
        constraint.columns.push(fk.from_column.clone());
        constraint.referenced_columns.push(fk.to_column.clone());
    }

    let mut result_tables = Vec::new();
    for table in tables {
        let table_columns = columns_by_table.remove(&table.name).unwrap_or_default();
        let mut key_columns = pk_by_table.remove(&table.name).unwrap_or_default();
        key_columns.sort_by_key(|(position, _)| *position);
        let primary_key = key_columns.into_iter().map(|(_, name)| name).collect();
        let table_foreign_keys: Vec<ForeignKeyDef> = fks_by_table
            .remove(&table.name)
            .map(|constraints| constraints.into_values().collect())
            .unwrap_or_default();

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
    fn composite_foreign_key_rows_fold_into_one_constraint() {
        let tables = vec![RawTableMetadata {
            name: "order_items".to_owned(),
        }];
        let cols = vec![
            RawColumnMetadata {
                table_name: "order_items".to_owned(),
                column_name: "order_id".to_owned(),
                data_type: "bigint".to_owned(),
                is_nullable: false,
                column_default: None,
            },
            RawColumnMetadata {
                table_name: "order_items".to_owned(),
                column_name: "line_no".to_owned(),
                data_type: "bigint".to_owned(),
                is_nullable: false,
                column_default: None,
            },
        ];
        let fks = vec![
            RawForeignKeyMetadata {
                name: "order_items_order_fkey".to_owned(),
                table_name: "order_items".to_owned(),
                column_name: "order_id".to_owned(),
                referenced_table: "orders".to_owned(),
                referenced_column: "id".to_owned(),
            },
            RawForeignKeyMetadata {
                name: "order_items_order_fkey".to_owned(),
                table_name: "order_items".to_owned(),
                column_name: "line_no".to_owned(),
                referenced_table: "orders".to_owned(),
                referenced_column: "line_no".to_owned(),
            },
        ];

        let catalog = catalog_from_metadata(&tables, &cols, &[], &fks);

        let table = &catalog.tables[0];
        assert_eq!(table.foreign_keys.len(), 1);
        assert_eq!(table.foreign_keys[0].columns, vec!["order_id", "line_no"]);
        assert_eq!(
            table.foreign_keys[0].referenced_columns,
            vec!["id", "line_no"]
        );
    }

    #[test]
    fn sqlite_composite_foreign_key_rows_fold_into_one_constraint() {
        let tables = vec![RawSqliteTableMetadata {
            name: "order_items".to_owned(),
        }];
        let columns = vec![
            RawSqliteColumnMetadata {
                table_name: "order_items".to_owned(),
                column_name: "order_id".to_owned(),
                data_type: "INTEGER".to_owned(),
                not_null: true,
                pk_position: 1,
                default_value: None,
            },
            RawSqliteColumnMetadata {
                table_name: "order_items".to_owned(),
                column_name: "line_no".to_owned(),
                data_type: "INTEGER".to_owned(),
                not_null: true,
                pk_position: 2,
                default_value: None,
            },
        ];
        let fks = vec![
            RawSqliteForeignKeyMetadata {
                table_name: "order_items".to_owned(),
                id: 0,
                from_column: "order_id".to_owned(),
                to_column: "id".to_owned(),
                referenced_table: "orders".to_owned(),
            },
            RawSqliteForeignKeyMetadata {
                table_name: "order_items".to_owned(),
                id: 0,
                from_column: "line_no".to_owned(),
                to_column: "line_no".to_owned(),
                referenced_table: "orders".to_owned(),
            },
        ];

        let catalog = catalog_from_sqlite_metadata(&tables, &columns, &fks);

        let table = &catalog.tables[0];
        assert_eq!(table.foreign_keys.len(), 1);
        assert_eq!(table.primary_key, vec!["order_id", "line_no"]);
        assert_eq!(table.foreign_keys[0].columns, vec!["order_id", "line_no"]);
        assert_eq!(
            table.foreign_keys[0].referenced_columns,
            vec!["id", "line_no"]
        );
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
        let email = users.columns.iter().find(|c| c.name == "email").unwrap();
        assert_eq!(email.type_kind, TypeKind::Text);
        assert!(email.nullable);

        let orders = catalog.tables.iter().find(|t| t.name == "orders").unwrap();
        assert_eq!(orders.foreign_keys.len(), 1);
        assert_eq!(orders.foreign_keys[0].referenced_table, "users");
        assert_eq!(orders.foreign_keys[0].columns, vec!["user_id"]);
    }

    #[test]
    fn sql_type_mapping_covers_builtin_kinds() {
        assert_eq!(map_sql_type_to_kind("BOOLEAN"), TypeKind::Boolean);
        assert_eq!(map_sql_type_to_kind("INTEGER"), TypeKind::Int32);
        assert_eq!(map_sql_type_to_kind("bigint"), TypeKind::Int64);
        assert_eq!(map_sql_type_to_kind("double precision"), TypeKind::Float64);
        assert_eq!(
            map_sql_type_to_kind("timestamp with time zone"),
            TypeKind::Timestamp
        );
        assert_eq!(map_sql_type_to_kind("uuid"), TypeKind::Uuid);
        assert_eq!(map_sql_type_to_kind("jsonb"), TypeKind::Json);
        assert_eq!(
            map_sql_type_to_kind("inet"),
            TypeKind::Other("inet".to_owned())
        );
    }

    #[test]
    fn introspect_sqlite_builds_catalog_from_live_tables() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE users (
                id INTEGER PRIMARY KEY,
                email TEXT NOT NULL,
                active INTEGER NOT NULL DEFAULT 1
            );
            CREATE TABLE posts (
                id INTEGER PRIMARY KEY,
                author_id INTEGER NOT NULL,
                title TEXT NOT NULL,
                FOREIGN KEY (author_id) REFERENCES users(id)
            );
            CREATE TABLE __rurqe_schema_history (
                installed_rank INTEGER PRIMARY KEY,
                version TEXT
            );
            "#,
        )
        .unwrap();

        let catalog = introspect_sqlite(&conn).unwrap();
        assert_eq!(catalog.tables.len(), 2);

        let users = catalog.tables.iter().find(|t| t.name == "users").unwrap();
        assert_eq!(users.primary_key, vec!["id"]);
        assert_eq!(users.columns.len(), 3);
        let email = users.columns.iter().find(|c| c.name == "email").unwrap();
        assert_eq!(email.type_kind, TypeKind::Text);
        assert!(!email.nullable);

        let posts = catalog.tables.iter().find(|t| t.name == "posts").unwrap();
        assert_eq!(posts.foreign_keys.len(), 1);
        assert_eq!(posts.foreign_keys[0].referenced_table, "users");
        assert_eq!(posts.foreign_keys[0].columns, vec!["author_id"]);
        assert_eq!(posts.foreign_keys[0].referenced_columns, vec!["id"]);
    }

    #[test]
    fn introspect_database_handles_sqlite_memory() {
        let catalog = introspect_database(":memory:").unwrap();
        assert!(catalog.tables.is_empty());

        let catalog = introspect_database("sqlite::memory:").unwrap();
        assert!(catalog.tables.is_empty());
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
        assert_eq!(detect_dialect(Some("app.sqlite3")), DatabaseDialect::Sqlite);
        assert_eq!(detect_dialect(Some("data/app.db")), DatabaseDialect::Sqlite);
        assert_eq!(detect_dialect(None), DatabaseDialect::Postgres);
        assert_eq!(DatabaseDialect::Sqlite.to_string(), "SQLite");
    }
}
