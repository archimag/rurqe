//! A deliberately small, line-oriented reader for the flat TOML tables the
//! RURQE toolchain interprets.
//!
//! Two readers consume the same minimal subset of TOML: the codegen overrides of
//! `rurqe.toml` (`[codegen.type_overrides]`) and the `checkpoint.toml` marker
//! (`[checkpoint]`). That subset is flat `[section]` headers, `key = value`
//! entries, optional double quoting, and trailing `#` comments. This module is
//! the single source of truth for the grammar, so the two readers cannot drift
//! apart.
//!
//! Quoted keys and values are decoded into owned strings: the escape sequences
//! `\\`, `\"`, `\n`, `\r`, `\t`, `\b`, `\f` and the `\uXXXX` / `\UXXXXXXXX`
//! unicode forms are expanded, so a value carrying an interior quote or a
//! non-ASCII identifier survives the round trip. Bare (unquoted) values are
//! returned verbatim.
//!
//! It is intentionally not a general TOML parser: nested tables, arrays, inline
//! tables, and multi-line strings are out of scope and rejected, and unknown
//! syntax is reported with the one-based line number that produced it.

/// A malformed line of the interpreted TOML subset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TomlError {
    /// The one-based line number of the offending line.
    pub line: usize,
    /// What is wrong with the line.
    pub message: String,
}

impl TomlError {
    /// Builds an error anchored at `line`.
    pub fn new(line: usize, message: impl Into<String>) -> Self {
        Self {
            line,
            message: message.into(),
        }
    }
}

/// Removes a trailing `#` comment that is not inside a quoted string.
///
/// A backslash-escaped quote (`\"`) inside a quoted string does not close it, so
/// a `#` following such a quote stays part of the value instead of starting a
/// comment.
pub fn strip_comment(line: &str) -> &str {
    let mut quoted = false;
    let mut escaped = false;

    for (index, character) in line.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }

        match character {
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            '#' if !quoted => return &line[..index],
            _ => {}
        }
    }

    line
}

/// Extracts the name of a `[section]` header line.
pub fn section_name(line: &str, number: usize) -> Result<&str, TomlError> {
    let name = line
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .ok_or_else(|| TomlError::new(number, "section headers must be written as `[name]`"))?
        .trim();

    if name.is_empty() {
        return Err(TomlError::new(number, "section headers cannot be empty"));
    }

    Ok(name)
}

/// Splits one `key = value` entry into its decoded key and value.
///
/// Both halves are owned, because unescaping a quoted half cannot borrow from
/// the source line. A quoted key is terminated by its closing quote, so a schema
/// path may itself contain `=` without corrupting the split.
pub fn split_entry(line: &str, number: usize) -> Result<(String, String), TomlError> {
    let (key, value) = split_key(line, number)?;
    Ok((key, value_of(value.trim(), number)?))
}

/// Splits the key off an entry, returning the still encoded value text.
fn split_key(line: &str, number: usize) -> Result<(String, &str), TomlError> {
    if line.starts_with('"') {
        let (key, rest) = scan_quoted(line, number)?;
        let value = rest
            .trim_start()
            .strip_prefix('=')
            .ok_or_else(|| TomlError::new(number, "expected `=` after the key"))?;
        return Ok((key, value));
    }

    let (key, value) = line
        .split_once('=')
        .ok_or_else(|| TomlError::new(number, "expected `key = value` on every entry"))?;
    let key = key.trim();

    if key.is_empty() {
        return Err(TomlError::new(number, "the key cannot be empty"));
    }

    Ok((key.to_owned(), value))
}

/// Decodes the value half of an entry.
fn value_of(text: &str, number: usize) -> Result<String, TomlError> {
    if text.starts_with('"') {
        let (value, rest) = scan_quoted(text, number)?;
        if !rest.trim().is_empty() {
            return Err(TomlError::new(
                number,
                "unexpected text after the quoted value",
            ));
        }
        return Ok(value);
    }

    if text.is_empty() {
        return Err(TomlError::new(number, "the value cannot be empty"));
    }

    Ok(text.to_owned())
}

/// Reads one double-quoted string, returning it decoded with the remainder.
fn scan_quoted(text: &str, number: usize) -> Result<(String, &str), TomlError> {
    if text.starts_with("\"\"\"") || text.starts_with("'''") {
        return Err(TomlError::new(
            number,
            "multi-line strings are not supported",
        ));
    }

    let mut index = match text.strip_prefix('"') {
        Some(_) => 1,
        None => return Err(TomlError::new(number, "expected a quoted string")),
    };
    let mut value = String::new();

    loop {
        let remainder = text
            .get(index..)
            .ok_or_else(|| TomlError::new(number, "unterminated quoted string"))?;
        let character = remainder
            .chars()
            .next()
            .ok_or_else(|| TomlError::new(number, "unterminated quoted string"))?;

        match character {
            '"' => return Ok((value, remainder.get(1..).unwrap_or(""))),
            '\\' => {
                let escaped = remainder
                    .get(1..)
                    .ok_or_else(|| TomlError::new(number, "unterminated escape sequence"))?;
                let (decoded, consumed) = decode_escape(escaped, number)?;
                value.push(decoded);
                index += 1 + consumed;
            }
            _ => {
                value.push(character);
                index += character.len_utf8();
            }
        }
    }
}

