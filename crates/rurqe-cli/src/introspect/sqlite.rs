//! SQLite catalog introspection through `PRAGMA` queries and `sqlite_master`.
//!
//! The catalog assembly of this module reads only the rows the pragmas report;
//! the parsing of the `CHECK (<column> IN (...))` constraints SQLite spells enums
//! with lives in [`check_enums`], and the DDL text of the table and index
//! declarations is scanned here for the attributes a pragma does not report.

use std::collections::{BTreeMap, BTreeSet};

use rurqe_migrate::catalog::{
    CheckDef, ColumnDef, EnumDef, ForeignKeyDef, SchemaCatalog, TableDef, TypeKind,
};

use crate::cli::CliError;

use super::types::{
    fold_indexes, map_sqlite_declared_type_to_kind, parse_foreign_key_action, RawCatalogSections,
    RawCheckMetadata, RawIndexMetadata, RawTriggerMetadata, RawViewMetadata,
};

mod check_enums;

use check_enums::extract_paren_block;

pub use check_enums::parse_sqlite_check_enums;

/// Query to fetch user tables and their DDL from the SQLite `sqlite_master` catalog.
pub(crate) const SQLITE_TABLES: &str = r#"
SELECT name, sql
FROM sqlite_master
WHERE type = 'table'
  AND name NOT LIKE 'sqlite_%'
  AND name != '__rurqe_schema_history'
ORDER BY name;
"#;

/// Query to fetch the views of the schema together with their DDL.
pub(crate) const SQLITE_VIEWS: &str = r#"
SELECT name, sql
FROM sqlite_master
WHERE type = 'view'
  AND name NOT LIKE 'sqlite_%'
ORDER BY name;
"#;

/// Query to fetch the triggers of the schema together with their DDL.
pub(crate) const SQLITE_TRIGGERS: &str = r#"
SELECT name, tbl_name, sql
FROM sqlite_master
WHERE type = 'trigger'
  AND name NOT LIKE 'sqlite_%'
ORDER BY name;
"#;

/// Query to fetch the DDL an index was declared with.
///
/// An index SQLite builds implicitly for a `UNIQUE` constraint carries no DDL, and
/// the statement then reports no row at all.
const SQLITE_INDEX_DDL: &str = "SELECT sql FROM sqlite_master WHERE type = 'index' AND name = ?1;";

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
    format!(
        "PRAGMA index_list(\"{}\");",
        escape_sqlite_identifier(table)
    )
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
    pub on_delete: rurqe_migrate::catalog::ForeignKeyAction,
    pub on_update: rurqe_migrate::catalog::ForeignKeyAction,
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
/// table and that id, so composite foreign keys stay a single constraint. The
/// referential actions of the key are carried into the definition.
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
    catalog_from_sqlite_metadata_with_sections(
        tables,
        columns,
        foreign_keys,
        indexes,
        &RawCatalogSections::default(),
    )
}

