//! SQLite catalog introspection through `PRAGMA` queries and `sqlite_master`.

use std::collections::{BTreeMap, BTreeSet};

use rurqe_migrate::catalog::{ColumnDef, EnumDef, ForeignKeyDef, SchemaCatalog, TableDef, TypeKind};

use crate::cli::CliError;

use super::types::{fold_indexes, map_sqlite_declared_type_to_kind, RawIndexMetadata};

/// Query to fetch user tables and their DDL from the SQLite `sqlite_master` catalog.
pub(crate) const SQLITE_TABLES: &str = r#"
SELECT name, sql
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

/// Builds the `PRAGMA index_list` query for `table`.
pub(crate) fn sqlite_index_list(table: &str) -> String {
    format!("PRAGMA index_list(\"{}\");", escape_sqlite_identifier(table))
}

/// Builds the `PRAGMA index_info` query for `index`.
pub(crate) fn sqlite_index_info(index: &str) -> String {
    format!(
        "PRAGMA index_info(\"{}\");",
        escape_sqlite_identifier(index)
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
    pub sql: Option<String>,
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
/// Column types follow the SQLite affinity rules of
/// [`map_sqlite_declared_type_to_kind`], so every integer column is reported as
/// an [`Int64`](rurqe_migrate::catalog::TypeKind::Int64) no matter how its
/// declared type is spelled.
///
/// Columns whose `pk_position` is greater than zero form the primary key, ordered
/// by their position in the key. `PRAGMA foreign_key_list` rows sharing a
/// constraint id are folded into one [`ForeignKeyDef`] named after the owning
/// table and that id, so composite foreign keys stay a single constraint.
///
/// Index rows are folded into one [`IndexDef`](rurqe_migrate::catalog::IndexDef)
/// per index, keeping the column order of the index declaration.
///
/// `PRAGMA foreign_key_list` reports a `NULL` target column for a key that
/// references the implicit primary key of its parent table; such a target is
/// resolved against the parent key columns by position instead of being kept as
/// an empty name.
pub fn catalog_from_sqlite_metadata(
    tables: &[RawSqliteTableMetadata],
    columns: &[RawSqliteColumnMetadata],
    foreign_keys: &[RawSqliteForeignKeyMetadata],
    indexes: &[RawIndexMetadata],
) -> SchemaCatalog {
    let mut enums_by_name: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut check_enums: BTreeMap<(String, String), String> = BTreeMap::new();
    for table in tables {
        let table_col_names: Vec<&str> = columns
            .iter()
            .filter(|c| c.table_name == table.name)
            .map(|c| c.column_name.as_str())
            .collect();
        if let Some(ref sql) = table.sql {
            for (col_name, variants) in parse_sqlite_check_enums(sql, &table_col_names) {
                let candidate = col_name.clone();
                let enum_name = match enums_by_name.get(&candidate) {
                    Some(existing_variants) if existing_variants == &variants => candidate,
                    None => candidate,
                    Some(_) => format!("{}_{}_enum", table.name, col_name),
                };
                enums_by_name
                    .entry(enum_name.clone())
                    .or_insert_with(|| variants);
                check_enums.insert(
                    (table.name.clone(), col_name),
                    enum_name,
                );
            }
        }
    }

    let mut columns_by_table: BTreeMap<String, Vec<ColumnDef>> = BTreeMap::new();
    let mut fks_by_table: BTreeMap<String, BTreeMap<i64, ForeignKeyDef>> = BTreeMap::new();
    let mut indexes_by_table = fold_indexes(indexes);

    // Key columns in key order: they name both the assembled primary keys and
    // the target of a foreign key that references an implicit parent key.
    let mut primary_keys: BTreeMap<&str, Vec<(i64, &str)>> = BTreeMap::new();

    for column in columns {
        let kind = if let Some(enum_name) =
            check_enums.get(&(column.table_name.clone(), column.column_name.clone()))
        {
            TypeKind::Enum(enum_name.clone())
        } else {
            map_sqlite_declared_type_to_kind(&column.data_type)
        };
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
        let table_indexes = indexes_by_table.remove(&table.name).unwrap_or_default();

        let mut def = TableDef::new(&table.name, table_columns, primary_key);
        for fk in table_foreign_keys {
            def = def.with_foreign_key(fk);
        }
        for index in table_indexes {
            def = def.with_index(index);
        }
        result_tables.push(def);
    }

    let mut enums: Vec<EnumDef> = enums_by_name
        .into_iter()
        .map(|(name, variants)| EnumDef::new(name, variants))
        .collect();
    enums.sort_by(|a, b| a.name.cmp(&b.name));

    SchemaCatalog::new(result_tables).with_enums(enums)
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
    let raw_tables = stmt
        .query_map([], |row| {
            Ok(RawSqliteTableMetadata {
                name: row.get::<_, String>(0)?,
                sql: row.get::<_, Option<String>>(1)?,
            })
        })
        .map_err(|e| CliError(format!("failed to query SQLite tables: {e}")))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| CliError(format!("failed to read SQLite table row: {e}")))?;

    let table_names: Vec<String> = raw_tables.iter().map(|t| t.name.clone()).collect();

    let mut raw_columns = Vec::new();
    let mut raw_foreign_keys = Vec::new();
    let mut raw_indexes = Vec::new();

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

        let mut fks = fk_stmt
            .query_map([], |row| {
                let id: i64 = row.get(0)?;
                // `seq` positions the column inside its constraint.
                let position: i64 = row.get(1)?;
                let referenced_table: String = row.get(2)?;
                let from_column: String = row.get(3)?;
                let to_column: Option<String> = row.get(4)?;
                Ok((
                    (id, position),
                    RawSqliteForeignKeyMetadata {
                        table_name: table.clone(),
                        id,
                        from_column,
                        to_column: to_column.unwrap_or_default(),
                        referenced_table,
                    },
                ))
            })
            .map_err(|e| CliError(format!("failed to inspect foreign keys for `{table}`: {e}")))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| CliError(format!("failed to read foreign key for `{table}`: {e}")))?;

        // `PRAGMA foreign_key_list` reports one row per key column, and `seq`
        // carries the position of the column inside its constraint. Ordering by
        // it keeps a composite key assembled in the order it was declared in,
        // even when the pragma emits its rows in another one.
        fks.sort_by_key(|(position, _)| *position);
        raw_foreign_keys.extend(fks.into_iter().map(|(_, row)| row));

        raw_indexes.extend(sqlite_indexes(conn, table)?);
    }

    Ok(catalog_from_sqlite_metadata(
        &raw_tables,
        &raw_columns,
        &raw_foreign_keys,
        &raw_indexes,
    ))
}

