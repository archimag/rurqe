//! Strictly typed SQL lexer and paren balancer for SQLite introspection.
//!
//! Provides lexical analysis for SQLite DDL statements, supporting keywords,
//! identifiers, quoted identifiers in all SQLite quotation styles (`"..."`,
//! `` `...` ``, `[...]`), string literals (`'...'` with `''` escape), numbers,
//! punctuation, line comments (`-- ...\n`), and block comments (`/* ... */`).

#![allow(dead_code)]

use std::fmt;

/// Recognised SQL keywords used during SQLite schema introspection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SqlKeyword {
    Check,
    Constraint,
    In,
    Where,
}

impl SqlKeyword {
    /// Parses a keyword case-insensitively.
    pub fn parse(s: &str) -> Option<Self> {
        if s.eq_ignore_ascii_case("check") {
            Some(Self::Check)
        } else if s.eq_ignore_ascii_case("constraint") {
            Some(Self::Constraint)
        } else if s.eq_ignore_ascii_case("in") {
            Some(Self::In)
        } else if s.eq_ignore_ascii_case("where") {
            Some(Self::Where)
        } else {
            None
        }
    }

    /// Canonical uppercase name of the keyword.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Check => "CHECK",
            Self::Constraint => "CONSTRAINT",
            Self::In => "IN",
            Self::Where => "WHERE",
        }
    }
}

impl fmt::Display for SqlKeyword {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Quotation styles supported by SQLite for identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum QuoteStyle {
    /// ANSI-style `"identifier"` with `""` escape.
    DoubleQuote,
    /// MySQL-style `` `identifier` `` with ```` `` ```` escape.
    Backtick,
    /// MS SQL-style `[identifier]` with `]]` escape.
    SquareBracket,
}

/// Classified kind of a SQL token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TokenKind {
    /// A recognised SQL keyword.
    Keyword(SqlKeyword),
    /// A bare unquoted identifier.
    Ident,
    /// A quoted identifier with its enclosing quote style.
    QuotedIdent(QuoteStyle),
    /// A single-quoted string literal (`'...'`).
    StringLiteral,
    /// An integer or floating-point number.
    Number,
    /// Opening parenthesis `(`.
    OpenParen,
    /// Closing parenthesis `)`.
    CloseParen,
    /// Comma `,`.
    Comma,
    /// Semicolon `;`.
    Semicolon,
    /// Dot `.`.
    Dot,
    /// Any other single-character operator or symbol.
    Other(char),
}

/// A scanned token referencing a slice of the source SQL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token<'a> {
    /// The token kind.
    pub kind: TokenKind,
    /// The exact source text slice.
    pub text: &'a str,
    /// The byte offsets in the source string: `(start, end)`.
    pub span: (usize, usize),
}

impl<'a> Token<'a> {
    /// Creates a new token.
    pub fn new(kind: TokenKind, text: &'a str, span: (usize, usize)) -> Self {
        Self { kind, text, span }
    }

    /// Unescapes an identifier token into its bare name.
    ///
    /// Strips enclosing quotes and unescapes doubled quote characters according
    /// to the token's quote style. For bare identifiers and other tokens,
    /// returns the verbatim text.
    pub fn unescaped_ident(&self) -> String {
        match self.kind {
            TokenKind::QuotedIdent(QuoteStyle::DoubleQuote) => {
                let mut s = self.text;
                if let Some(rest) = s.strip_prefix('"') {
                    s = rest;
                }
                if let Some(rest) = s.strip_suffix('"') {
                    s = rest;
                }
                s.replace("\"\"", "\"")
            }
            TokenKind::QuotedIdent(QuoteStyle::Backtick) => {
                let mut s = self.text;
                if let Some(rest) = s.strip_prefix('`') {
                    s = rest;
                }
                if let Some(rest) = s.strip_suffix('`') {
                    s = rest;
                }
                s.replace("``", "`")
            }
            TokenKind::QuotedIdent(QuoteStyle::SquareBracket) => {
                let mut s = self.text;
                if let Some(rest) = s.strip_prefix('[') {
                    s = rest;
                }
                if let Some(rest) = s.strip_suffix(']') {
                    s = rest;
                }
                s.replace("]]", "]")
            }
            _ => self.text.to_owned(),
        }
    }