/// Assembles a normalized [`SchemaCatalog`] including checks, views, and triggers.
pub fn catalog_from_sqlite_metadata_with_sections(
    tables: &[RawSqliteTableMetadata],
    columns: &[RawSqliteColumnMetadata],
    foreign_keys: &[RawSqliteForeignKeyMetadata],
    indexes: &[RawIndexMetadata],
    sections: &RawCatalogSections<'_>,
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
                check_enums.insert((table.name.clone(), col_name), enum_name);
            }
        }
    }

    let mut columns_by_table: BTreeMap<String, Vec<ColumnDef>> = BTreeMap::new();
    let mut fks_by_table: BTreeMap<String, BTreeMap<i64, ForeignKeyDef>> = BTreeMap::new();
    let mut checks_by_table: BTreeMap<String, Vec<CheckDef>> = BTreeMap::new();
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
                .with_actions(fk.on_delete, fk.on_update)
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

    for check in sections.checks {
        checks_by_table
            .entry(check.table_name.clone())
            .or_default()
            .push(CheckDef::new(&check.name, &check.expression));
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
        let table_checks = checks_by_table.remove(&table.name).unwrap_or_default();
        let table_indexes = indexes_by_table.remove(&table.name).unwrap_or_default();

        let mut def = TableDef::new(&table.name, table_columns, primary_key);
        for check in table_checks {
            def = def.with_check(check);
        }
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

    let views: Vec<rurqe_migrate::catalog::ViewDef> = sections
        .views
        .iter()
        .map(|view| rurqe_migrate::catalog::ViewDef::new(&view.name, &view.definition))
        .collect();
    let triggers: Vec<rurqe_migrate::catalog::TriggerDef> = sections
        .triggers
        .iter()
        .map(|trigger| {
            rurqe_migrate::catalog::TriggerDef::new(
                &trigger.name,
                &trigger.table_name,
                &trigger.definition,
            )
        })
        .collect();

    SchemaCatalog::new(result_tables)
        .with_enums(enums)
        .with_views(views)
        .with_triggers(triggers)
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
                let on_update: String = row.get(5)?;
                let on_delete: String = row.get(6)?;
                Ok((
                    (id, position),
                    RawSqliteForeignKeyMetadata {
                        table_name: table.clone(),
                        id,
                        from_column,
                        to_column: to_column.unwrap_or_default(),
                        referenced_table,
                        on_delete: parse_foreign_key_action(&on_delete),
                        on_update: parse_foreign_key_action(&on_update),
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

    let raw_checks = sqlite_checks(&raw_tables, &raw_columns);
    let raw_views = sqlite_views(conn)?;
    let raw_triggers = sqlite_triggers(conn)?;

    Ok(catalog_from_sqlite_metadata_with_sections(
        &raw_tables,
        &raw_columns,
        &raw_foreign_keys,
        &raw_indexes,
        &RawCatalogSections {
            checks: &raw_checks,
            views: &raw_views,
            triggers: &raw_triggers,
        },
    ))
}

/// Collects the check constraints declared by the tables of the schema.
///
/// A check closing the value set of a column (`<column> IN (...)`) is the enum
/// spelling SQLite uses and is described by an enum type instead, so it never
/// becomes a check constraint of the catalog.
fn sqlite_checks(
    tables: &[RawSqliteTableMetadata],
    columns: &[RawSqliteColumnMetadata],
) -> Vec<RawCheckMetadata> {
    let mut checks = Vec::new();

    for table in tables {
        let Some(sql) = table.sql.as_deref() else {
            continue;
        };
        let known: Vec<&str> = columns
            .iter()
            .filter(|column| column.table_name == table.name)
            .map(|column| column.column_name.as_str())
            .collect();

        for check in parse_sqlite_checks(&table.name, sql, &known) {
            checks.push(RawCheckMetadata {
                table_name: table.name.clone(),
                name: check.name,
                expression: check.expression,
            });
        }
    }

    checks
}

/// Parses the check constraints of a table DDL.
///
/// A named constraint keeps the name it was declared with, while an unnamed one
/// receives a deterministic name derived from its table and position, so the
/// same DDL always yields the same catalog and never drifts from itself.
fn parse_sqlite_checks(table: &str, sql: &str, known_columns: &[&str]) -> Vec<CheckDef> {
    let enum_columns: BTreeSet<String> = parse_sqlite_check_enums(sql, known_columns)
        .into_iter()
        .map(|(column, _)| column)
        .collect();

    let mut checks = Vec::new();
    let mut unnamed = 0usize;

    for (name, expression) in scan_check_constraints(sql) {
        if names_an_enum_column(&expression, &enum_columns) {
            continue;
        }

        let name = match name {
            Some(name) => name,
            None => {
                let name = format!("{table}_check_{unnamed}");
                unnamed += 1;
                name
            }
        };
        checks.push(CheckDef::new(name, expression));
    }

    checks
}

/// Collects every `CHECK ( ... )` clause of a table DDL with its constraint name.
///
/// The scan skips string literals and quoted identifiers, so a `CHECK` written
/// inside a value is never read as a constraint. A `CONSTRAINT <name>` seen
/// before a check names it; a comma ends the current table element and clears a
/// name that belonged to another constraint.
fn scan_check_constraints(sql: &str) -> Vec<(Option<String>, String)> {
    const CONSTRAINT: &str = "constraint";
    const CHECK: &str = "check";

    let bytes = sql.as_bytes();
    let mut results = Vec::new();
    let mut pending: Option<String> = None;
    let mut index = 0;

    while index < bytes.len() {
        match bytes[index] {
            b'\'' | b'"' | b'`' => index = skip_quoted(bytes, index),
            b'[' => index = skip_until(bytes, index, b']'),
            b',' => {
                pending = None;
                index += 1;
            }
            _ => {
                if keyword_at(bytes, index, CONSTRAINT) {
                    match constraint_name(bytes, index + CONSTRAINT.len()) {
                        Some((name, next)) => {
                            pending = Some(name);
                            index = next;
                        }
                        None => index += 1,
                    }
                    continue;
                }

                if keyword_at(bytes, index, CHECK) {
                    let mut open = index + CHECK.len();
                    while open < bytes.len() && bytes[open].is_ascii_whitespace() {
                        open += 1;
                    }

                    if open < bytes.len() && bytes[open] == b'(' && sql.is_char_boundary(open) {
                        if let Some((inner, length)) = extract_paren_block(&sql[open..]) {
                            results.push((pending.take(), inner.trim().to_owned()));
                            index = open + length;
                            continue;
                        }
                    }

                    pending = None;
                }

                index += 1;
            }
        }
    }

    results
}

/// Whether a check predicate closes the value set of an enum column.
fn names_an_enum_column(expression: &str, enum_columns: &BTreeSet<String>) -> bool {
    let mut text = expression.trim();
    while text.len() >= 2 && text.starts_with('(') && text.ends_with(')') {
        text = text[1..text.len() - 1].trim();
    }

    let Some((identifier, rest)) = split_identifier(text) else {
        return false;
    };

    let mut words = rest.split_whitespace();
    matches!(words.next(), Some(word) if word.eq_ignore_ascii_case("in"))
        && enum_columns
            .iter()
            .any(|column| column.eq_ignore_ascii_case(&identifier))
}

/// Splits a leading SQL identifier off `text`.
fn split_identifier(text: &str) -> Option<(String, &str)> {
    if let Some(rest) = text.strip_prefix('"') {
        let mut name = String::new();
        let mut chars = rest.char_indices().peekable();
        while let Some((index, character)) = chars.next() {
            if character == '"' {
                if chars.peek().map(|&(_, next)| next) == Some('"') {
                    chars.next();
                    name.push('"');
                } else {
                    return Some((name, &rest[index + 1..]));
                }
            } else {
                name.push(character);
            }
        }
        return None;
    }

    let end = text
        .char_indices()
        .take_while(|(_, character)| character.is_ascii_alphanumeric() || *character == '_')
        .map(|(index, character)| index + character.len_utf8())
        .last()?;

    Some((text[..end].to_owned(), &text[end..]))
}

/// Reads the identifier that follows a `CONSTRAINT` keyword.
fn constraint_name(bytes: &[u8], mut index: usize) -> Option<(String, usize)> {
    while index < bytes.len() && bytes[index].is_ascii_whitespace() {
        index += 1;
    }

    let quote = match bytes.get(index) {
        Some(b'"') => Some(b'"'),
        Some(b'\'') => Some(b'\''),
        Some(b'`') => Some(b'`'),
        _ => None,
    };

    if let Some(quote) = quote {
        let mut name = Vec::new();
        index += 1;
        while index < bytes.len() {
            if bytes[index] == quote {
                if index + 1 < bytes.len() && bytes[index + 1] == quote {
                    name.push(quote);
                    index += 2;
                    continue;
                }
                index += 1;
                break;
            }
            name.push(bytes[index]);
            index += 1;
        }
        return String::from_utf8(name).ok().map(|name| (name, index));
    }

    let start = index;
    while index < bytes.len() && (bytes[index].is_ascii_alphanumeric() || bytes[index] == b'_') {
        index += 1;
    }

    if index == start {
        return None;
    }

    String::from_utf8(bytes[start..index].to_vec())
        .ok()
        .map(|name| (name, index))
}

/// Whether `keyword` appears at `index` of `bytes` on an identifier boundary.
fn keyword_at(bytes: &[u8], index: usize, keyword: &str) -> bool {
    let keyword = keyword.as_bytes();
    let end = index + keyword.len();

    if end > bytes.len() || !bytes[index..end].eq_ignore_ascii_case(keyword) {
        return false;
    }

    let before_ok = index == 0 || !is_identifier_byte(bytes[index - 1]);
    let after_ok = end == bytes.len() || !is_identifier_byte(bytes[end]);

    before_ok && after_ok
}

/// Whether `byte` continues a SQL identifier.
fn is_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$'
}

/// Advances past the quoted token opening at `index`.
fn skip_quoted(bytes: &[u8], index: usize) -> usize {
    let quote = bytes[index];
    let mut cursor = index + 1;

    while cursor < bytes.len() {
        if bytes[cursor] == quote {
            if cursor + 1 < bytes.len() && bytes[cursor + 1] == quote {
                cursor += 2;
                continue;
            }
            return cursor + 1;
        }
        cursor += 1;
    }

    bytes.len()
}

/// Advances past the `[ ... ]` token opening at `index`.
fn skip_until(bytes: &[u8], index: usize, close: u8) -> usize {
    let mut cursor = index + 1;
    while cursor < bytes.len() {
        if bytes[cursor] == close {
            return cursor + 1;
        }
        cursor += 1;
    }
    bytes.len()
}

/// Collects the views declared by the schema.
///
/// `sqlite_master` stores the statement a view was created with, so the catalog
/// keeps that statement as the definition of the view.
fn sqlite_views(conn: &rusqlite::Connection) -> Result<Vec<RawViewMetadata>, CliError> {
    let mut statement = conn
        .prepare(SQLITE_VIEWS)
        .map_err(|e| CliError(format!("failed to prepare SQLite view query: {e}")))?;

    let views = statement
        .query_map([], |row| {
            Ok(RawViewMetadata {
                name: row.get::<_, String>(0)?,
                definition: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
            })
        })
        .map_err(|e| CliError(format!("failed to query SQLite views: {e}")))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| CliError(format!("failed to read SQLite view row: {e}")))?;

    Ok(views)
}

/// Collects the triggers declared by the schema.
fn sqlite_triggers(conn: &rusqlite::Connection) -> Result<Vec<RawTriggerMetadata>, CliError> {
    let mut statement = conn
        .prepare(SQLITE_TRIGGERS)
        .map_err(|e| CliError(format!("failed to prepare SQLite trigger query: {e}")))?;

    let triggers = statement
        .query_map([], |row| {
            Ok(RawTriggerMetadata {
                name: row.get::<_, String>(0)?,
                table_name: row.get::<_, String>(1)?,
                definition: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
            })
        })
        .map_err(|e| CliError(format!("failed to query SQLite triggers: {e}")))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| CliError(format!("failed to read SQLite trigger row: {e}")))?;

    Ok(triggers)
}

