//! Project configuration for the code generation toolchain.
//!
//! `cargo db-codegen` reads the optional `[codegen]` table and its
//! `[codegen.type_overrides]` sub-table from the `rurqe.toml` file in the working
//! directory. Every override entry maps a
//! `table.column` schema path onto the Rust type generated for that column, so
//! domain newtypes (`uuid::Uuid`, `chrono::NaiveDateTime`, money wrappers)
//! survive code generation instead of degrading to the engine-neutral fallback.
//!
//! ```toml
//! [codegen.type_overrides]
//! "users.id" = "uuid::Uuid"
//! users.created_at = "chrono::NaiveDateTime"
//! ```
//!
//! The document is read through its parsed representation rather than through a
//! derived shape, because every rejected entry has to be reported on the line it
//! occupies. `toml_edit` locates a malformed document through the span its parser
//! records, but it does not expose a comparable span for a value it has already
//! accepted, so the entries themselves are located through a declaration index
//! built once from the source text: the index maps the key path each line declares
//! onto that line, which finds the flat (`"users.id" = "..."`), dotted
//! (`users.id = ...`) and table (`[codegen.type_overrides.users]`) spellings of an
//! entry without ever matching its text against unrelated lines.

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

use toml_edit::{DocumentMut, TableLike};

/// The configuration file read by default.
pub const DEFAULT_CONFIG_PATH: &str = "rurqe.toml";

/// The section holding the code generation settings.
const CODEGEN_SECTION: &str = "codegen";

/// The key enabling domain types in the generated schema.
const DOMAIN_TYPES_KEY: &str = "domain_types";

/// The key declaring the per-column Rust type overrides.
const TYPE_OVERRIDES_KEY: &str = "type_overrides";

/// The components of an entry path, the section headers included.
type EntryPath = Vec<String>;

/// The codegen configuration of a project.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CodegenConfig {
    /// Rust types keyed by their `table.column` schema path.
    type_overrides: BTreeMap<String, String>,
    /// Whether domain types (`Uuid`, `Json`, `Timestamp`, `Date`) are enabled.
    domain_types: bool,
}

