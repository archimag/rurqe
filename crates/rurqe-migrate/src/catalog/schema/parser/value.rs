//! Shared quoted-value, identifier-list, and type-text readers.

use crate::catalog::types::TypeKind;
use crate::error::Error;

/// Creates an [`Error::InvalidLockFormat`] from the given message.
pub(super) fn lock_error(message: impl Into<String>) -> Error {
    Error::InvalidLockFormat {
        message: message.into(),
    }
}

/// Parses a quoted value and rejects trailing text.
pub(super) fn parse_quoted_value(input: &str) -> Result<String, Error> {
    let (value, rest) = parse_quoted_prefix(input)?;
    if !rest.trim().is_empty() {
        return Err(lock_error("unexpected text after a quoted value"));
    }
    Ok(value)
}

/// Parses the `["a", "b"]` label list of an enum section.
pub(super) fn parse_variant_list(input: &str) -> Result<Vec<String>, Error> {
    let mut rest = input
        .strip_prefix('[')
        .ok_or_else(|| lock_error("expected `[` to start an enum label list"))?
        .trim_start();

    let mut variants = Vec::new();
    loop {
        if let Some(empty) = rest.strip_prefix(']') {
            if !empty.trim().is_empty() {
                return Err(lock_error("unexpected text after an enum label list"));
            }
            return Ok(variants);
        }

        let (variant, after) = parse_quoted_prefix(rest)?;
        variants.push(variant);

        let after = after.trim_start();
        match after.strip_prefix(',') {
            Some(remainder) => rest = remainder.trim_start(),
            None => {
                let trailing = after
                    .strip_prefix(']')
                    .ok_or_else(|| lock_error("expected `,` or `]` after an enum label"))?;
                if !trailing.trim().is_empty() {
                    return Err(lock_error("unexpected text after an enum label list"));
                }
                return Ok(variants);
            }
        }
    }
}

/// Parses a leading `"..."` identifier, decoding doubled quotes.
pub(super) fn parse_quoted_prefix(input: &str) -> Result<(String, &str), Error> {
    let mut result = String::new();
    let mut chars = input.char_indices().peekable();
    match chars.next() {
        Some((_, '"')) => {}
        _ => return Err(lock_error("expected a double-quoted identifier")),
    }
    while let Some((index, character)) = chars.next() {
        if character == '"' {
            if let Some(&(_, '"')) = chars.peek() {
                chars.next();
                result.push('"');
            } else {
                return Ok((result, &input[index + 1..]));
            }
        } else {
            result.push(character);
        }
    }
    Err(lock_error("unterminated quoted identifier"))
}

/// Parses a `("a", "b")` list of quoted identifiers.
pub(super) fn parse_quoted_list(input: &str) -> Result<(Vec<String>, &str), Error> {
    let mut rest = input
        .strip_prefix('(')
        .ok_or_else(|| lock_error("expected `(` to start an identifier list"))?;
    let mut items = Vec::new();
    let trimmed = rest.trim_start();
    if let Some(after_close) = trimmed.strip_prefix(')') {
        return Ok((items, after_close));
    }
    loop {
        let (name, after) = parse_quoted_prefix(rest)?;
        items.push(name);
        if let Some(after_comma) = after.strip_prefix(", ") {
            rest = after_comma;
        } else {
            rest = after;
            break;
        }
    }
    let rest = rest
        .strip_prefix(')')
        .ok_or_else(|| lock_error("expected `)` to close an identifier list"))?;
    Ok((items, rest))
}

/// Parses the canonical type text of a column back into a [`TypeKind`].
pub(super) fn parse_type_kind(text: &str) -> Result<TypeKind, Error> {
    let text = text.trim();

    // An array renders its element type followed by `[]`, while the name of an
    // enum may end with the very same characters, so the suffix only marks an
    // array when it does not leave a quoted name open.
    if let Some(element) = text.strip_suffix("[]") {
        if !has_open_quote(element) {
            return Ok(TypeKind::Array(Box::new(parse_type_kind(element)?)));
        }
    }

    if let Some(rest) = text.strip_prefix("enum ") {
        return Ok(TypeKind::Enum(parse_quoted_value(rest.trim_start())?));
    }

    Ok(match text {
        "boolean" => TypeKind::Boolean,
        "int32" => TypeKind::Int32,
        "int64" => TypeKind::Int64,
        "float64" => TypeKind::Float64,
        "numeric" => TypeKind::Numeric,
        "text" => TypeKind::Text,
        "bytes" => TypeKind::Bytes,
        "uuid" => TypeKind::Uuid,
        "json" => TypeKind::Json,
        "timestamp" => TypeKind::Timestamp,
        "date" => TypeKind::Date,
        other => TypeKind::Other(other.to_owned()),
    })
}

/// Reports whether `text` leaves a double quote open.
///
/// A doubled quote inside a name counts as two, so a complete name always has an
/// even number of quotes.
fn has_open_quote(text: &str) -> bool {
    text.chars().filter(|character| *character == '"').count() % 2 == 1
}

/// Parses a leading quoted, escaped string prefix.
pub(super) fn parse_escaped_quoted_prefix(input: &str) -> Result<(String, &str), Error> {
    let mut characters = input.char_indices();

    match characters.next() {
        Some((_, '"')) => {}
        _ => return Err(lock_error("expected a double-quoted value")),
    }

    let mut value = String::new();
    let mut closing = None;

    while let Some((index, character)) = characters.next() {
        match character {
            '"' => {
                if let Some((_, '"')) = characters.clone().next() {
                    characters.next();
                    value.push('"');
                } else {
                    closing = Some(index);
                    break;
                }
            }
            '\\' => match characters.next() {
                Some((_, 'n')) => value.push('\n'),
                Some((_, 'r')) => value.push('\r'),
                Some((_, '\\')) => value.push('\\'),
                Some((_, other)) => {
                    return Err(lock_error(format!(
                        "the escape `\\{other}` is not recognised in a quoted value"
                    )))
                }
                None => return Err(lock_error("a quoted value ends inside an escape")),
            },
            other => value.push(other),
        }
    }

    let closing = closing.ok_or_else(|| lock_error("unterminated quoted value"))?;
    Ok((value, &input[closing + 1..]))
}

/// Parses a quoted, escaped default value written by a current snapshot.
pub(super) fn parse_quoted_default(input: &str) -> Result<String, Error> {
    let (value, rest) = parse_escaped_quoted_prefix(input)?;
    if !rest.trim().is_empty() {
        return Err(lock_error("unexpected text after a quoted default value"));
    }
    Ok(value)
}
