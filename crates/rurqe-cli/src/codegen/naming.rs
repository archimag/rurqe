//! Sanitization of schema names into valid Rust identifiers.

/// Rust keywords, including reserved and reserved-for-future spellings.
const RUST_KEYWORDS: &[&str] = &[
    "abstract", "as", "async", "await", "become", "box", "break", "const", "continue", "crate",
    "do", "dyn", "else", "enum", "extern", "false", "final", "fn", "for", "gen", "if", "impl",
    "in", "let", "loop", "macro", "match", "mod", "move", "mut", "override", "priv", "pub", "ref",
    "return", "self", "Self", "static", "struct", "super", "trait", "true", "try", "type",
    "typeof", "unsafe", "unsized", "use", "virtual", "where", "while", "yield",
];

/// Keywords that cannot be spelled as raw identifiers and are suffixed instead.
const NON_RAW_KEYWORDS: &[&str] = &["crate", "self", "super", "_"];

/// Derives the PascalCase marker type of a table.
pub(super) fn table_type_ident(name: &str) -> String {
    let mut ident = to_pascal_case(name);

    // `Self` is the only capitalized keyword, and it cannot be spelled as a raw
    // identifier, so a table named `self` is suffixed instead.
    if ident == "Self" {
        ident.push('_');
    }

    ident
}

/// Converts an identifier (e.g., `user_profiles`) to PascalCase (`UserProfiles`).
///
/// Characters outside `[A-Za-z0-9]` act as word separators, so `user.profiles`
/// and `user-profiles` both become `UserProfiles`, and a name that starts with a
/// digit is prefixed with `_` to stay a valid Rust type name.
pub fn to_pascal_case(s: &str) -> String {
    let mut out = String::new();
    let mut capitalize = true;

    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            if capitalize {
                out.push(c.to_ascii_uppercase());
                capitalize = false;
            } else {
                out.push(c);
            }
        } else {
            capitalize = true;
        }
    }

    if out.is_empty() {
        "Anonymous".to_owned()
    } else if out.starts_with(|c: char| c.is_ascii_digit()) {
        format!("_{out}")
    } else {
        out
    }
}

/// Converts a column identifier to a safe Rust field identifier.
///
/// Characters outside `[A-Za-z0-9]` become `_` and a leading digit is prefixed
/// with `_`, so every schema name yields a valid identifier. Rust keywords are
/// escaped as raw identifiers (`type` becomes `r#type`), except the handful that
/// cannot be raw identifiers (`crate`, `self`, `super`, `_`), which are
/// suffixed with `_` instead. Every keyword is matched after lowercasing, so
/// `Self` escapes exactly like `self`.
pub fn to_safe_ident(s: &str) -> String {
    let snake = sanitize_snake_case(s);

    if NON_RAW_KEYWORDS.contains(&snake.as_str()) {
        format!("{snake}_")
    } else if RUST_KEYWORDS.contains(&snake.as_str()) {
        format!("r#{snake}")
    } else {
        snake
    }
}

/// Reduces a schema name to lowercase ASCII alphanumerics and `_` separators.
fn sanitize_snake_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len());

    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else {
            out.push('_');
        }
    }

    if out.is_empty() {
        out.push('_');
    }

    if out.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        out.insert(0, '_');
    }

    out
}

/// Escapes a schema name for inclusion in a generated Rust string literal.
///
/// The quote and the backslash are escaped explicitly, and every control
/// character is escaped through [`push_control_escape`]. A name carried over
/// from a database catalog therefore never breaks the generated literal over
/// several lines nor embeds a raw control character into it.
pub(super) fn escape_literal(name: &str) -> String {
    let mut out = String::with_capacity(name.len());

    for c in name.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            other => push_control_escape(&mut out, other),
        }
    }

    out
}

/// Renders a schema name as text that stays inside a generated doc comment.
///
/// A `//`-style comment ends at a line break, so a raw catalog name carrying one
/// would close the comment and start arbitrary source in the generated module.
/// Every control character is therefore spelled with its visible escape, which
/// keeps the rendered comment on a single line while preserving the name the
/// catalog declares:
///
/// ```text
/// /// The `users\npub struct Injected;` table.
/// ```
///
/// The result is only meant for comment text: identifiers go through
/// [`to_safe_ident`] and string literals through [`escape_literal`].
pub(super) fn doc_text(name: &str) -> String {
    let mut out = String::with_capacity(name.len());

    for c in name.chars() {
        push_control_escape(&mut out, c);
    }

    out
}