/// Collects the secondary indexes declared on `table`.
///
/// `PRAGMA index_list` also reports the implicit index SQLite builds for a
/// primary key; it carries the `pk` origin and is skipped, because a primary key
/// is already described by the key columns of its table.
///
/// The pragma reports no method, no partial predicate, and no expression, so the
/// DDL of the index is read from `sqlite_master` for those. An index over an
/// expression reports an unnamed column, and the parenthesised column list of its
/// DDL is then recorded as the expression of the index.
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
                row.get::<_, i64>(4)? != 0,
            ))
        })
        .map_err(|e| CliError(format!("failed to inspect indexes for `{table}`: {e}")))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| CliError(format!("failed to read index for `{table}`: {e}")))?;

    let mut rows = Vec::new();
    for (index_name, unique, origin, partial) in declared {
        if origin == "pk" {
            continue;
        }

        let columns = sqlite_index_columns(conn, &index_name)?;
        let ddl = sqlite_index_ddl(conn, &index_name)?;

        let expression = match ddl.as_deref() {
            Some(sql) if columns.iter().any(Option::is_none) => column_list_of(sql),
            _ => None,
        };
        let predicate = match ddl.as_deref() {
            Some(sql) if partial => predicate_of(sql),
            _ => None,
        };

        let names: Vec<String> = columns.into_iter().flatten().collect();

        if names.is_empty() {
            // An index over an expression searches no named column, so a single
            // row carries its expression into the catalog.
            rows.push(RawIndexMetadata {
                table_name: table.to_owned(),
                index_name,
                column_name: None,
                unique,
                method: None,
                predicate,
                expression,
            });
            continue;
        }

        for column_name in names {
            rows.push(RawIndexMetadata {
                table_name: table.to_owned(),
                index_name: index_name.clone(),
                column_name: Some(column_name),
                unique,
                method: None,
                predicate: predicate.clone(),
                expression: expression.clone(),
            });
        }
    }

    Ok(rows)
}

