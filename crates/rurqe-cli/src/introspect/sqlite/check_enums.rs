//! Parsing of the `CHECK (<column> IN (...))` constraints SQLite uses for enums.
//!
//! SQLite has no `CREATE TYPE ... AS ENUM`, so a schema closes the set of values
//! of a column with a table constraint instead. The parser walks the token stream
//! of the shared lexer, so a `CHECK` written inside a value or a comment is never
//! mistaken for a constraint, and it recovers one `(column, variants)` pair per
//! column.

use std::collections::BTreeSet;

use super::lexer::{tokenize, SqlKeyword, Token, TokenKind};

/// Parses deterministic `CHECK (<col> IN ('val1', 'val2', ...))` enum constraints
/// from the table DDL `sql`.
pub fn parse_sqlite_check_enums(sql: &str, known_columns: &[&str]) -> Vec<(String, Vec<String>)> {
    let tokens = tokenize(sql);
    let mut results = Vec::new();
    let mut seen = BTreeSet::new();
    let mut index = 0;

    while let Some(token) = tokens.get(index) {
        if !matches!(token.kind, TokenKind::Keyword(SqlKeyword::Check)) {
            index += 1;
            continue;
        }

        let open = index + 1;
        if !matches!(
            tokens.get(open).map(|token| token.kind),
            Some(TokenKind::OpenParen)
        ) {
            index += 1;
            continue;
        }

        let Some(close) = matching_close(&tokens, open) else {
            index += 1;
            continue;
        };

        if let Some((column, variants)) =
            parse_check_clause(&tokens[open + 1..close], known_columns)
        {
            if seen.insert(column.clone()) {
                results.push((column, variants));
            }
        }

        index = close + 1;
    }

    results
}