    /// Unescapes a string literal token into its content.
    ///
    /// Strips enclosing single quotes and replaces doubled `''` with a single `'`.
    /// For non-string tokens, returns the verbatim text.
    pub fn unescaped_string(&self) -> String {
        match self.kind {
            TokenKind::StringLiteral => {
                let mut s = self.text;
                if let Some(rest) = s.strip_prefix('\'') {
                    s = rest;
                }
                if let Some(rest) = s.strip_suffix('\'') {
                    s = rest;
                }
                s.replace("''", "'")
            }
            _ => self.text.to_owned(),
        }
    }
}

/// Tokenizes a SQL string into a sequence of [`Token`]s.
///
/// Whitespace, single-line comments (`-- ...\n`), and block comments
/// (`/* ... */`) are skipped and not emitted as tokens.
pub fn tokenize(sql: &str) -> Vec<Token<'_>> {
    let mut tokens = Vec::new();
    let mut i = 0;

    while i < sql.len() {
        let rem = &sql[i..];

        // 1. Skip whitespace
        if let Some(ch) = rem.chars().next() {
            if ch.is_whitespace() {
                i += ch.len_utf8();
                continue;
            }
        }

        // 2. Skip line comments: -- ...
        if rem.starts_with("--") {
            i += 2;
            if let Some(pos) = sql[i..].find('\n') {
                i += pos + 1;
            } else {
                i = sql.len();
            }
            continue;
        }

        // 3. Skip block comments: /* ... */
        if rem.starts_with("/*") {
            i += 2;
            if let Some(pos) = sql[i..].find("*/") {
                i += pos + 2;
            } else {
                i = sql.len();
            }
            continue;
        }

        let start = i;
        let ch = match rem.chars().next() {
            Some(c) => c,
            None => break,
        };

        match ch {
            '\'' => {
                i += 1;
                while i < sql.len() {
                    let r = &sql[i..];
                    if r.starts_with("''") {
                        i += 2;
                    } else if r.starts_with('\'') {
                        i += 1;
                        break;
                    } else if let Some(c) = r.chars().next() {
                        i += c.len_utf8();
                    } else {
                        break;
                    }
                }
                tokens.push(Token {
                    kind: TokenKind::StringLiteral,
                    text: &sql[start..i],
                    span: (start, i),
                });
            }
            '"' => {
                i += 1;
                while i < sql.len() {
                    let r = &sql[i..];
                    if r.starts_with("\"\"") {
                        i += 2;
                    } else if r.starts_with('"') {
                        i += 1;
                        break;
                    } else if let Some(c) = r.chars().next() {
                        i += c.len_utf8();
                    } else {
                        break;
                    }
                }
                tokens.push(Token {
                    kind: TokenKind::QuotedIdent(QuoteStyle::DoubleQuote),
                    text: &sql[start..i],
                    span: (start, i),
                });
            }
            '`' => {
                i += 1;
                while i < sql.len() {
                    let r = &sql[i..];
                    if r.starts_with("``") {
                        i += 2;
                    } else if r.starts_with('`') {
                        i += 1;
                        break;
                    } else if let Some(c) = r.chars().next() {
                        i += c.len_utf8();
                    } else {
                        break;
                    }
                }
                tokens.push(Token {
                    kind: TokenKind::QuotedIdent(QuoteStyle::Backtick),
                    text: &sql[start..i],
                    span: (start, i),
                });
            }
            '[' => {
                i += 1;
                while i < sql.len() {
                    let r = &sql[i..];
                    if r.starts_with("]]") {
                        i += 2;
                    } else if r.starts_with(']') {
                        i += 1;
                        break;
                    } else if let Some(c) = r.chars().next() {
                        i += c.len_utf8();
                    } else {
                        break;
                    }
                }
                tokens.push(Token {
                    kind: TokenKind::QuotedIdent(QuoteStyle::SquareBracket),
                    text: &sql[start..i],
                    span: (start, i),
                });
            }
            '(' => {
                i += 1;
                tokens.push(Token {
                    kind: TokenKind::OpenParen,
                    text: &sql[start..i],
                    span: (start, i),
                });
            }
            ')' => {
                i += 1;
                tokens.push(Token {
                    kind: TokenKind::CloseParen,
                    text: &sql[start..i],
                    span: (start, i),
                });
            }
            ',' => {
                i += 1;
                tokens.push(Token {
                    kind: TokenKind::Comma,
                    text: &sql[start..i],
                    span: (start, i),
                });
            }
            ';' => {
                i += 1;
                tokens.push(Token {
                    kind: TokenKind::Semicolon,
                    text: &sql[start..i],
                    span: (start, i),
                });
            }
            '.' => {
                i += 1;
                tokens.push(Token {
                    kind: TokenKind::Dot,
                    text: &sql[start..i],
                    span: (start, i),
                });
            }
            c if c.is_alphabetic() || c == '_' => {
                i += c.len_utf8();
                while i < sql.len() {
                    if let Some(next_ch) = sql[i..].chars().next() {
                        if next_ch.is_alphanumeric() || next_ch == '_' || next_ch == '$' {
                            i += next_ch.len_utf8();
                        } else {
                            break;
                        }
                    } else {
                        break;
                    }
                }
                let text = &sql[start..i];
                let kind = match SqlKeyword::parse(text) {
                    Some(kw) => TokenKind::Keyword(kw),
                    None => TokenKind::Ident,
                };
                tokens.push(Token {
                    kind,
                    text,
                    span: (start, i),
                });
            }
            c if c.is_ascii_digit() => {
                i += 1;
                while i < sql.len() {
                    if let Some(next_ch) = sql[i..].chars().next() {
                        if next_ch.is_ascii_digit() {
                            i += 1;
                        } else {
                            break;
                        }
                    } else {
                        break;
                    }
                }
                if sql
                    .get(i..)
                    .and_then(|rest| rest.strip_prefix('.'))
                    .is_some_and(|after_dot| after_dot.starts_with(|d: char| d.is_ascii_digit()))
                {
                    i += 1;
                    while i < sql.len() {
                        if let Some(next_ch) = sql[i..].chars().next() {
                            if next_ch.is_ascii_digit() {
                                i += 1;
                            } else {
                                break;
                            }
                        } else {
                            break;
                        }
                    }
                }
                tokens.push(Token {
                    kind: TokenKind::Number,
                    text: &sql[start..i],
                    span: (start, i),
                });
            }
            c => {
                i += c.len_utf8();
                tokens.push(Token {
                    kind: TokenKind::Other(c),
                    text: &sql[start..i],
                    span: (start, i),
                });
            }
        }
    }

    tokens
}

