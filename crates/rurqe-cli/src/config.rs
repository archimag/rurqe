//! Project configuration for the code generation toolchain.
//!
//! `cargo db-codegen` reads the optional `[codegen.type_overrides]` table of the
//! `rurqe.toml` file in the working directory. Every entry maps a
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
//! Only that single table is interpreted. The reader is a deliberately small
//! line-oriented parser instead of a full TOML implementation, so the toolchain
//! keeps its dependency footprint; unrelated sections are ignored and malformed
//! entries are reported with their line number.

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

use rurqe_migrate::mini_toml::{section_name, split_entry, strip_comment, TomlError};

/// The configuration file read by default.
pub const DEFAULT_CONFIG_PATH: &str = "rurqe.toml";

/// The section holding general codegen options.
const CODEGEN_SECTION: &str = "codegen";

/// The section holding the per-column Rust type overrides.
const TYPE_OVERRIDES_SECTION: &str = "codegen.type_overrides";

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
    pub fn parse(contents: &str) -> Result<Self, ConfigError> {
        let mut config = Self::default();
        let mut current_section: Option<String> = None;

        for (index, raw) in contents.lines().enumerate() {
            let number = index + 1;
            let line = strip_comment(raw).trim();

            if line.is_empty() {
                continue;
            }

            if line.starts_with('[') {
                current_section =
                    Some(section_name(line, number).map_err(from_toml)?.to_owned());
                continue;
            }

            match current_section.as_deref() {
                Some(CODEGEN_SECTION) => {
                    let (key, value) = split_entry(line, number).map_err(from_toml)?;
                    if key == "domain_types" {
                        let enabled = match value.as_str() {
                            "true" => true,
                            "false" => false,
                            _ => {
                                return Err(syntax(
                                    number,
                                    format!("expected `true` or `false` for `domain_types`, got `{value}`"),
                                ));
                            }
                        };
                        config.domain_types = enabled;
                    }
                }
                Some(TYPE_OVERRIDES_SECTION) => {
                    let (key, value) = entry(line, number)?;
                    if config.type_overrides.insert(key.clone(), value).is_some() {
                        return Err(syntax(number, format!("`{key}` is given more than once")));
                    }
                }
                _ => {}
            }
        }

        Ok(config)
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
    /// A line of the `[codegen.type_overrides]` table is malformed.
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

/// Builds a syntax error anchored at `line`.
fn syntax(line: usize, message: impl Into<String>) -> ConfigError {
    ConfigError::Syntax {
        line,
        message: message.into(),
    }
}

/// Lifts a shared [`TomlError`] into the configuration error type.
fn from_toml(error: TomlError) -> ConfigError {
    syntax(error.line, error.message)
}

/// Splits one `key = value` entry of the overrides table.
fn entry(line: &str, number: usize) -> Result<(String, String), ConfigError> {
    let (key, value) = split_entry(line, number).map_err(from_toml)?;

    if !key.contains('.') || key.starts_with('.') || key.ends_with('.') {
        return Err(syntax(
            number,
            format!("expected a `table.column` schema path, got `{key}`"),
        ));
    }

    if value.is_empty() {
        return Err(syntax(number, "the Rust type cannot be empty"));
    }

    Ok((key, value))
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
    fn other_sections_and_comments_are_ignored() {
        let config = CodegenConfig::parse(
            "# project configuration\n\
             [workspace]\n\
             members = [\"a\", \"b\"]\n\
             \n\
             [codegen.type_overrides]\n\
             \"users.id\" = \"uuid::Uuid\" # the identity type\n",
        )
        .expect("unrelated sections are ignored");

        assert_eq!(config.type_overrides().len(), 1);
        assert_eq!(config.type_override("users", "id"), Some("uuid::Uuid"));
    }

    #[test]
    fn entries_outside_the_table_are_ignored() {
        let config = CodegenConfig::parse("[codegen]\n\"users.id\" = \"uuid::Uuid\"\n")
            .expect("unrelated tables are ignored");

        assert!(config.type_overrides().is_empty());
    }

    #[test]
    fn parses_domain_types_in_codegen_section() {
        let config = CodegenConfig::parse("[codegen]\ndomain_types = true\n")
            .expect("codegen section parsed");
        assert!(config.domain_types());

        let config_false = CodegenConfig::parse("[codegen]\ndomain_types = false\n")
            .expect("codegen section parsed");
        assert!(!config_false.domain_types());
    }

    #[test]
    fn invalid_domain_types_is_rejected() {
        let err = CodegenConfig::parse("[codegen]\ndomain_types = not_a_bool\n")
            .expect_err("non-boolean rejected");
        assert!(err.to_string().contains("domain_types"));
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
        assert!(duplicate.to_string().contains("more than once"));

        let bad_section = CodegenConfig::parse("[codegen.type_overrides\n")
            .expect_err("an unterminated section header is rejected");
        assert!(bad_section.to_string().contains("section"));
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
}