/// Collects the secondary indexes declared on `table`.
///
/// `PRAGMA index_list` also reports the implicit index SQLite builds for a
/// primary key; it carries the `pk` origin and is skipped, because a primary key
/// is already described by the key columns of its table. An index over an
/// expression reports an unnamed column, so it is dropped entirely instead of
/// being recorded with a partial column list.
fn sqlite_indexes(
    conn: &rusqlite::Connection,
    table: &str,
) -> Result<Vec<RawIndexMetadata>, CliError> {
    let mut statement = conn
        .prepare(&sqlite_index_list(table))
        .map_err(|e| CliError(format!("failed to prepare index_list for `{table}`: {e}")))?;

    let declared = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)? != 0,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(|e| CliError(format!("failed to inspect indexes for `{table}`: {e}")))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| CliError(format!("failed to read index for `{table}`: {e}")))?;

    let mut rows = Vec::new();
    for (index_name, unique, origin) in declared {
        if origin == "pk" {
            continue;
        }

        let Some(columns) = sqlite_index_columns(conn, &index_name)? else {
            continue;
        };

        for column_name in columns {
            rows.push(RawIndexMetadata {
                table_name: table.to_owned(),
                index_name: index_name.clone(),
                column_name,
                unique,
            });
        }
    }

    Ok(rows)
}

/// The indexed columns of `index` in key order, or `None` for an expression index.
fn sqlite_index_columns(
    conn: &rusqlite::Connection,
    index: &str,
) -> Result<Option<Vec<String>>, CliError> {
    let mut statement = conn
        .prepare(&sqlite_index_info(index))
        .map_err(|e| CliError(format!("failed to prepare index_info for `{index}`: {e}")))?;

    let columns = statement
        .query_map([], |row| row.get::<_, Option<String>>(2))
        .map_err(|e| CliError(format!("failed to inspect columns of `{index}`: {e}")))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| CliError(format!("failed to read column of `{index}`: {e}")))?;

    Ok(columns.into_iter().collect())
}