impl CodegenConfig {
    /// Reads the configuration file at `path`, defaulting when it is absent.
    ///
    /// A missing file means "no overrides", while an unreadable or malformed
    /// file is an error: silently ignoring a typo would generate wrong types.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        match std::fs::read_to_string(path) {
            Ok(contents) => Self::parse(&contents),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(ConfigError::Io {
                path: path.display().to_string(),
                message: error.to_string(),
            }),
        }
    }

    /// Parses the `[codegen]` and `[codegen.type_overrides]` tables out of `contents`.
    ///
    /// Both spellings of an override are read: the quoted `"users.id" = "..."` form
    /// declares one path directly, and the `users.id = "..."` (or
    /// `[codegen.type_overrides.users]`) form nests the column one level deeper.
    /// Every rejected entry is reported at its own line, and the two spellings of
    /// one path collide while flattening instead of silently overwriting each
    /// other.
    pub fn parse(contents: &str) -> Result<Self, ConfigError> {
        let document = contents
            .parse::<DocumentMut>()
            .map_err(|error| ConfigError::Syntax {
                line: error
                    .span()
                    .map_or(1, |span| line_of(contents, span.start)),
                message: format!("malformed TOML section or entry: {}", error.message()),
            })?;

        let index = DeclarationIndex::of(contents);
        let mut config = Self::default();

        for (section, item) in document.iter() {
            let path = vec![section.to_owned()];

            if section != CODEGEN_SECTION {
                return Err(declaration_error(
                    &index,
                    &path,
                    format!("unknown configuration section `{section}`"),
                ));
            }

            let table = item.as_table_like().ok_or_else(|| {
                declaration_error(
                    &index,
                    &path,
                    format!("`{CODEGEN_SECTION}` must be a table"),
                )
            })?;
            config.read_section(&index, table, &path)?;
        }

        Ok(config)
    }

    /// Reads the keys of the `[codegen]` table.
    fn read_section(
        &mut self,
        index: &DeclarationIndex,
        table: &dyn TableLike,
        section: &[String],
    ) -> Result<(), ConfigError> {
        for (key, item) in table.iter() {
            let mut path = section.to_vec();
            path.push(key.to_owned());

            match key {
                DOMAIN_TYPES_KEY => {
                    self.domain_types = item.as_bool().ok_or_else(|| {
                        declaration_error(
                            index,
                            &path,
                            format!("`{CODEGEN_SECTION}.{DOMAIN_TYPES_KEY}` must be a boolean"),
                        )
                    })?;
                }
                TYPE_OVERRIDES_KEY => {
                    let overrides = item.as_table_like().ok_or_else(|| {
                        declaration_error(
                            index,
                            &path,
                            format!("`{CODEGEN_SECTION}.{TYPE_OVERRIDES_KEY}` must be a table"),
                        )
                    })?;
                    self.read_overrides(index, overrides, &path)?;
                }
                other => {
                    return Err(declaration_error(
                        index,
                        &path,
                        format!("unknown key `{other}` in the `{CODEGEN_SECTION}` section"),
                    ));
                }
            }
        }

        Ok(())
    }

    /// Flattens the `[codegen.type_overrides]` table onto `table.column` paths.
    fn read_overrides(
        &mut self,
        index: &DeclarationIndex,
        table: &dyn TableLike,
        section: &[String],
    ) -> Result<(), ConfigError> {
        for (key, item) in table.iter() {
            let mut path = section.to_vec();
            path.push(key.to_owned());

            if let Some(rust_type) = item.as_str() {
                require_schema_path(index, &path, key)?;
                self.insert_override(index, &path, key.to_owned(), rust_type)?;
            } else if let Some(columns) = item.as_table_like() {
                for (column, value) in columns.iter() {
                    let mut entry = path.clone();
                    entry.push(column.to_owned());

                    let rust_type = value.as_str().ok_or_else(|| {
                        declaration_error(
                            index,
                            &entry,
                            format!("`{}` must be a Rust type string", display_path(&entry)),
                        )
                    })?;
                    let schema_path = format!("{key}.{column}");
                    self.insert_override(index, &entry, schema_path, rust_type)?;
                }
            } else {
                return Err(declaration_error(
                    index,
                    &path,
                    format!(
                        "`{}` must be a Rust type string or a table of column overrides",
                        display_path(&path)
                    ),
                ));
            }
        }

        Ok(())
    }

    /// Records one override, rejecting an empty type or a repeated path.
    fn insert_override(
        &mut self,
        index: &DeclarationIndex,
        path: &[String],
        schema_path: String,
        rust_type: &str,
    ) -> Result<(), ConfigError> {
        if rust_type.trim().is_empty() {
            return Err(declaration_error(
                index,
                path,
                "the Rust type cannot be empty".to_owned(),
            ));
        }

        // The quoted (`"users.id"`) and the dotted (`users.id`) spelling of one
        // path are distinct TOML keys, so the collision between the two only
        // surfaces here: TOML itself rejects a path repeated in the very same
        // spelling, before the document reaches this point.
        if self
            .type_overrides
            .insert(schema_path.clone(), rust_type.to_owned())
            .is_some()
        {
            return Err(declaration_error(
                index,
                path,
                format!("duplicate type override `{schema_path}`"),
            ));
        }

        Ok(())
    }

    /// Returns the Rust type declared for `table`.`column`, if any.
    pub fn type_override(&self, table: &str, column: &str) -> Option<&str> {
        self.type_overrides
            .get(&format!("{table}.{column}"))
            .map(String::as_str)
    }

    /// Returns whether domain types are enabled in configuration.
    pub fn domain_types(&self) -> bool {
        self.domain_types
    }

    /// Returns the declared overrides keyed by their `table.column` path.
    pub fn type_overrides(&self) -> &BTreeMap<String, String> {
        &self.type_overrides
    }

    /// Consumes the configuration, yielding the overrides for code generation.
    pub fn into_type_overrides(self) -> BTreeMap<String, String> {
        self.type_overrides
    }
}

/// Error raised while reading the project configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    /// The configuration file exists but could not be read.
    Io {
        /// The path that was read.
        path: String,
        /// The underlying operating-system error.
        message: String,
    },
    /// A line of the configuration is malformed.
    Syntax {
        /// The one-based line number of the offending line.
        line: usize,
        /// What is wrong with the line.
        message: String,
    },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::Io { path, message } => {
                write!(f, "failed to read configuration `{path}`: {message}")
            }
            ConfigError::Syntax { line, message } => {
                write!(f, "invalid configuration at line {line}: {message}")
            }
        }
    }
}

impl std::error::Error for ConfigError {}