/// Parses `<col> IN ('val1', 'val2', ...)` from the tokens of a CHECK clause.
fn parse_check_clause(
    tokens: &[Token<'_>],
    known_columns: &[&str],
) -> Option<(String, Vec<String>)> {
    let (start, end) = trim_parens(tokens, 0, tokens.len());

    if matches!(
        tokens.get(start).map(|token| token.kind),
        Some(TokenKind::OpenParen)
    ) {
        let close = matching_close(tokens, start)?;
        let (column, next) = parse_column_identifier(tokens, start + 1)?;
        if next != close {
            return None;
        }
        return parse_in_list(tokens, close + 1, end, column, known_columns);
    }

    let (column, next) = parse_column_identifier(tokens, start)?;
    parse_in_list(tokens, next, end, column, known_columns)
}

/// Reads the column identifier at `index` and reports the following position.
fn parse_column_identifier(tokens: &[Token<'_>], index: usize) -> Option<(String, usize)> {
    let token = tokens.get(index)?;
    match token.kind {
        TokenKind::Ident | TokenKind::QuotedIdent(_) => Some((token.unescaped_ident(), index + 1)),
        _ => None,
    }
}

/// Reads the `IN (<literal>, ...)` tail of a check predicate.
fn parse_in_list(
    tokens: &[Token<'_>],
    start: usize,
    end: usize,
    column: String,
    known_columns: &[&str],
) -> Option<(String, Vec<String>)> {
    if start >= end
        || !matches!(
            tokens.get(start).map(|token| token.kind),
            Some(TokenKind::Keyword(SqlKeyword::In))
        )
    {
        return None;
    }

    let open = start + 1;
    if open >= end
        || !matches!(
            tokens.get(open).map(|token| token.kind),
            Some(TokenKind::OpenParen)
        )
    {
        return None;
    }

    let close = matching_close(tokens, open)?;
    if close + 1 != end {
        return None;
    }

    let variants = parse_variant_list(tokens, open + 1, close)?;
    let matched = known_columns
        .iter()
        .find(|candidate| candidate.eq_ignore_ascii_case(&column))?;

    Some(((*matched).to_owned(), variants))
}

/// Reads the single-quoted literals of an `IN (...)` list.
fn parse_variant_list(tokens: &[Token<'_>], start: usize, end: usize) -> Option<Vec<String>> {
    let mut variants: Vec<String> = Vec::new();
    let mut index = start;

    loop {
        if index >= end {
            // A trailing comma is accepted, an empty list is not.
            return if variants.is_empty() {
                None
            } else {
                Some(variants)
            };
        }

        let token = tokens.get(index)?;
        if token.kind != TokenKind::StringLiteral {
            return None;
        }
        variants.push(token.unescaped_string());
        index += 1;

        if index >= end {
            return Some(variants);
        }

        match tokens.get(index).map(|token| token.kind) {
            Some(TokenKind::Comma) => index += 1,
            _ => return None,
        }
    }
}

/// Strips the parenthesis pairs wrapping the whole `start..end` range.
fn trim_parens(tokens: &[Token<'_>], mut start: usize, mut end: usize) -> (usize, usize) {
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

/// The index of the parenthesis closing the one at `open`.
fn matching_close(tokens: &[Token<'_>], open: usize) -> Option<usize> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_enum_parser_handles_multibyte_utf8() {
        let sql = r#"
            CREATE TABLE "пользователи" (
                "ид" INTEGER PRIMARY KEY,
                -- Комментарий с кириллицей и спецсимволами: ©, ®, €, 🚀
                "описание" TEXT DEFAULT 'значение с кириллицей',
                "статус" TEXT NOT NULL,
                CHECK ("статус" IN ('активен', 'заблокирован'))
            );
        "#;
        let enums = parse_sqlite_check_enums(sql, &["ид", "описание", "статус"]);
        assert_eq!(
            enums,
            vec![(
                "статус".to_owned(),
                vec!["активен".to_owned(), "заблокирован".to_owned()]
            )]
        );
    }

    #[test]
    fn a_check_appearing_inside_a_literal_is_not_a_constraint() {
        let sql = "CREATE TABLE notes (body TEXT DEFAULT 'CHECK (state IN (''a''))');";

        assert_eq!(parse_sqlite_check_enums(sql, &["body"]), Vec::new());
    }

    #[test]
    fn a_check_inside_a_line_comment_is_ignored() {
        let sql = "CREATE TABLE tasks (\n\
                     state TEXT NOT NULL -- CHECK (state IN ('hidden'))\n\
                   );";

        assert_eq!(parse_sqlite_check_enums(sql, &["state"]), Vec::new());
    }

    #[test]
    fn a_check_inside_a_block_comment_is_ignored() {
        let sql = "CREATE TABLE tasks (\n\
                     state TEXT NOT NULL,\n\
                     /* CHECK (state IN ('hidden', 'lost')) */\n\
                     note TEXT\n\
                   );";

        assert_eq!(parse_sqlite_check_enums(sql, &["state"]), Vec::new());
    }

    #[test]
    fn a_stray_closing_parenthesis_never_breaks_the_scan() {
        let sql = "CREATE TABLE tasks (state TEXT, CHECK (state IN ('new', 'done'))));";

        assert_eq!(
            parse_sqlite_check_enums(sql, &["state"]),
            vec![(
                "state".to_owned(),
                vec!["new".to_owned(), "done".to_owned()]
            )]
        );
    }

    #[test]
    fn an_unclosed_parenthesis_yields_no_constraint() {
        let sql = "CREATE TABLE tasks (state TEXT, CHECK (state IN ('new', 'done')";

        assert_eq!(parse_sqlite_check_enums(sql, &["state"]), Vec::new());
    }

    #[test]
    fn a_parenthesis_inside_a_literal_never_closes_the_block() {
        let sql = "CREATE TABLE tasks (state TEXT, CHECK (state IN ('a)', 'b')));";

        assert_eq!(
            parse_sqlite_check_enums(sql, &["state"]),
            vec![(
                "state".to_owned(),
                vec!["a)".to_owned(), "b".to_owned()]
            )]
        );
    }
}