/// The token index of the parenthesis closing the one at `open`.
///
/// The scan runs over the token stream of [`tokenize`], so a parenthesis written
/// inside a literal, a quoted identifier, or a comment never affects the depth of
/// the one being closed. An unbalanced stream reports `None`.
pub fn matching_close(tokens: &[Token<'_>], open: usize) -> Option<usize> {
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

/// Extracts the balanced text inside `(...)` starting at `sql[0] == '('`.
///
/// Returns `Some((inner_slice, total_length_consumed))`.
///
/// The extraction delegates to the shared [`matching_close`] scanner over the
/// token stream, so parentheses inside comments, literals, and quoted
/// identifiers do not affect balancing.
pub fn extract_paren_block(sql: &str) -> Option<(&str, usize)> {
    if !sql.starts_with('(') {
        return None;
    }

    let tokens = tokenize(sql);
    let close = matching_close(&tokens, 0)?;
    Some((&sql[1..tokens[close].span.0], tokens[close].span.1))
}

/// Checks whether `sql` (trimmed) is enclosed in a single balanced pair of parentheses.
///
/// Returns `true` only if the opening parenthesis at the start is balanced by the
/// closing parenthesis at the very end of the expression.
pub fn is_enclosed_in_parens(sql: &str) -> bool {
    let s = sql.trim();
    if !s.starts_with('(') || !s.ends_with(')') {
        return false;
    }
    match extract_paren_block(s) {
        Some((_, consumed)) => consumed == s.len(),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenize_keywords_and_identifiers() {
        let sql = "CHECK constraint In WHERE status my_col _leading col$val";
        let tokens = tokenize(sql);

        assert_eq!(tokens.len(), 8);
        assert_eq!(tokens[0].kind, TokenKind::Keyword(SqlKeyword::Check));
        assert_eq!(tokens[0].text, "CHECK");
        assert_eq!(tokens[0].span, (0, 5));

        assert_eq!(tokens[1].kind, TokenKind::Keyword(SqlKeyword::Constraint));
        assert_eq!(tokens[1].text, "constraint");

        assert_eq!(tokens[2].kind, TokenKind::Keyword(SqlKeyword::In));
        assert_eq!(tokens[2].text, "In");

        assert_eq!(tokens[3].kind, TokenKind::Keyword(SqlKeyword::Where));
        assert_eq!(tokens[3].text, "WHERE");

        assert_eq!(tokens[4].kind, TokenKind::Ident);
        assert_eq!(tokens[4].text, "status");

        assert_eq!(tokens[5].kind, TokenKind::Ident);
        assert_eq!(tokens[5].text, "my_col");

        assert_eq!(tokens[6].kind, TokenKind::Ident);
        assert_eq!(tokens[6].text, "_leading");

        assert_eq!(tokens[7].kind, TokenKind::Ident);
        assert_eq!(tokens[7].text, "col$val");
    }

    #[test]
    fn tokenize_quoted_identifiers_and_unescape() {
        let sql = r#""users" "user""name" `table` `tab``le` [order details] [col]]name]"#;
        let tokens = tokenize(sql);

        assert_eq!(tokens.len(), 6);

        assert_eq!(
            tokens[0].kind,
            TokenKind::QuotedIdent(QuoteStyle::DoubleQuote)
        );
        assert_eq!(tokens[0].unescaped_ident(), "users");

        assert_eq!(
            tokens[1].kind,
            TokenKind::QuotedIdent(QuoteStyle::DoubleQuote)
        );
        assert_eq!(tokens[1].unescaped_ident(), "user\"name");

        assert_eq!(tokens[2].kind, TokenKind::QuotedIdent(QuoteStyle::Backtick));
        assert_eq!(tokens[2].unescaped_ident(), "table");

        assert_eq!(tokens[3].kind, TokenKind::QuotedIdent(QuoteStyle::Backtick));
        assert_eq!(tokens[3].unescaped_ident(), "tab`le");

        assert_eq!(
            tokens[4].kind,
            TokenKind::QuotedIdent(QuoteStyle::SquareBracket)
        );
        assert_eq!(tokens[4].unescaped_ident(), "order details");

        assert_eq!(
            tokens[5].kind,
            TokenKind::QuotedIdent(QuoteStyle::SquareBracket)
        );
        assert_eq!(tokens[5].unescaped_ident(), "col]name");
    }

    #[test]
    fn tokenize_string_literals_and_unescape() {
        let sql = "'hello' 'it''s fine' ''";
        let tokens = tokenize(sql);

        assert_eq!(tokens.len(), 3);

        assert_eq!(tokens[0].kind, TokenKind::StringLiteral);
        assert_eq!(tokens[0].unescaped_string(), "hello");

        assert_eq!(tokens[1].kind, TokenKind::StringLiteral);
        assert_eq!(tokens[1].unescaped_string(), "it's fine");

        assert_eq!(tokens[2].kind, TokenKind::StringLiteral);
        assert_eq!(tokens[2].unescaped_string(), "");
    }

    #[test]
    fn tokenize_comments_and_whitespace() {
        let sql = r#"
            -- Single line comment with quotes 'ignored' and (parens)
            SELECT
            /* Block comment
               spanning lines with ) and "quoted" */
            id -- trailing line comment
        "#;
        let tokens = tokenize(sql);

        assert_eq!(tokens.len(), 2);
        assert_eq!(tokens[0].kind, TokenKind::Ident);
        assert_eq!(tokens[0].text, "SELECT");
        assert_eq!(tokens[1].kind, TokenKind::Ident);
        assert_eq!(tokens[1].text, "id");
    }

    #[test]
    fn tokenize_numbers_and_punctuation() {
        let sql = "(123, 45.67); . >=";
        let tokens = tokenize(sql);

        assert_eq!(tokens[0].kind, TokenKind::OpenParen);
        assert_eq!(tokens[1].kind, TokenKind::Number);
        assert_eq!(tokens[1].text, "123");
        assert_eq!(tokens[2].kind, TokenKind::Comma);
        assert_eq!(tokens[3].kind, TokenKind::Number);
        assert_eq!(tokens[3].text, "45.67");
        assert_eq!(tokens[4].kind, TokenKind::CloseParen);
        assert_eq!(tokens[5].kind, TokenKind::Semicolon);
        assert_eq!(tokens[6].kind, TokenKind::Dot);
        assert_eq!(tokens[7].kind, TokenKind::Other('>'));
        assert_eq!(tokens[8].kind, TokenKind::Other('='));
    }

    #[test]
    fn tokenize_a_number_ending_at_a_trailing_dot() {
        let tokens = tokenize("SELECT 1.");
        assert_eq!(tokens.len(), 3);
        assert_eq!(tokens[1].kind, TokenKind::Number);
        assert_eq!(tokens[1].text, "1");
        assert_eq!(tokens[1].span, (7, 8));
        assert_eq!(tokens[2].kind, TokenKind::Dot);
        assert_eq!(tokens[2].span, (8, 9));

        let tokens = tokenize("(1.)");
        assert_eq!(tokens.len(), 4);
        assert_eq!(tokens[1].kind, TokenKind::Number);
        assert_eq!(tokens[1].text, "1");
        assert_eq!(tokens[2].kind, TokenKind::Dot);
        assert_eq!(tokens[3].kind, TokenKind::CloseParen);
    }

    #[test]
    fn extract_paren_block_balanced() {
        assert_eq!(extract_paren_block("not a block"), None);
        assert_eq!(extract_paren_block("(unclosed"), None);
        assert_eq!(extract_paren_block("()"), Some(("", 2)));
        assert_eq!(extract_paren_block("(a, (b, c))"), Some(("a, (b, c)", 11)));
        assert_eq!(
            extract_paren_block("(a, (b, c)), trailing"),
            Some(("a, (b, c)", 11))
        );
    }

    #[test]
    fn extract_paren_block_with_comments_and_quotes() {
        let sql = "(status IN ('val1', 'val2'))";
        assert_eq!(
            extract_paren_block(sql),
            Some(("status IN ('val1', 'val2')", 28))
        );

        let with_block_comment = "(a /* (ignore paren) */ b)";
        assert_eq!(
            extract_paren_block(with_block_comment),
            Some(("a /* (ignore paren) */ b", 26))
        );

        let with_line_comment = "(a -- (ignore paren)\n b)";
        assert_eq!(
            extract_paren_block(with_line_comment),
            Some(("a -- (ignore paren)\n b", 24))
        );

        let with_quote_paren = "(a, 'string with ) paren', b)";
        assert_eq!(
            extract_paren_block(with_quote_paren),
            Some(("a, 'string with ) paren', b", 29))
        );

        let with_ident_paren = r#"(a, "ident with ) paren", b)"#;
        assert_eq!(
            extract_paren_block(with_ident_paren),
            Some((r#"a, "ident with ) paren", b"#, 28))
        );

        let with_bracket_paren = "(a, [bracket with ) paren], b)";
        assert_eq!(
            extract_paren_block(with_bracket_paren),
            Some(("a, [bracket with ) paren], b", 30))
        );
    }

    #[test]
    fn is_enclosed_in_parens_scenarios() {
        assert!(is_enclosed_in_parens("(x)"));
        assert!(is_enclosed_in_parens("((x))"));
        assert!(is_enclosed_in_parens("  (x)  "));
        assert!(is_enclosed_in_parens("('a)')"));
        assert!(is_enclosed_in_parens("(a /* ) */ b)"));
        assert!(is_enclosed_in_parens("(a -- )\n b)"));

        assert!(!is_enclosed_in_parens("(a) AND (b)"));
        assert!(!is_enclosed_in_parens("()()"));
        assert!(!is_enclosed_in_parens(")("));
        assert!(!is_enclosed_in_parens("(("));
        assert!(!is_enclosed_in_parens("x"));
    }

    #[test]
    fn multibyte_utf8_tokenization() {
        let sql = r#"
            CREATE TABLE "пользователи" (
                -- Комментарий с кириллицей: 🚀
                "статус" TEXT NOT NULL,
                CHECK ("статус" IN ('активен', 'заблокирован'))
            );
        "#;
        let tokens = tokenize(sql);
        assert!(!tokens.is_empty());

        let status_quoted = tokens
            .iter()
            .find(|t| t.unescaped_ident() == "статус")
            .expect("find status identifier");
        assert_eq!(
            status_quoted.kind,
            TokenKind::QuotedIdent(QuoteStyle::DoubleQuote)
        );

        let active_lit = tokens
            .iter()
            .find(|t| t.unescaped_string() == "активен")
            .expect("find active literal");
        assert_eq!(active_lit.kind, TokenKind::StringLiteral);
    }

    #[test]
    fn matching_close_balances_parentheses_across_literals_and_comments() {
        let sql = "CHECK ((status) IN ('a)', \"b)\", [c)])) /* ) */";
        let tokens = tokenize(sql);

        assert_eq!(matching_close(&tokens, 0), None);
        assert_eq!(matching_close(&tokens, 1), Some(tokens.len() - 1));
        assert_eq!(matching_close(&tokens, 2), Some(4));
        assert_eq!(matching_close(&tokenize("(unclosed"), 0), None);
    }
}