/// Maps the entry paths a configuration document declares onto their lines.
///
/// The index is filled from the source text only after `toml_edit` accepted it, so
/// it never judges whether a line is valid TOML: it records the key path each line
/// declares, one entry per path with the first declaration winning. Paths are
/// compared component by component, so the quoted `"users.id"` (one component) and
/// the dotted `users.id` (two components) remain distinct declarations even though
/// both name the same column.
#[derive(Debug, Default)]
struct DeclarationIndex {
    lines: BTreeMap<EntryPath, usize>,
}

impl DeclarationIndex {
    /// Indexes the declarations of `contents`.
    fn of(contents: &str) -> Self {
        let mut index = Self::default();
        let mut section: EntryPath = Vec::new();

        for (offset, line) in contents.lines().enumerate() {
            let text = strip_comment(line).trim();
            if text.is_empty() {
                continue;
            }

            if let Some(header) = text.strip_prefix('[') {
                let inner = header
                    .trim_start_matches('[')
                    .trim_end_matches(']')
                    .trim();
                section = split_key_path(inner).unwrap_or_default();
                index.lines.entry(section.clone()).or_insert(offset + 1);
                continue;
            }

            let Some(key) = key_expression(text) else {
                continue;
            };
            let Some(components) = split_key_path(key) else {
                continue;
            };

            let mut path = section.clone();
            path.extend(components);
            index.lines.entry(path).or_insert(offset + 1);
        }

        index
    }

    /// Returns the one-based line declaring `path`, when the document declares it.
    fn line_of(&self, path: &[String]) -> Option<usize> {
        self.lines.get(path).copied()
    }
}

/// Builds a [`ConfigError::Syntax`] located at the line declaring `path`.
fn declaration_error(index: &DeclarationIndex, path: &[String], message: String) -> ConfigError {
    ConfigError::Syntax {
        line: index.line_of(path).unwrap_or(1),
        message,
    }
}

/// Renders an entry path the way the configuration spells it.
fn display_path(path: &[String]) -> String {
    path.join(".")
}

/// Accepts only an explicit `table.column` path on the flat spelling of an entry.
fn require_schema_path(
    index: &DeclarationIndex,
    path: &[String],
    key: &str,
) -> Result<(), ConfigError> {
    if !key.contains('.') || key.starts_with('.') || key.ends_with('.') {
        return Err(declaration_error(
            index,
            path,
            format!("expected a `table.column` schema path, got `{key}`"),
        ));
    }

    Ok(())
}

/// The one-based line containing the byte at `offset`, defaulting to the first.
///
/// The prefix is sliced through [`str::get`] so an offset that is not a character
/// boundary — or one past the end of the file — falls back to line one instead of
/// panicking, and the line counts the newlines the prefix contains.
fn line_of(contents: &str, offset: usize) -> usize {
    contents
        .get(..offset)
        .map_or(1, |prefix| prefix.matches('\n').count() + 1)
}

/// Strips the trailing comment of `line`, ignoring a `#` inside a quoted value.
fn strip_comment(line: &str) -> &str {
    match find_unquoted(line, |character| character == '#') {
        Some(index) => line.get(..index).unwrap_or(line),
        None => line,
    }
}

/// The key expression preceding the first unquoted `=` of `text`.
fn key_expression(text: &str) -> Option<&str> {
    let index = find_unquoted(text, |character| character == '=')?;
    text.get(..index)
}

/// Splits a key expression into the components it names.
///
/// Each unquoted dot separates two components and the quotes around a component are
/// removed, so `"users.id"` stays one component while `users.id` names two.
fn split_key_path(expression: &str) -> Option<EntryPath> {
    let mut components = Vec::new();
    let mut rest = expression;

    loop {
        let (segment, remainder) = match find_unquoted(rest, |character| character == '.') {
            // A dot is a single ASCII byte, so the remainder starts right behind it.
            Some(index) => (rest.get(..index)?, Some(rest.get(index + 1..)?)),
            None => (rest, None),
        };

        components.push(unquote_key(segment.trim())?);

        match remainder {
            Some(remainder) => rest = remainder,
            None => return Some(components),
        }
    }
}