/// The indexed columns of `index` in key order, with `None` for an expression.
fn sqlite_index_columns(
    conn: &rusqlite::Connection,
    index: &str,
) -> Result<Vec<Option<String>>, CliError> {
    let mut statement = conn
        .prepare(&sqlite_index_info(index))
        .map_err(|e| CliError(format!("failed to prepare index_info for `{index}`: {e}")))?;

    let columns = statement
        .query_map([], |row| row.get::<_, Option<String>>(2))
        .map_err(|e| CliError(format!("failed to inspect columns of `{index}`: {e}")))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| CliError(format!("failed to read column of `{index}`: {e}")))?;

    Ok(columns)
}

/// The DDL an index was declared with, if `sqlite_master` records one.
fn sqlite_index_ddl(conn: &rusqlite::Connection, index: &str) -> Result<Option<String>, CliError> {
    let mut statement = conn
        .prepare(SQLITE_INDEX_DDL)
        .map_err(|e| CliError(format!("failed to prepare the index definition query: {e}")))?;

    match statement.query_row([index], |row| row.get::<_, Option<String>>(0)) {
        Ok(sql) => Ok(sql),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(error) => Err(CliError(format!(
            "failed to read the definition of `{index}`: {error}"
        ))),
    }
}

/// The parenthesised column list of an index DDL.
///
/// The first opening parenthesis of an index statement introduces the list of
/// indexed terms, so everything inside it is what the index searches.
fn column_list_of(sql: &str) -> Option<String> {
    let open = first_unquoted(sql, b'(')?;
    let (inner, _) = extract_paren_block(&sql[open..])?;
    Some(inner.trim().to_owned())
}