/// Parses deterministic `CHECK (<col> IN ('val1', 'val2', ...))` enum constraints
/// from the table DDL `sql`.
pub fn parse_sqlite_check_enums(
    sql: &str,
    known_columns: &[&str],
) -> Vec<(String, Vec<String>)> {
    let mut results = Vec::new();
    let mut seen = BTreeSet::new();

    let bytes = sql.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] == b'\'' {
            i += 1;
            while i < bytes.len() {
                if bytes[i] == b'\'' {
                    if i + 1 < bytes.len() && bytes[i + 1] == b'\'' {
                        i += 2;
                    } else {
                        i += 1;
                        break;
                    }
                } else {
                    i += 1;
                }
            }
            continue;
        }

        if i + 5 <= bytes.len() && sql[i..i + 5].eq_ignore_ascii_case("check") {
            let prev_ok = i == 0
                || (!bytes[i - 1].is_ascii_alphanumeric() && bytes[i - 1] != b'_');
            let next_idx = i + 5;
            let next_ok = next_idx < bytes.len()
                && (bytes[next_idx].is_ascii_whitespace() || bytes[next_idx] == b'(');

            if prev_ok && next_ok {
                let mut paren_idx = next_idx;
                while paren_idx < bytes.len() && bytes[paren_idx].is_ascii_whitespace() {
                    paren_idx += 1;
                }
                if paren_idx < bytes.len() && bytes[paren_idx] == b'(' {
                    if let Some((inner, end_idx)) = extract_paren_block(&sql[paren_idx..]) {
                        if let Some((col, variants)) = parse_check_clause(inner, known_columns) {
                            if seen.insert(col.clone()) {
                                results.push((col, variants));
                            }
                        }
                        i = paren_idx + end_idx;
                        continue;
                    }
                }
            }
        }
        i += 1;
    }

    results
}

/// Extracts the balanced text inside `(...)` starting at `s[0] == '('`.
/// Returns `Some((inner_slice, total_length_consumed))`.
fn extract_paren_block(s: &str) -> Option<(&str, usize)> {
    if !s.starts_with('(') {
        return None;
    }

    let bytes = s.as_bytes();
    let mut depth = 0;
    let mut i = 0;

    while i < bytes.len() {
        match bytes[i] {
            b'\'' => {
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == b'\'' {
                        if i + 1 < bytes.len() && bytes[i + 1] == b'\'' {
                            i += 2;
                        } else {
                            i += 1;
                            break;
                        }
                    } else {
                        i += 1;
                    }
                }
            }
            b'"' => {
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == b'"' {
                        if i + 1 < bytes.len() && bytes[i + 1] == b'"' {
                            i += 2;
                        } else {
                            i += 1;
                            break;
                        }
                    } else {
                        i += 1;
                    }
                }
            }
            b'[' => {
                i += 1;
                while i < bytes.len() && bytes[i] != b']' {
                    i += 1;
                }
                if i < bytes.len() {
                    i += 1;
                }
            }
            b'`' => {
                i += 1;
                while i < bytes.len() && bytes[i] != b'`' {
                    i += 1;
                }
                if i < bytes.len() {
                    i += 1;
                }
            }
            b'(' => {
                depth += 1;
                i += 1;
            }
            b')' => {
                depth -= 1;
                i += 1;
                if depth == 0 {
                    return Some((&s[1..i - 1], i));
                }
            }
            _ => {
                i += 1;
            }
        }
    }

    None
}

/// Parses `<col> IN ('val1', 'val2', ...)` from the inside of a CHECK clause.
fn parse_check_clause(inner: &str, known_columns: &[&str]) -> Option<(String, Vec<String>)> {
    let mut expr = inner.trim();
    while is_enclosed_in_parens(expr) {
        expr = expr[1..expr.len() - 1].trim();
    }

    let mut had_open_paren = false;
    if let Some(rest) = expr.strip_prefix('(') {
        had_open_paren = true;
        expr = rest.trim_start();
    }

    let (col_ident, rest) = parse_column_identifier(expr)?;
    let mut rest = rest.trim_start();

    if had_open_paren {
        rest = rest.strip_prefix(')')?.trim_start();
    }

    if rest.len() < 2 || !rest[..2].eq_ignore_ascii_case("in") {
        return None;
    }
    let after_in = &rest[2..];
    if !after_in.starts_with(|c: char| c.is_ascii_whitespace() || c == '(') {
        return None;
    }

    let after_in = after_in.trim_start();
    let after_open = after_in.strip_prefix('(')?;

    let (variants, remaining) = parse_in_variant_list(after_open)?;
    let remaining = remaining.trim();
    if !remaining.is_empty() && !remaining.chars().all(|c| c == ')') {
        return None;
    }

    let matched_col = known_columns
        .iter()
        .find(|&&c| c.eq_ignore_ascii_case(&col_ident))?;

    Some(((*matched_col).to_owned(), variants))
}

