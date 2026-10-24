//! Introspection of the secondary indexes, views, and triggers of a schema.

use crate::cli::CliError;
use crate::introspect::types::{RawIndexMetadata, RawTriggerMetadata, RawViewMetadata};

use super::lexer::{matching_close, tokenize, SqlKeyword, TokenKind};
use super::{sqlite_index_info, sqlite_index_list, SQLITE_INDEX_DDL, SQLITE_TRIGGERS, SQLITE_VIEWS};

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
pub(super) fn sqlite_indexes(
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
/// indexed terms, so everything inside it is what the index searches. The token
/// stream already splits quoted and commented spans apart, so a parenthesis
/// written inside a comment never opens the list.
pub(super) fn column_list_of(sql: &str) -> Option<String> {
    let tokens = tokenize(sql);
    let open = tokens
        .iter()
        .position(|token| token.kind == TokenKind::OpenParen)?;
    let close = matching_close(&tokens, open)?;
    let start = tokens[open].span.1;
    let end = tokens[close].span.0;
    Some(sql[start..end].trim().to_owned())
}

/// The partial index predicate of an index DDL, if it declares one.
///
/// The predicate is the text after the `WHERE` keyword that closes the index
/// statement, and a `WHERE` written inside a comment or a nested parenthesis is
/// skipped by the token stream.
pub(super) fn predicate_of(sql: &str) -> Option<String> {
    let tokens = tokenize(sql);
    let mut depth = 0usize;

    for token in &tokens {
        match token.kind {
            TokenKind::OpenParen => depth = depth.saturating_add(1),
            TokenKind::CloseParen => depth = depth.saturating_sub(1),
            TokenKind::Keyword(SqlKeyword::Where) if depth == 0 => {
                let predicate = sql[token.span.1..].trim().trim_end_matches(';').trim();
                return if predicate.is_empty() {
                    None
                } else {
                    Some(predicate.to_owned())
                };
            }
            _ => {}
        }
    }

    None
}

/// Collects the views declared by the schema.
///
/// `sqlite_master` stores the statement a view was created with, so the catalog
/// keeps that statement as the definition of the view.
pub(super) fn sqlite_views(conn: &rusqlite::Connection) -> Result<Vec<RawViewMetadata>, CliError> {
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
pub(super) fn sqlite_triggers(
    conn: &rusqlite::Connection,
) -> Result<Vec<RawTriggerMetadata>, CliError> {
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
