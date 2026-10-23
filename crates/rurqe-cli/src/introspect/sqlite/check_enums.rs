//! Parsing of the `CHECK (<column> IN (...))` constraints SQLite uses for enums.
//!
//! SQLite has no `CREATE TYPE ... AS ENUM`, so a schema closes the set of values
//! of a column with a table constraint instead. The parser walks the table DDL
//! byte-wise and skips string literals and quoted identifiers, so a `CHECK`
//! appearing inside a value is never mistaken for a constraint, and it recovers
//! one `(column, variants)` pair per column.

use std::collections::BTreeSet;

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

        if i + 5 <= bytes.len() && bytes[i..i + 5].eq_ignore_ascii_case(b"check") {
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
                if paren_idx < bytes.len()
                    && bytes[paren_idx] == b'('
                    && sql.is_char_boundary(paren_idx)
                {
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

    if rest.len() < 2 || !rest.as_bytes()[..2].eq_ignore_ascii_case(b"in") {
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
}