/// Removes the quotes around one key component.
///
/// Escape sequences are not decoded, because a configuration path never needs one;
/// a component whose quotes do not pair up is rejected so that an unreadable entry
/// stays unindexed instead of being matched against the wrong text.
fn unquote_key(component: &str) -> Option<String> {
    for quote in ['"', '\''] {
        if let Some(inner) = component
            .strip_prefix(quote)
            .and_then(|rest| rest.strip_suffix(quote))
        {
            return Some(inner.to_owned());
        }
    }

    if component.contains('"') || component.contains('\'') {
        return None;
    }

    Some(component.to_owned())
}

/// The byte index of the first `predicate` character outside quotes.
///
/// A basic string (`"..."`) honours backslash escapes while a literal string
/// (`'...'`) does not, matching the quoting rules of the TOML the parser accepted.
fn find_unquoted(text: &str, predicate: impl Fn(char) -> bool) -> Option<usize> {
    let mut quote: Option<char> = None;
    let mut escaped = false;

    for (index, character) in text.char_indices() {
        match quote {
            Some(active) => {
                if escaped {
                    escaped = false;
                } else if character == '\\' && active == '"' {
                    escaped = true;
                } else if character == active {
                    quote = None;
                }
            }
            None => {
                if predicate(character) {
                    return Some(index);
                }
                if character == '"' || character == '\'' {
                    quote = Some(character);
                }
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoted_and_bare_entries_are_read() {
        let config = CodegenConfig::parse(
            "[codegen.type_overrides]\n\
             \"users.id\" = \"uuid::Uuid\"\n\
             users.created_at = \"chrono::NaiveDateTime\"\n",
        )
        .expect("the table is valid");

        assert_eq!(config.type_override("users", "id"), Some("uuid::Uuid"));
        assert_eq!(
            config.type_override("users", "created_at"),
            Some("chrono::NaiveDateTime")
        );
        assert_eq!(config.type_override("users", "email"), None);
    }

    #[test]
    fn unknown_sections_are_rejected() {
        let err = CodegenConfig::parse(
            "# project configuration\n\
             [workspace]\n\
             members = [\"a\", \"b\"]\n\
             \n\
             [codegen.type_overrides]\n\
             \"users.id\" = \"uuid::Uuid\"\n",
        )
        .expect_err("unknown sections are rejected");

        assert!(err.to_string().contains("workspace"));
    }

    #[test]
    fn unknown_codegen_keys_are_rejected() {
        let err = CodegenConfig::parse("[codegen]\ndomain_type = true\n")
            .expect_err("misspelled key rejected");
        assert!(err.to_string().contains("domain_type"));
    }

    #[test]
    fn domain_types_in_codegen_section() {
        let config = CodegenConfig::parse("[codegen]\ndomain_types = true\n")
            .expect("codegen section parsed");
        assert!(config.domain_types());

        let config_false = CodegenConfig::parse("[codegen]\ndomain_types = false\n")
            .expect("codegen section parsed");
        assert!(!config_false.domain_types());
    }

    #[test]
    fn domain_types_are_off_by_default() {
        let config = CodegenConfig::parse("").expect("an empty file is valid");
        assert!(!config.domain_types());

        let config = CodegenConfig::parse("[codegen.type_overrides]\n")
            .expect("an empty overrides table is valid");
        assert!(!config.domain_types());
    }

    #[test]
    fn invalid_domain_types_is_rejected() {
        let err = CodegenConfig::parse("[codegen]\ndomain_types = \"not_a_bool\"\n")
            .expect_err("non-boolean rejected");
        assert!(err.to_string().contains("domain_types") || err.to_string().contains("invalid type"));
    }

    #[test]
    fn malformed_entries_are_reported_with_their_line() {
        let missing_separator =
            CodegenConfig::parse("[codegen.type_overrides]\n\"users.id\" \"uuid::Uuid\"\n")
                .expect_err("a missing `=` is rejected");
        assert!(matches!(
            missing_separator,
            ConfigError::Syntax { line: 2, .. }
        ));

        let empty_type = CodegenConfig::parse("[codegen.type_overrides]\n\"users.id\" = \"\"\n")
            .expect_err("an empty type is rejected");
        assert!(empty_type.to_string().contains("cannot be empty"));

        let unqualified = CodegenConfig::parse("[codegen.type_overrides]\nid = \"uuid::Uuid\"\n")
            .expect_err("an unqualified path is rejected");
        assert!(unqualified.to_string().contains("table.column"));

        let duplicate = CodegenConfig::parse(
            "[codegen.type_overrides]\n\"users.id\" = \"a::Type\"\n\"users.id\" = \"b::Type\"\n",
        )
        .expect_err("repeated keys are rejected");
        assert!(duplicate.to_string().contains("duplicate") || duplicate.to_string().contains("more than once"));

        let bad_section = CodegenConfig::parse("[codegen.type_overrides\n")
            .expect_err("an unterminated section header is rejected");
        assert!(bad_section.to_string().contains("section") || bad_section.to_string().contains("expected"));
    }

    #[test]
    fn configuration_files_are_read_from_disk() {
        let path = std::env::temp_dir().join(format!(
            "rurqe-config-{}-{}.toml",
            std::process::id(),
            std::line!()
        ));
        std::fs::write(
            &path,
            "[codegen.type_overrides]\n\"users.id\" = \"uuid::Uuid\"\n",
        )
        .expect("write configuration");

        let config = CodegenConfig::load(&path).expect("the file is valid");
        let _ = std::fs::remove_file(&path);

        assert_eq!(config.type_override("users", "id"), Some("uuid::Uuid"));
    }

    #[test]
    fn missing_configuration_yields_the_default() {
        let path = std::env::temp_dir().join(format!(
            "rurqe-config-missing-{}-{}.toml",
            std::process::id(),
            std::line!()
        ));
        let _ = std::fs::remove_file(&path);

        let config = CodegenConfig::load(&path).expect("a missing file is not an error");

        assert_eq!(config, CodegenConfig::default());
        assert!(config.type_override("users", "id").is_none());
    }

    #[test]
    fn diagnostics_point_at_the_entry_not_at_an_earlier_mention() {
        let err = CodegenConfig::parse(
            "# `users.id` is generated from the database\n\
             [codegen.type_overrides]\n\
             \"users.id\" = \"\"\n",
        )
        .expect_err("an empty type is rejected");

        assert!(matches!(&err, ConfigError::Syntax { line: 3, .. }), "{err}");
        assert!(err.to_string().contains("cannot be empty"));
    }

    #[test]
    fn commented_out_entries_are_not_located() {
        let err = CodegenConfig::parse(
            "# \"users.id\" = \"uuid::Uuid\"\n\
             [codegen.type_overrides]\n\
             \"users.id\" = \"\"  # an empty type is rejected\n",
        )
        .expect_err("an empty type is rejected");

        assert!(matches!(&err, ConfigError::Syntax { line: 3, .. }), "{err}");
    }

    #[test]
    fn nested_entries_are_reported_at_their_own_line() {
        let err = CodegenConfig::parse(
            "# `created_at` is generated from the database\n\
             [codegen.type_overrides.users]\n\
             created_at = \"\"\n",
        )
        .expect_err("an empty type is rejected");

        assert!(matches!(&err, ConfigError::Syntax { line: 3, .. }), "{err}");
    }

    #[test]
    fn dotted_entries_are_reported_at_their_own_line() {
        let err = CodegenConfig::parse(
            "# `chrono::NaiveDateTime` is the supported timestamp type\n\
             [codegen.type_overrides]\n\
             users.created_at = \"\"\n",
        )
        .expect_err("an empty type is rejected");

        assert!(matches!(&err, ConfigError::Syntax { line: 3, .. }), "{err}");
    }

    #[test]
    fn non_string_entries_are_rejected_at_their_own_line() {
        let err = CodegenConfig::parse(
            "[codegen.type_overrides]\n\
             \"users.id\" = 42\n",
        )
        .expect_err("a numeric entry is rejected");

        assert!(matches!(&err, ConfigError::Syntax { line: 2, .. }), "{err}");
    }

    #[test]
    fn unknown_sections_are_reported_at_their_own_line() {
        let err = CodegenConfig::parse(
            "# project configuration\n\
             [workspace]\n\
             members = [\"a\", \"b\"]\n",
        )
        .expect_err("unknown sections are rejected");

        assert!(matches!(&err, ConfigError::Syntax { line: 2, .. }), "{err}");
    }

    #[test]
    fn quoted_and_dotted_spellings_collide() {
        let err = CodegenConfig::parse(
            "[codegen.type_overrides]\n\
             \"users.id\" = \"uuid::Uuid\"\n\
             users.id = \"chrono::NaiveDateTime\"\n",
        )
        .expect_err("both spellings name the same entry");

        assert!(matches!(&err, ConfigError::Syntax { line: 3, .. }), "{err}");
        assert!(err.to_string().contains("duplicate type override `users.id`"));
    }
}