/// Appends `c` to `out`, spelling a control character with its visible escape.
///
/// The line, carriage return, tab, and NUL characters get their short escapes,
/// and every other control character is escaped through `escape_default`. All
/// remaining characters are copied verbatim.
fn push_control_escape(out: &mut String, c: char) {
    match c {
        '\n' => out.push_str("\\n"),
        '\r' => out.push_str("\\r"),
        '\t' => out.push_str("\\t"),
        '\0' => out.push_str("\\0"),
        other if other.is_control() => out.extend(other.escape_default()),
        other => out.push(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserved_keywords_are_escaped() {
        assert_eq!(to_safe_ident("type"), "r#type");
        assert_eq!(to_safe_ident("match"), "r#match");
        assert_eq!(to_safe_ident("abstract"), "r#abstract");
        assert_eq!(to_safe_ident("final"), "r#final");
        assert_eq!(to_safe_ident("try"), "r#try");
        assert_eq!(to_safe_ident("yield"), "r#yield");
        assert_eq!(to_safe_ident("name"), "name");
    }

    #[test]
    fn keywords_that_cannot_be_raw_identifiers_are_suffixed() {
        assert_eq!(to_safe_ident("self"), "self_");
        assert_eq!(to_safe_ident("Self"), "self_");
        assert_eq!(to_safe_ident("crate"), "crate_");
        assert_eq!(to_safe_ident("super"), "super_");
        assert_eq!(to_safe_ident("_"), "__");
    }

    #[test]
    fn identifiers_are_sanitized_into_valid_rust_names() {
        assert_eq!(to_safe_ident("user.id"), "user_id");
        assert_eq!(to_safe_ident("line\ttotal"), "line_total");
        assert_eq!(to_safe_ident("2fa enabled"), "_2fa_enabled");
        assert_eq!(to_safe_ident("Grüße"), "gr__e");
    }

    #[test]
    fn pascal_casing() {
        assert_eq!(to_pascal_case("user_orders"), "UserOrders");
        assert_eq!(to_pascal_case("users"), "Users");
        assert_eq!(to_pascal_case("user.profiles"), "UserProfiles");
        assert_eq!(to_pascal_case("2fa"), "_2fa");
        assert_eq!(to_pascal_case("self"), "Self");
        // `Self` cannot be spelled as a raw identifier, so a table named `self`
        // has its marker type suffixed instead.
        assert_eq!(table_type_ident("self"), "Self_");
    }

    #[test]
    fn literals_are_escaped() {
        assert_eq!(escape_literal("plain"), "plain");
        assert_eq!(escape_literal("say \"hi\""), "say \\\"hi\\\"");
        assert_eq!(escape_literal("back\\slash"), "back\\\\slash");
    }

    #[test]
    fn control_characters_are_escaped() {
        assert_eq!(escape_literal("line\nbreak"), "line\\nbreak");
        assert_eq!(escape_literal("carriage\rreturn"), "carriage\\rreturn");
        assert_eq!(escape_literal("tab\tseparated"), "tab\\tseparated");
        assert_eq!(escape_literal("nul\0byte"), "nul\\0byte");

        // Any other control character keeps a single-line escape instead of the
        // raw byte it is spelled with.
        assert_eq!(escape_literal("bell\u{7}"), "bell\\u{7}");

        // Ordinary text, including multi-byte characters, is left untouched.
        assert_eq!(escape_literal("Grüße"), "Grüße");
    }

    #[test]
    fn doc_text_keeps_a_name_on_a_single_line() {
        assert_eq!(doc_text("plain"), "plain");
        assert_eq!(doc_text("user.profiles"), "user.profiles");
        assert_eq!(doc_text("line\nbreak"), "line\\nbreak");
        assert_eq!(doc_text("carriage\rreturn"), "carriage\\rreturn");
        assert_eq!(doc_text("tab\tseparated"), "tab\\tseparated");
        assert_eq!(doc_text("nul\0byte"), "nul\\0byte");

        // Any other control character keeps a single-line escape instead of the
        // raw byte it is spelled with.
        assert_eq!(doc_text("bell\u{7}"), "bell\\u{7}");

        // Ordinary text, including multi-byte characters, is left untouched.
        assert_eq!(doc_text("Grüße"), "Grüße");
    }
}
