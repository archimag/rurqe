//! Collection of the `CHECK (...)` constraints declared by the tables of a schema.

use std::ops::Range;

use rurqe_migrate::catalog::CheckDef;

use rurqe_migrate::introspect::RawCheckMetadata;

use super::check_enums::{parse_check_clause, parse_check_enums};
use super::lexer::{matching_close, span_text, tokenize, SqlKeyword, Token, TokenKind};
use super::{RawSqliteColumnMetadata, RawSqliteTableMetadata};

/// An enum column recovered from the `CHECK (<column> IN (...))` constraint of a table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SqliteEnumColumn {
    /// The table declaring the constraint.
    pub(super) table_name: String,
    /// The column whose value set the constraint closes.
    pub(super) column_name: String,
    /// The accepted values, in declaration order.
    pub(super) variants: Vec<String>,
}

/// The constraints a single scan of a table DDL recovers.
struct ScannedTable {
    /// The `(column, variants)` pairs of the enum constraints the DDL declares.
    enums: Vec<(String, Vec<String>)>,
    /// The check constraints of the table that are not enum declarations.
    checks: Vec<CheckDef>,
}

/// Collects the check constraints and the enum columns declared by the tables of
/// the schema.
///
/// A check closing the value set of a column (`<column> IN (...)`) is the enum
/// spelling SQLite uses and is described by an enum type instead, so it never
/// becomes a check constraint of the catalog. Both results come out of one scan
/// of every table DDL, so a catalog build lexes each statement once.
pub(super) fn sqlite_checks(
    tables: &[RawSqliteTableMetadata],
    columns: &[RawSqliteColumnMetadata],
) -> (Vec<RawCheckMetadata>, Vec<SqliteEnumColumn>) {
    let mut checks = Vec::new();
    let mut enum_columns = Vec::new();

    for table in tables {
        let Some(sql) = table.sql.as_deref() else {
            continue;
        };
        let known: Vec<&str> = columns
            .iter()
            .filter(|column| column.table_name == table.name)
            .map(|column| column.column_name.as_str())
            .collect();

        let ScannedTable {
            enums,
            checks: table_checks,
        } = scan_table_ddl(&table.name, sql, &known);

        for check in table_checks {
            checks.push(RawCheckMetadata {
                schema: None,
                table_name: table.name.clone(),
                name: check.name,
                expression: check.expression,
            });
        }

        for (column_name, variants) in enums {
            enum_columns.push(SqliteEnumColumn {
                table_name: table.name.clone(),
                column_name,
                variants,
            });
        }
    }

    (checks, enum_columns)
}

/// Parses the check constraints of a table DDL.
///
/// A named constraint keeps the name it was declared with, while an unnamed one
/// receives a deterministic name derived from its table and position, so the
/// same DDL always yields the same catalog and never drifts from itself.
#[cfg(test)]
pub(super) fn parse_sqlite_checks(table: &str, sql: &str, known_columns: &[&str]) -> Vec<CheckDef> {
    scan_table_ddl(table, sql, known_columns).checks
}

/// Scans one table DDL, recovering its enum columns and its check constraints.
///
/// The statement is lexed once, and the two scans that follow read the same token
/// stream, so neither of them repeats the work of the other.
fn scan_table_ddl(table: &str, sql: &str, known_columns: &[&str]) -> ScannedTable {
    let tokens = tokenize(sql);
    let enums = parse_check_enums(&tokens, known_columns);
    let enum_columns: Vec<&str> = enums.iter().map(|(column, _)| column.as_str()).collect();
    let checks = name_checks(table, &tokens, sql, &enum_columns);

    ScannedTable { enums, checks }
}

/// Names the `CHECK (...)` clauses of an already tokenized table DDL.
///
/// A clause closing the value set of an enum column is described by the enum type
/// of that column instead, so it never becomes a check constraint of the catalog.
/// The clause body is the token range the scan already produced, so the enum test
/// never lexes the predicate a second time.
fn name_checks(
    table: &str,
    tokens: &[Token<'_>],
    sql: &str,
    enum_columns: &[&str],
) -> Vec<CheckDef> {
    let mut checks = Vec::new();
    let mut unnamed = 0usize;

    for CheckClause {
        name,
        expression,
        body,
    } in scan_check_clauses(tokens, sql)
    {
        if !enum_columns.is_empty() {
            let clause_body = tokens.get(body).unwrap_or_default();
            if parse_check_clause(clause_body, enum_columns).is_some() {
                continue;
            }
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

/// A `CHECK (...)` clause found in the token stream of a table DDL.
struct CheckClause {
    /// The constraint name, when the clause declares one.
    name: Option<String>,
    /// The trimmed source text of the clause body.
    expression: String,
    /// The token range of the clause body.
    body: Range<usize>,
}

/// Collects every `CHECK ( ... )` clause of a table DDL with its constraint name.
#[cfg(test)]
pub(super) fn scan_check_constraints(sql: &str) -> Vec<(Option<String>, String)> {
    let tokens = tokenize(sql);

    scan_check_clauses(&tokens, sql)
        .into_iter()
        .map(|clause| (clause.name, clause.expression))
        .collect()
}

/// Collects the `CHECK ( ... )` clauses of an already tokenized table DDL.
///
/// The scan runs over the token stream of the shared lexer, so a `CHECK` or a
/// `CONSTRAINT` written inside a literal, a quoted identifier, or a comment is
/// never read as a constraint. A `CONSTRAINT <name>` names a check only when it
/// directly precedes it, so a name attached to a `PRIMARY KEY`, `UNIQUE`,
/// `FOREIGN KEY`, or another table element never leaks onto a later check. A
/// comma ends the current table element and clears a pending name.
fn scan_check_clauses(tokens: &[Token<'_>], sql: &str) -> Vec<CheckClause> {
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
                // Only `CONSTRAINT <name> CHECK (...)` hands its name to a
                // check; a name declaring a key, a foreign key, or any other
                // table element names that element and never a check.
                match (
                    tokens.get(index + 1).and_then(name_of),
                    tokens.get(index + 2).map(|token| token.kind),
                ) {
                    (Some(name), Some(TokenKind::Keyword(SqlKeyword::Check))) => {
                        pending = Some(name);
                        index += 2;
                    }
                    _ => {
                        pending = None;
                        index += 1;
                    }
                }
            }
            TokenKind::Keyword(SqlKeyword::Check) => {
                let open = index + 1;
                if tokens.get(open).map(|token| token.kind) == Some(TokenKind::OpenParen) {
                    if let Some(close) = matching_close(tokens, open) {
                        // The body is read through the checked access, so a span
                        // the scan did not place on a character boundary skips the
                        // clause instead of slicing the statement.
                        match span_text(sql, tokens[open].span.1, tokens[close].span.0) {
                            Some(expression) => results.push(CheckClause {
                                name: pending.take(),
                                expression: expression.trim().to_owned(),
                                body: (open + 1)..close,
                            }),
                            None => pending = None,
                        }
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