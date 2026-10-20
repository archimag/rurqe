//! SQLite catalog introspection through `PRAGMA` queries and `sqlite_master`.

use std::collections::BTreeMap;

use rurqe_migrate::catalog::{ColumnDef, ForeignKeyDef, SchemaCatalog, TableDef};

use crate::cli::CliError;

use super::types::map_sql_type_to_kind;

/// Query to fetch user tables from the SQLite `sqlite_master` catalog.
pub(crate) const SQLITE_TABLES: &str = r#"
SELECT name
FROM sqlite_master
WHERE type = 'table'
  AND name NOT LIKE 'sqlite_%'
  AND name != '__rurqe_schema_history'
ORDER BY name;
"#;

/// Builds the `PRAGMA table_info` query for `table`.
///
/// The table name is wrapped in double quotes and every embedded `"` is
/// doubled, so legal-but-unusual table names neither break the statement nor
/// smuggle additional SQL into it.
pub(crate) fn sqlite_table_info(table: &str) -> String {
    format!(
        "PRAGMA table_info(\"{}\");",
        escape_sqlite_identifier(table)
    )
}

/// Builds the `PRAGMA foreign_key_list` query for `table`.
pub(crate) fn sqlite_foreign_key_list(table: &str) -> String {
    format!(
        "PRAGMA foreign_key_list(\"{}\");",
        escape_sqlite_identifier(table)
    )
}

/// Escapes a SQLite identifier for use inside a double-quoted name literal.
fn escape_sqlite_identifier(identifier: &str) -> String {
    identifier.replace('"', "\"\"")
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
///
/// `PRAGMA foreign_key_list` reports a `NULL` target column for a key that
/// references the implicit primary key of its parent table; such a target is
/// resolved against the parent key columns by position instead of being kept as
/// an empty name.
pub fn catalog_from_sqlite_metadata(
    tables: &[RawSqliteTableMetadata],
    columns: &[RawSqliteColumnMetadata],
    foreign_keys: &[RawSqliteForeignKeyMetadata],
) -> SchemaCatalog {
    let mut columns_by_table: BTreeMap<String, Vec<ColumnDef>> = BTreeMap::new();
    let mut fks_by_table: BTreeMap<String, BTreeMap<i64, ForeignKeyDef>> = BTreeMap::new();

    // Key columns in key order: they name both the assembled primary keys and
    // the target of a foreign key that references an implicit parent key.
    let mut primary_keys: BTreeMap<&str, Vec<(i64, &str)>> = BTreeMap::new();

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
            primary_keys
                .entry(column.table_name.as_str())
                .or_default()
                .push((column.pk_position, column.column_name.as_str()));
        }
    }

    for key in primary_keys.values_mut() {
        key.sort_by_key(|(position, _)| *position);
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

        // `PRAGMA foreign_key_list` reports a NULL target column for a key that
        // references the implicit primary key of the parent table, so the name
        // is recovered from the introspected key columns by position.
        let referenced_column = if fk.to_column.is_empty() {
            implicit_referenced_column(
                &primary_keys,
                &fk.referenced_table,
                constraint.columns.len(),
            )
            .unwrap_or_default()
        } else {
            fk.to_column.clone()
        };
        constraint.columns.push(fk.from_column.clone());
        constraint.referenced_columns.push(referenced_column);
    }

    let mut result_tables = Vec::new();
    for table in tables {
        let table_columns = columns_by_table.remove(&table.name).unwrap_or_default();
        let primary_key: Vec<String> = primary_keys
            .get(table.name.as_str())
            .map(|key| key.iter().map(|(_, name)| (*name).to_owned()).collect())
            .unwrap_or_default();
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

/// Names the column a foreign key references when SQLite omits it.
///
/// A key that targets the implicit primary key of its parent table is reported
/// without a target column, and `ordinal` selects the matching key column.
fn implicit_referenced_column(
    primary_keys: &BTreeMap<&str, Vec<(i64, &str)>>,
    referenced_table: &str,
    ordinal: usize,
) -> Option<String> {
    primary_keys
        .get(referenced_table)
        .and_then(|key| key.get(ordinal))
        .map(|(_, name)| (*name).to_owned())
}

/// Introspects a live SQLite database connection and returns the normalized [`SchemaCatalog`].
pub fn introspect_sqlite(conn: &rusqlite::Connection) -> Result<SchemaCatalog, CliError> {
    let mut stmt = conn
        .prepare(SQLITE_TABLES)
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
        let col_sql = sqlite_table_info(table);
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

        let fk_sql = sqlite_foreign_key_list(table);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn column(table: &str, name: &str, pk_position: i64) -> RawSqliteColumnMetadata {
        RawSqliteColumnMetadata {
            table_name: table.to_owned(),
            column_name: name.to_owned(),
            data_type: "INTEGER".to_owned(),
            not_null: true,
            pk_position,
            default_value: None,
        }
    }

    fn key(id: i64, from: &str, to: &str) -> RawSqliteForeignKeyMetadata {
        RawSqliteForeignKeyMetadata {
            table_name: "order_lines".to_owned(),
            id,
            from_column: from.to_owned(),
            to_column: to.to_owned(),
            referenced_table: "orders".to_owned(),
        }
    }

    #[test]
    fn implicit_key_targets_follow_the_parent_key_order() {
        let tables = vec![
            RawSqliteTableMetadata {
                name: "orders".to_owned(),
            },
            RawSqliteTableMetadata {
                name: "order_lines".to_owned(),
            },
        ];
        // The parent key columns arrive out of order, so the resolution has to
        // follow the declared key positions instead of the introspection order.
        let columns = vec![
            column("orders", "region", 2),
            column("orders", "id", 1),
            column("order_lines", "order_id", 0),
            column("order_lines", "order_region", 0),
        ];
        let foreign_keys = vec![key(0, "order_id", ""), key(0, "order_region", "")];

        let catalog = catalog_from_sqlite_metadata(&tables, &columns, &foreign_keys);

        let lines = catalog
            .tables
            .iter()
            .find(|table| table.name == "order_lines")
            .expect("the child table is assembled");
        assert_eq!(
            lines.foreign_keys[0].referenced_columns,
            vec!["id", "region"]
        );

        let orders = catalog
            .tables
            .iter()
            .find(|table| table.name == "orders")
            .expect("the parent table is assembled");
        assert_eq!(orders.primary_key, vec!["id", "region"]);
    }

    #[test]
    fn explicit_key_targets_are_preserved() {
        let tables = vec![
            RawSqliteTableMetadata {
                name: "orders".to_owned(),
            },
            RawSqliteTableMetadata {
                name: "order_lines".to_owned(),
            },
        ];
        let columns = vec![
            column("orders", "id", 1),
            column("orders", "region", 2),
            column("order_lines", "order_region", 0),
        ];
        let foreign_keys = vec![key(0, "order_region", "region")];

        let catalog = catalog_from_sqlite_metadata(&tables, &columns, &foreign_keys);

        let lines = catalog
            .tables
            .iter()
            .find(|table| table.name == "order_lines")
            .expect("the child table is assembled");
        assert_eq!(lines.foreign_keys[0].referenced_columns, vec!["region"]);
    }
}
