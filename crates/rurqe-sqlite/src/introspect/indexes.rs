//! Introspection of the secondary indexes, views, and triggers of a schema.

use std::collections::HashMap;

use rurqe_migrate::introspect::{RawIndexMetadata, RawTriggerMetadata, RawViewMetadata};

use super::lexer::{matching_close, span_text, tokenize, SqlKeyword, Token, TokenKind};
use super::{
    query_error, sqlite_index_info, sqlite_index_list, SqliteCatalogError, SQLITE_INDEX_DDL,
    SQLITE_TRIGGERS, SQLITE_VIEWS,
};

/// Collects the secondary indexes declared on `table`.
///
/// `PRAGMA index_list` also reports the implicit index SQLite builds for a
/// primary key; it carries the `pk` origin and is skipped, because a primary key
/// is already described by the key columns of its table.
///
/// The pragma reports no method, no partial predicate, and no expression, so the
/// DDL of the index is read from `sqlite_master` for those. Every definition of
/// the table arrives in one statement and each one is lexed at most once, so a
/// partial index over an expression neither costs a dedicated catalog query nor
/// two lexer passes. An index over an expression reports an unnamed column, and
/// the functional terms of the parenthesised list of its DDL are then recorded as
/// the expression of the index, so the named columns it also indexes are never
/// repeated there.
pub(super) fn sqlite_indexes(
    conn: &rusqlite::Connection,
    table: &str,
) -> Result<Vec<RawIndexMetadata>, SqliteCatalogError> {
    let mut statement = conn.prepare(&sqlite_index_list(table)).map_err(|error| {
        query_error(format!("failed to prepare index_list for `{table}`"), error)
    })?;

    let declared = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)? != 0,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)? != 0,
            ))
        })
        .map_err(|error| query_error(format!("failed to inspect indexes for `{table}`"), error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| query_error(format!("failed to read index for `{table}`"), error))?;

    let definitions = sqlite_index_definitions(conn, table)?;

    let mut rows = Vec::new();
    for (index_name, unique, origin, partial) in declared {
        if origin == "pk" {
            continue;
        }

        let columns = sqlite_index_columns(conn, &index_name)?;
        let definition = definitions.get(&index_name).and_then(Option::as_deref);

        let searches_expression = columns.iter().any(Option::is_none);
        // One lexer pass serves both consumers of the definition, so a partial
        // index over an expression is never lexed twice.
        let tokens = if partial || searches_expression {
            definition.map(tokenize)
        } else {
            None
        };

        let expression = if searches_expression {
            definition
                .zip(tokens.as_deref())
                .and_then(|(sql, tokens)| sqlite_index_expressions(sql, tokens, &columns))
        } else {
            None
        };
        let predicate = if partial {
            definition
                .zip(tokens.as_deref())
                .and_then(|(sql, tokens)| predicate_of(sql, tokens))
        } else {
            None
        };

        let names: Vec<String> = columns.into_iter().flatten().collect();

        if names.is_empty() {
            // An index over an expression searches no named column, so a single
            // row carries its expression into the catalog.
            rows.push(RawIndexMetadata {
                schema: None,
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
                schema: None,
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

/// The DDL of every index declared on `table`, keyed by index name.
///
/// One statement reads the whole `sqlite_master` slice of the table, so an
/// introspection never issues a dedicated catalog query per index. An index
/// SQLite builds implicitly carries no DDL, and its entry then holds `None`.
fn sqlite_index_definitions(
    conn: &rusqlite::Connection,
    table: &str,
) -> Result<HashMap<String, Option<String>>, SqliteCatalogError> {
    let mut statement = conn.prepare(SQLITE_INDEX_DDL).map_err(|error| {
        query_error(
            format!("failed to prepare the index definitions for `{table}`"),
            error,
        )
    })?;

    let definitions = statement
        .query_map([table], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
        })
        .map_err(|error| {
            query_error(
                format!("failed to inspect index definitions for `{table}`"),
                error,
            )
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| {
            query_error(
                format!("failed to read index definition for `{table}`"),
                error,
            )
        })?;

    Ok(definitions.into_iter().collect())
}

/// The indexed columns of `index` in key order, with `None` for an expression.
fn sqlite_index_columns(
    conn: &rusqlite::Connection,
    index: &str,
) -> Result<Vec<Option<String>>, SqliteCatalogError> {
    let mut statement = conn.prepare(&sqlite_index_info(index)).map_err(|error| {
        query_error(format!("failed to prepare index_info for `{index}`"), error)
    })?;

    let columns = statement
        .query_map([], |row| row.get::<_, Option<String>>(2))
        .map_err(|error| query_error(format!("failed to inspect columns of `{index}`"), error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| query_error(format!("failed to read column of `{index}`"), error))?;

    Ok(columns)
}

/// The comma-separated terms of the parenthesised list of an index DDL.
///
/// The first opening parenthesis of an index statement introduces the list of
/// indexed terms, and the caller hands over the statement together with the
/// token stream it already lexed, so the list is never recovered by lexing the
/// statement again. The terms are cut at the `Comma` tokens written at the top
/// level of that list: a comma inside a string literal, inside a quoted
/// identifier of any of the three SQLite styles, inside a comment, or inside a
/// nested parenthesis belongs to the term it is written in and never ends one.
/// That is precisely what a hand-rolled text splitter gets wrong on an index
/// such as `ON users ([weird, name], lower(email))`, whose recorded expression
/// then reached `schema.lock` as a fragment of the identifier.
///
/// Every term is the exact source span of what the index declares, so the text
/// kept in the snapshot is the text the DDL carries.
///
/// An unbalanced list, or a statement without one, reports `None`.
pub(super) fn indexed_terms<'a>(sql: &'a str, tokens: &[Token<'_>]) -> Option<Vec<&'a str>> {
    let open = tokens
        .iter()
        .position(|token| token.kind == TokenKind::OpenParen)?;
    let close = matching_close(tokens, open)?;

    let mut terms = Vec::new();
    let mut depth = 0usize;
    let mut start = tokens.get(open)?.span.1;

    for token in tokens.get(open + 1..close)? {
        match token.kind {
            TokenKind::OpenParen => depth = depth.checked_add(1)?,
            TokenKind::CloseParen => depth = depth.checked_sub(1)?,
            TokenKind::Comma if depth == 0 => {
                terms.push(span_text(sql, start, token.span.0)?.trim());
                start = token.span.1;
            }
            _ => {}
        }
    }

    terms.push(span_text(sql, start, tokens.get(close)?.span.0)?.trim());
    Some(terms)
}

/// The functional expressions of an index, without the columns it also indexes.
///
/// The parenthesised list of an index DDL carries every indexed term in key
/// order, and `PRAGMA index_info` reports the very same terms with a `NULL` name
/// for each one declared as an expression. Keeping the terms whose reported name
/// is absent leaves the expressions alone, so a mixed index neither repeats its
/// named columns in the expression field nor loses the expressions it computes.
fn sqlite_index_expressions(
    sql: &str,
    tokens: &[Token<'_>],
    columns: &[Option<String>],
) -> Option<String> {
    let terms = indexed_terms(sql, tokens)?;

    let expressions: Vec<&str> = terms
        .into_iter()
        .zip(columns.iter())
        .filter_map(|(term, column)| column.is_none().then_some(term))
        .filter(|term| !term.is_empty())
        .collect();

    if expressions.is_empty() {
        None
    } else {
        Some(expressions.join(", "))
    }
}

/// The partial index predicate of an index DDL, if it declares one.
///
/// The predicate is the text after the `WHERE` keyword that closes the index
/// statement, and a `WHERE` written inside a comment or a nested parenthesis is
/// skipped by the token stream the caller lexed once.
pub(super) fn predicate_of(sql: &str, tokens: &[Token<'_>]) -> Option<String> {
    let mut depth = 0usize;

    for token in tokens {
        match token.kind {
            TokenKind::OpenParen => depth = depth.saturating_add(1),
            TokenKind::CloseParen => depth = depth.saturating_sub(1),
            TokenKind::Keyword(SqlKeyword::Where) if depth == 0 => {
                let predicate = sql.get(token.span.1..)?.trim().trim_end_matches(';').trim();
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
pub(super) fn sqlite_views(
    conn: &rusqlite::Connection,
) -> Result<Vec<RawViewMetadata>, SqliteCatalogError> {
    let mut statement = conn
        .prepare(SQLITE_VIEWS)
        .map_err(|error| query_error("failed to prepare SQLite view query", error))?;

    let views = statement
        .query_map([], |row| {
            Ok(RawViewMetadata {
                schema: None,
                name: row.get::<_, String>(0)?,
                definition: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
            })
        })
        .map_err(|error| query_error("failed to query SQLite views", error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| query_error("failed to read SQLite view row", error))?;

    Ok(views)
}

/// Collects the triggers declared by the schema.
pub(super) fn sqlite_triggers(
    conn: &rusqlite::Connection,
) -> Result<Vec<RawTriggerMetadata>, SqliteCatalogError> {
    let mut statement = conn
        .prepare(SQLITE_TRIGGERS)
        .map_err(|error| query_error("failed to prepare SQLite trigger query", error))?;

    let triggers = statement
        .query_map([], |row| {
            Ok(RawTriggerMetadata {
                schema: None,
                name: row.get::<_, String>(0)?,
                table_name: row.get::<_, String>(1)?,
                definition: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
            })
        })
        .map_err(|error| query_error("failed to query SQLite triggers", error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| query_error("failed to read SQLite trigger row", error))?;

    Ok(triggers)
}
