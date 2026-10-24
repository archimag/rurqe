//! Collection of the `CHECK (...)` constraints declared by the tables of a schema.

use std::collections::BTreeSet;

use rurqe_migrate::catalog::CheckDef;

use crate::introspect::types::RawCheckMetadata;

use super::check_enums::parse_sqlite_check_enums;
use super::lexer::{tokenize, SqlKeyword, Token, TokenKind};
use super::{RawSqliteColumnMetadata, RawSqliteTableMetadata};

/// Collects the check constraints declared by the tables of the schema.
///
/// A check closing the value set of a column (`<column> IN (...)`) is the enum
/// spelling SQLite uses and is described by an enum type instead, so it never
/// becomes a check constraint of the catalog.
pub(super) fn sqlite_checks(
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
pub(super) fn parse_sqlite_checks(
    table: &str,
    sql: &str,
    known_columns: &[&str],
) -> Vec<CheckDef> {
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
/// The scan runs over the token stream of the shared lexer, so a `CHECK` or a
/// `CONSTRAINT` written inside a literal, a quoted identifier, or a comment is
/// never read as a constraint. A `CONSTRAINT <name>` seen before a check names
/// it; a comma ends the current table element and clears a name that belonged to
/// another constraint.
pub(super) fn scan_check_constraints(sql: &str) -> Vec<(Option<String>, String)> {
    let tokens = tokenize(sql);
    let mut results = Vec::new();
    let mut pending: Option<String> = None;
    let mut index = 0;

    while index < tokens.len() {
        match tokens[index].kind {
            TokenKind::Comma => {
                pending = None;
                index += 1;
            }
            TokenKind::Keyword(SqlKeyword::Constraint) => {
                match tokens.get(index + 1).and_then(name_of) {
                    Some(name) => {
                        pending = Some(name);
                        index += 2;
                    }
                    None => index += 1,
                }
            }
            TokenKind::Keyword(SqlKeyword::Check) => {
                let open = index + 1;
                if tokens.get(open).map(|token| token.kind) == Some(TokenKind::OpenParen) {
                    if let Some(close) = matching_close(&tokens, open) {
                        let start = tokens[open].span.1;
                        let end = tokens[close].span.0;
                        results.push((pending.take(), sql[start..end].trim().to_owned()));
                        index = close + 1;
                        continue;
                    }
                }

                pending = None;
                index += 1;
            }
            _ => index += 1,
        }
    }

    results
}

/// The unescaped name a constraint identifier token carries, if it is one.
fn name_of(token: &Token<'_>) -> Option<String> {
    match token.kind {
        TokenKind::Ident | TokenKind::QuotedIdent(_) => Some(token.unescaped_ident()),
        TokenKind::StringLiteral => Some(token.unescaped_string()),
        _ => None,
    }
}

/// Whether a check predicate closes the value set of an enum column.
fn names_an_enum_column(expression: &str, enum_columns: &BTreeSet<String>) -> bool {
    let tokens = tokenize(expression);
    let (start, _) = trim_wrapping_parens(&tokens, 0, tokens.len());

    let Some(column) = tokens.get(start) else {
        return false;
    };
    let name = match column.kind {
        TokenKind::Ident | TokenKind::QuotedIdent(_) => column.unescaped_ident(),
        _ => return false,
    };

    if !matches!(
        tokens.get(start + 1).map(|token| token.kind),
        Some(TokenKind::Keyword(SqlKeyword::In))
    ) {
        return false;
    }

    enum_columns
        .iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(&name))
}

/// The token index of the parenthesis closing the one at `open`.
pub(super) fn matching_close(tokens: &[Token<'_>], open: usize) -> Option<usize> {
    if tokens.get(open).map(|token| token.kind) != Some(TokenKind::OpenParen) {
        return None;
    }

    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate().skip(open) {
        match token.kind {
            TokenKind::OpenParen => depth = depth.checked_add(1)?,
            TokenKind::CloseParen => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }

    None
}

/// Strips the parenthesis pairs wrapping the whole `start..end` token range.
fn trim_wrapping_parens(tokens: &[Token<'_>], mut start: usize, mut end: usize) -> (usize, usize) {
    while start < end {
        if tokens.get(start).map(|token| token.kind) != Some(TokenKind::OpenParen) {
            break;
        }
        match matching_close(tokens, start) {
            Some(close) if close + 1 == end => {
                start += 1;
                end = close;
            }
            _ => break,
        }
    }

    (start, end)
}