/// The partial index predicate of an index DDL, if it declares one.
fn predicate_of(sql: &str) -> Option<String> {
    const WHERE: &str = "where";

    let position = find_keyword(sql, WHERE)?;
    let predicate = sql[position + WHERE.len()..]
        .trim()
        .trim_end_matches(';')
        .trim();

    if predicate.is_empty() {
        None
    } else {
        Some(predicate.to_owned())
    }
}

/// The byte index of the first `target` byte outside a quoted token.
fn first_unquoted(sql: &str, target: u8) -> Option<usize> {
    let bytes = sql.as_bytes();
    let mut index = 0;

    while index < bytes.len() {
        match bytes[index] {
            b'\'' | b'"' | b'`' => index = skip_quoted(bytes, index),
            b'[' => index = skip_until(bytes, index, b']'),
            byte if byte == target => return Some(index),
            _ => index += 1,
        }
    }

    None
}

/// The byte index of `keyword` outside every parenthesised block and quoted token.
fn find_keyword(sql: &str, keyword: &str) -> Option<usize> {
    let bytes = sql.as_bytes();
    let mut depth = 0usize;
    let mut index = 0;

    while index < bytes.len() {
        match bytes[index] {
            b'\'' | b'"' | b'`' => index = skip_quoted(bytes, index),
            b'[' => index = skip_until(bytes, index, b']'),
            b'(' => {
                depth += 1;
                index += 1;
            }
            b')' => {
                depth = depth.saturating_sub(1);
                index += 1;
            }
            _ => {
                if depth == 0 && keyword_at(bytes, index, keyword) {
                    return Some(index);
                }
                index += 1;
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurqe_migrate::catalog::ForeignKeyAction;

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
            on_delete: ForeignKeyAction::NoAction,
            on_update: ForeignKeyAction::NoAction,
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

    #[test]
    fn referential_actions_are_read_from_the_pragma() {
        let conn = rusqlite::Connection::open_in_memory().expect("open the scratch database");
        conn.execute_batch(
            "CREATE TABLE users (id INTEGER PRIMARY KEY);
             CREATE TABLE orders (
                 id INTEGER PRIMARY KEY,
                 user_id INTEGER NOT NULL,
                 FOREIGN KEY (user_id) REFERENCES users (id)
                     ON DELETE CASCADE ON UPDATE RESTRICT
             );",
        )
        .expect("create the schema");

        let catalog = introspect_sqlite(&conn).expect("the catalog is introspected");
        let orders = catalog
            .tables
            .iter()
            .find(|table| table.name == "orders")
            .expect("the table is assembled");

        assert_eq!(orders.foreign_keys[0].on_delete, ForeignKeyAction::Cascade);
        assert_eq!(orders.foreign_keys[0].on_update, ForeignKeyAction::Restrict);
    }

    #[test]
    fn a_foreign_key_action_is_read_from_its_declared_spelling() {
        assert_eq!(
            parse_foreign_key_action("SET NULL"),
            ForeignKeyAction::SetNull
        );
        assert_eq!(
            parse_foreign_key_action("set default"),
            ForeignKeyAction::SetDefault
        );
        assert_eq!(
            parse_foreign_key_action("cascade"),
            ForeignKeyAction::Cascade
        );
        assert_eq!(
            parse_foreign_key_action("NO ACTION"),
            ForeignKeyAction::NoAction
        );
        assert_eq!(
            parse_foreign_key_action("something else"),
            ForeignKeyAction::NoAction
        );
    }

    #[test]
    fn a_check_constraint_inside_a_literal_is_not_read() {
        let sql = "CREATE TABLE notes (body TEXT DEFAULT 'CHECK (length(body) > 0)');";

        assert_eq!(scan_check_constraints(sql), Vec::new());
    }

    #[test]
    fn a_named_check_keeps_its_declared_name() {
        let sql = "CREATE TABLE users (\n\
                     id INTEGER PRIMARY KEY,\n\
                     age INTEGER NOT NULL CONSTRAINT users_age_check CHECK (age >= 0),\n\
                     email TEXT NOT NULL CHECK (length(email) > 0)\n\
                   );";

        let checks = parse_sqlite_checks("users", sql, &["id", "age", "email"]);

        assert_eq!(checks.len(), 2);
        assert_eq!(checks[0].name, "users_age_check");
        assert_eq!(checks[0].expression, "age >= 0");
        assert_eq!(checks[1].name, "users_check_0");
        assert_eq!(checks[1].expression, "length(email) > 0");
    }

    #[test]
    fn an_enum_check_stays_an_enum() {
        let sql = "CREATE TABLE tasks (\n\
                     status TEXT NOT NULL CHECK (status IN ('todo', 'done'))\n\
                   );";

        let checks = parse_sqlite_checks("tasks", sql, &["status"]);

        assert!(checks.is_empty(), "unexpected checks: {checks:?}");
    }

    #[test]
    fn a_partial_and_an_expression_index_keep_their_attributes() {
        let conn = rusqlite::Connection::open_in_memory().expect("open the scratch database");
        conn.execute_batch(
            "CREATE TABLE users (
                 id INTEGER PRIMARY KEY,
                 email TEXT NOT NULL
             );
             CREATE UNIQUE INDEX users_email_idx ON users (email) WHERE email IS NOT NULL;
             CREATE INDEX users_lower_email_idx ON users (lower(email));",
        )
        .expect("create the schema");

        let catalog = introspect_sqlite(&conn).expect("the catalog is introspected");
        let table = &catalog.tables[0];
        let index = |name: &str| {
            table
                .indexes
                .iter()
                .find(|index| index.name == name)
                .expect("the index is introspected")
        };

        let partial = index("users_email_idx");
        assert_eq!(partial.columns, vec!["email"]);
        assert!(partial.unique);
        assert_eq!(partial.predicate.as_deref(), Some("email IS NOT NULL"));

        let expression = index("users_lower_email_idx");
        assert!(expression.columns.is_empty());
        assert_eq!(expression.expression.as_deref(), Some("lower(email)"));
        assert!(expression.predicate.is_none());
    }

    #[test]
    fn views_and_triggers_are_introspected() {
        let conn = rusqlite::Connection::open_in_memory().expect("open the scratch database");
        conn.execute_batch(
            "CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT NOT NULL);
             CREATE VIEW active_users AS SELECT id FROM users;
             CREATE TRIGGER users_touch AFTER UPDATE ON users BEGIN SELECT 1; END;",
        )
        .expect("create the schema");

        let catalog = introspect_sqlite(&conn).expect("the catalog is introspected");

        assert_eq!(catalog.views.len(), 1);
        assert_eq!(catalog.views[0].name, "active_users");
        assert!(catalog.views[0].definition.contains("CREATE VIEW"));

        assert_eq!(catalog.triggers.len(), 1);
        assert_eq!(catalog.triggers[0].name, "users_touch");
        assert_eq!(catalog.triggers[0].table_name, "users");
        assert!(catalog.triggers[0].definition.contains("CREATE TRIGGER"));
    }
}
