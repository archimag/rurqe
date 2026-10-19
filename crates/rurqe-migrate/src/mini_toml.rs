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
//! It is intentionally not a general TOML parser: nested tables, arrays, inline
//! tables, and multi-line strings are out of scope, and unknown syntax is
//! reported with the one-based line number that produced it.

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

/// Splits one `key = value` entry into its key and its unquoted value.
///
/// A quoted key is terminated by its closing quote, so a schema path may itself
/// contain `=` without corrupting the split.
pub fn split_entry(line: &str, number: usize) -> Result<(&str, &str), TomlError> {
    if let Some(rest) = line.strip_prefix('"') {
        let (key, rest) = rest
            .split_once('"')
            .ok_or_else(|| TomlError::new(number, "unterminated quoted key"))?;
        let value = rest
            .trim_start()
            .strip_prefix('=')
            .ok_or_else(|| TomlError::new(number, "expected `=` after the key"))?
            .trim();
        return Ok((key.trim(), unquote(value)));
    }

    let (key, value) = line
        .split_once('=')
        .ok_or_else(|| TomlError::new(number, "expected `key = value` on every entry"))?;
    Ok((key.trim(), unquote(value.trim())))
}

/// Removes one pair of surrounding double quotes, keeping bare values intact.
pub fn unquote(text: &str) -> &str {
    text.strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .unwrap_or(text)
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
        assert_eq!(
            strip_comment("a = \"x\\\"#y\" # tail"),
            "a = \"x\\\"#y\" "
        );
    }

    #[test]
    fn section_headers_are_trimmed_and_validated() {
        assert_eq!(section_name("[checkpoint]", 1), Ok("checkpoint"));
        assert_eq!(section_name("[ checkpoint ]", 1), Ok("checkpoint"));
        assert!(section_name("checkpoint", 2).is_err());
        assert!(section_name("[]", 3).is_err());
    }

    #[test]
    fn entries_split_on_the_first_separator_and_unquote() {
        assert_eq!(split_entry("key = \"value\"", 1), Ok(("key", "value")));
        assert_eq!(split_entry("\"a.b\" = \"Type\"", 1), Ok(("a.b", "Type")));
        assert_eq!(split_entry("bare = value", 1), Ok(("bare", "value")));
        assert!(split_entry("missing separator", 2).is_err());
    }

    #[test]
    fn unquote_keeps_bare_values_intact() {
        assert_eq!(unquote("\"value\""), "value");
        assert_eq!(unquote("value"), "value");
    }
}