fn parse_column_identifier(s: &str) -> Option<(String, &str)> {
    let s = s.trim_start();
    if s.is_empty() {
        return None;
    }

    if let Some(rest) = s.strip_prefix('"') {
        let mut name = String::new();
        let mut chars = rest.char_indices().peekable();
        while let Some((idx, ch)) = chars.next() {
            if ch == '"' {
                if chars.peek().map(|&(_, next)| next) == Some('"') {
                    chars.next();
                    name.push('"');
                } else {
                    return Some((name, &rest[idx + 1..]));
                }
            } else {
                name.push(ch);
            }
        }
        return None;
    }

    if let Some(rest) = s.strip_prefix('[') {
        let end = rest.find(']')?;
        return Some((rest[..end].to_owned(), &rest[end + 1..]));
    }

    if let Some(rest) = s.strip_prefix('`') {
        let end = rest.find('`')?;
        return Some((rest[..end].to_owned(), &rest[end + 1..]));
    }

    let mut end = 0;
    for (idx, ch) in s.char_indices() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            end = idx + ch.len_utf8();
        } else {
            break;
        }
    }

    if end == 0 {
        None
    } else {
        Some((s[..end].to_owned(), &s[end..]))
    }
}

fn parse_in_variant_list(mut s: &str) -> Option<(Vec<String>, &str)> {
    let mut variants = Vec::new();

    loop {
        s = s.trim_start();
        if let Some(rest) = s.strip_prefix(')') {
            if !variants.is_empty() {
                return Some((variants, rest));
            }
            return None;
        }

        let rest = s.strip_prefix('\'')?;
        let mut variant = String::new();
        let mut chars = rest.char_indices().peekable();
        let mut closed_at = None;

        while let Some((idx, ch)) = chars.next() {
            if ch == '\'' {
                if chars.peek().map(|&(_, next)| next) == Some('\'') {
                    chars.next();
                    variant.push('\'');
                } else {
                    closed_at = Some(idx + 1);
                    break;
                }
            } else {
                variant.push(ch);
            }
        }

        let close_pos = closed_at?;
        variants.push(variant);
        s = rest[close_pos..].trim_start();

        if let Some(after_comma) = s.strip_prefix(',') {
            s = after_comma;
        } else {
            let after_close = s.strip_prefix(')')?;
            return Some((variants, after_close));
        }
    }
}

fn is_enclosed_in_parens(s: &str) -> bool {
    if !s.starts_with('(') || !s.ends_with(')') {
        return false;
    }

    let bytes = s.as_bytes();
    let mut depth = 0;
    let mut i = 0;

    while i < bytes.len() {
        match bytes[i] {
            b'\'' => {
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == b'\'' {
                        if i + 1 < bytes.len() && bytes[i + 1] == b'\'' {
                            i += 2;
                        } else {
                            i += 1;
                            break;
                        }
                    } else {
                        i += 1;
                    }
                }
            }
            b'(' => {
                depth += 1;
                i += 1;
            }
            b')' => {
                depth -= 1;
                i += 1;
                if depth == 0 && i < bytes.len() {
                    return false;
                }
            }
            _ => {
                i += 1;
            }
        }
    }

    depth == 0
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
                sql: None,
            },
            RawSqliteTableMetadata {
                name: "order_lines".to_owned(),
                sql: None,
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

        let catalog = catalog_from_sqlite_metadata(&tables, &columns, &foreign_keys, &[]);

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
                sql: None,
            },
            RawSqliteTableMetadata {
                name: "order_lines".to_owned(),
                sql: None,
            },
        ];
        let columns = vec![
            column("orders", "id", 1),
            column("orders", "region", 2),
            column("order_lines", "order_region", 0),
        ];
        let foreign_keys = vec![key(0, "order_region", "region")];

        let catalog = catalog_from_sqlite_metadata(&tables, &columns, &foreign_keys, &[]);

        let lines = catalog
            .tables
            .iter()
            .find(|table| table.name == "order_lines")
            .expect("the child table is assembled");
        assert_eq!(lines.foreign_keys[0].referenced_columns, vec!["region"]);
    }

    #[test]
    fn composite_foreign_keys_keep_their_declared_column_order() {
        let conn = rusqlite::Connection::open_in_memory().expect("open the scratch database");
        conn.execute_batch(
            "CREATE TABLE orders (
                 id INTEGER NOT NULL,
                 region TEXT NOT NULL,
                 PRIMARY KEY (id, region)
             );
             CREATE TABLE order_lines (
                 id INTEGER PRIMARY KEY,
                 order_id INTEGER NOT NULL,
                 order_region TEXT NOT NULL,
                 FOREIGN KEY (order_id, order_region) REFERENCES orders (id, region)
             );",
        )
        .expect("create the schema");

        let catalog = introspect_sqlite(&conn).expect("the catalog is introspected");

        let lines = catalog
            .tables
            .iter()
            .find(|table| table.name == "order_lines")
            .expect("the child table is assembled");
        assert_eq!(
            lines.foreign_keys[0].columns,
            vec!["order_id", "order_region"]
        );
        assert_eq!(
            lines.foreign_keys[0].referenced_columns,
            vec!["id", "region"]
        );
    }
}