/// Decodes the escape sequence following a backslash.
///
/// `text` starts right after the backslash; the returned length counts the bytes
/// consumed from it, excluding the backslash itself.
fn decode_escape(text: &str, number: usize) -> Result<(char, usize), TomlError> {
    let character = text
        .chars()
        .next()
        .ok_or_else(|| TomlError::new(number, "unterminated escape sequence"))?;

    let simple = match character {
        '\\' => Some('\\'),
        '"' => Some('"'),
        'n' => Some('\n'),
        'r' => Some('\r'),
        't' => Some('\t'),
        'b' => Some('\u{8}'),
        'f' => Some('\u{c}'),
        _ => None,
    };

    if let Some(decoded) = simple {
        return Ok((decoded, character.len_utf8()));
    }

    let digits = match character {
        'u' => 4,
        'U' => 8,
        other => {
            return Err(TomlError::new(
                number,
                format!("unknown escape sequence `\\{other}`"),
            ))
        }
    };

    let rest = text.get(character.len_utf8()..).unwrap_or("");
    let hex = rest
        .get(..digits)
        .filter(|hex| hex.chars().all(|digit| digit.is_ascii_hexdigit()))
        .ok_or_else(|| {
            TomlError::new(
                number,
                format!("`\\{character}` must be followed by {digits} hexadecimal digits"),
            )
        })?;
    let code = u32::from_str_radix(hex, 16).map_err(|_| {
        TomlError::new(
            number,
            format!("invalid unicode escape `\\{character}{hex}`"),
        )
    })?;
    let decoded = char::from_u32(code).ok_or_else(|| {
        TomlError::new(
            number,
            format!("`\\{character}{hex}` is not a unicode scalar value"),
        )
    })?;

    Ok((decoded, character.len_utf8() + digits))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comments_are_stripped_outside_quotes() {
        assert_eq!(strip_comment("a = \"b#c\" # tail"), "a = \"b#c\" ");
        assert_eq!(strip_comment("a = 1 # tail"), "a = 1 ");
        assert_eq!(strip_comment("a = \"b\""), "a = \"b\"");
    }

    #[test]
    fn escaped_quotes_do_not_close_a_value() {
        assert_eq!(strip_comment("a = \"x\\\"#y\" # tail"), "a = \"x\\\"#y\" ");
    }

    #[test]
    fn section_headers_are_trimmed_and_validated() {
        assert_eq!(section_name("[checkpoint]", 1), Ok("checkpoint"));
        assert_eq!(section_name("[ checkpoint ]", 1), Ok("checkpoint"));
        assert!(section_name("checkpoint", 2).is_err());
        assert!(section_name("[]", 3).is_err());
    }

    #[test]
    fn entries_split_on_the_first_separator() {
        assert_eq!(
            split_entry("key = \"value\"", 1),
            Ok(("key".into(), "value".into()))
        );
        assert_eq!(
            split_entry("\"a.b\" = \"Type\"", 1),
            Ok(("a.b".into(), "Type".into()))
        );
        assert_eq!(
            split_entry("bare = value", 1),
            Ok(("bare".into(), "value".into()))
        );
        assert_eq!(
            split_entry("\"a=b\" = \"c\"", 1),
            Ok(("a=b".into(), "c".into()))
        );
        assert!(split_entry("missing separator", 2).is_err());
    }

    #[test]
    fn quoted_values_are_decoded() {
        assert_eq!(
            split_entry("k = \"a\\nb\"", 1),
            Ok(("k".into(), "a\nb".into()))
        );
        assert_eq!(
            split_entry("k = \"x\\\"#y\"", 1),
            Ok(("k".into(), "x\"#y".into()))
        );
        assert_eq!(
            split_entry("k = \"\\\\\"", 1),
            Ok(("k".into(), "\\".into()))
        );
        assert_eq!(
            split_entry("k = \"\\u00e9\"", 1),
            Ok(("k".into(), "\u{e9}".into()))
        );
    }

    #[test]
    fn malformed_strings_are_rejected() {
        assert!(split_entry("k = \"unterminated", 3).is_err());
        assert!(split_entry("k = \"bad \\q escape\"", 4).is_err());
        assert!(split_entry("k = \"\\u12\"", 5).is_err());
        assert!(split_entry("k = \"\"\"multi\"\"\"", 6).is_err());
        assert!(split_entry("k = \"value\" trailing", 7).is_err());
        assert!(split_entry("k = ", 8).is_err());
    }
}
