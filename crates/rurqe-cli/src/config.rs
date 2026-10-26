//! Project configuration for the code generation toolchain.
//!
//! `cargo db-codegen` reads the optional `[codegen]` table and its
//! `[codegen.type_overrides]` sub-table from the `rurqe.toml` file the command is
//! pointed at — the working directory of the process by default. Every override
//! entry maps a
//! `table.column` schema path onto the Rust type generated for that column, so
//! domain newtypes (`uuid::Uuid`, `chrono::NaiveDateTime`, money wrappers)
//! survive code generation instead of degrading to the engine-neutral fallback.
//!
//! The root-level `schema` key names the one database schema every catalog
//! digest of the project describes. It is read by each command that compares
//! catalogs — `codegen`, `verify`, `stage init`, `stage commit`, and the
//! `migrate` actions — where an explicit `--schema` argument wins over it and
//! the libpq default `public` applies when neither name a schema, so a project
//! keeping its relations outside `public` never anchors a hash of an empty
//! catalog.
//!
//! ```toml
//! schema = "app"
//!
//! [codegen]
//! domain_types = true
//!
//! [codegen.type_overrides]
//! "users.id" = "uuid::Uuid"
//! users.created_at = "chrono::NaiveDateTime"
//! ```
//!
//! The document is read through [`toml_edit::ImDocument`], the immutable
//! representation that keeps the byte span of every key and value recorded while
//! parsing. Every rejected entry is therefore reported at the line and column of
//! its own declaration: the positions come from the parsed tree rather than from a
//! scan of the source text, so a declaration spelled inside a multi-line string
//! literal, inside a comment, or in the flat (`"users.id" = "..."`), dotted
//! (`users.id = ...`) and table (`[codegen.type_overrides.users]`) form is located
//! exactly where it is written, independently of how the file is formatted.

use std::collections::BTreeMap;
use std::fmt;
use std::ops::Range;
use std::path::Path;

use toml_edit::{ImDocument, Item, TableLike};

/// The configuration file read by default.
pub const DEFAULT_CONFIG_PATH: &str = "rurqe.toml";

/// The document-root key naming the database schema every catalog digest describes.
const SCHEMA_KEY: &str = "schema";

/// The section holding the code generation settings.
const CODEGEN_SECTION: &str = "codegen";

/// The key enabling domain types in the generated schema.
const DOMAIN_TYPES_KEY: &str = "domain_types";

/// The key declaring the per-column Rust type overrides.
const TYPE_OVERRIDES_KEY: &str = "type_overrides";

/// The project configuration read from `rurqe.toml`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CodegenConfig {
    /// The database schema the project's catalogs describe, if named.
    schema: Option<String>,
    /// Rust types keyed by their `table.column` schema path.
    type_overrides: BTreeMap<String, String>,
    /// Whether domain types (`Uuid`, `Json`, `Timestamp`, `Date`) are enabled.
    domain_types: bool,
}

impl CodegenConfig {
    /// Reads the configuration file at `path`, defaulting when it is absent.
    ///
    /// A missing file means "no overrides and no schema selection", while an
    /// unreadable or malformed file is an error: silently ignoring a typo would
    /// generate wrong types or describe the wrong schema.
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

    /// Parses the root-level `schema` key, the `[codegen]` table, and the
    /// `[codegen.type_overrides]` sub-table out of `contents`.
    ///
    /// Both spellings of an override are read: the quoted `"users.id" = "..."` form
    /// declares one path directly, and the `users.id = "..."` (or
    /// `[codegen.type_overrides.users]`) form nests the column one level deeper.
    /// Every rejected entry is reported where its own key is declared, and the two
    /// spellings of one path collide while flattening instead of silently
    /// overwriting each other.
    pub fn parse(contents: &str) -> Result<Self, ConfigError> {
        let document = ImDocument::parse(contents).map_err(|error| {
            syntax_error(
                contents,
                error.span(),
                format!("malformed TOML section or entry: {}", error.message()),
            )
        })?;

        let root = document.as_table();
        let mut config = Self::default();

        for (key, item) in root.iter() {
            let span = key_span(root, key);

            if key == SCHEMA_KEY {
                config.schema = Some(read_schema_name(contents, span, item)?);
                continue;
            }

            if key != CODEGEN_SECTION {
                return Err(syntax_error(
                    contents,
                    span,
                    format!("unknown configuration section `{key}`"),
                ));
            }

            let table = item.as_table_like().ok_or_else(|| {
                syntax_error(
                    contents,
                    span.clone(),
                    format!("`{CODEGEN_SECTION}` must be a table"),
                )
            })?;
            config.read_section(contents, table, span)?;
        }

        Ok(config)
    }

    /// Reads the keys of the `[codegen]` table.
    fn read_section(
        &mut self,
        contents: &str,
        table: &dyn TableLike,
        section_span: Option<Range<usize>>,
    ) -> Result<(), ConfigError> {
        for (key, item) in table.iter() {
            let span = key_span(table, key).or_else(|| section_span.clone());

            match key {
                DOMAIN_TYPES_KEY => {
                    self.domain_types = item.as_bool().ok_or_else(|| {
                        syntax_error(
                            contents,
                            span,
                            format!("`{CODEGEN_SECTION}.{DOMAIN_TYPES_KEY}` must be a boolean"),
                        )
                    })?;
                }
                TYPE_OVERRIDES_KEY => {
                    let overrides = item.as_table_like().ok_or_else(|| {
                        syntax_error(
                            contents,
                            span.clone(),
                            format!("`{CODEGEN_SECTION}.{TYPE_OVERRIDES_KEY}` must be a table"),
                        )
                    })?;
                    let section = format!("{CODEGEN_SECTION}.{TYPE_OVERRIDES_KEY}");
                    self.read_overrides(contents, overrides, &section)?;
                }
                other => {
                    return Err(syntax_error(
                        contents,
                        span,
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
        contents: &str,
        table: &dyn TableLike,
        section: &str,
    ) -> Result<(), ConfigError> {
        for (key, item) in table.iter() {
            let span = key_span(table, key);

            if let Some(rust_type) = item.as_str() {
                require_schema_path(contents, span.clone(), key)?;
                self.insert_override(contents, span, key.to_owned(), rust_type)?;
            } else if let Some(columns) = item.as_table_like() {
                for (column, value) in columns.iter() {
                    let entry_span = key_span(columns, column).or_else(|| span.clone());

                    let rust_type = value.as_str().ok_or_else(|| {
                        syntax_error(
                            contents,
                            entry_span.clone(),
                            format!("`{section}.{key}.{column}` must be a Rust type string"),
                        )
                    })?;
                    self.insert_override(
                        contents,
                        entry_span,
                        format!("{key}.{column}"),
                        rust_type,
                    )?;
                }
            } else {
                return Err(syntax_error(
                    contents,
                    span,
                    format!(
                        "`{section}.{key}` must be a Rust type string or a table of column overrides"
                    ),
                ));
            }
        }

        Ok(())
    }

    /// Records one override, rejecting an empty type or a repeated path.
    fn insert_override(
        &mut self,
        contents: &str,
        span: Option<Range<usize>>,
        schema_path: String,
        rust_type: &str,
    ) -> Result<(), ConfigError> {
        if rust_type.trim().is_empty() {
            return Err(syntax_error(
                contents,
                span,
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
            return Err(syntax_error(
                contents,
                span,
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

    /// Returns the database schema the project's catalogs describe, if named.
    ///
    /// A project keeping its relations outside the libpq default `public` names
    /// its schema here, so the codegen snapshot, the drift gate, the cold-start
    /// marker, and the checkpoint hash all describe the same catalog. The
    /// resolution of the selection — an explicit `--schema` argument winning over
    /// this key, and `public` applying when neither names a schema — belongs to
    /// the command layer, which reads this accessor.
    pub fn schema(&self) -> Option<&str> {
        self.schema.as_deref()
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
        /// The one-based line number of the offending declaration.
        line: usize,
        /// The one-based column number within that line.
        column: usize,
        /// What is wrong with the declaration.
        message: String,
    },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::Io { path, message } => {
                write!(f, "failed to read configuration `{path}`: {message}")
            }
            ConfigError::Syntax {
                line,
                column,
                message,
            } => write!(
                f,
                "invalid configuration at line {line}, column {column}: {message}"
            ),
        }
    }
}

impl std::error::Error for ConfigError {}

/// Reads the document-root `schema` key naming the described database schema.
///
/// The name decides which catalog every digest of the project describes, so a
/// value that is not a string is reported where it is declared and a blank one
/// is refused instead of silently selecting nothing.
fn read_schema_name(
    contents: &str,
    span: Option<Range<usize>>,
    item: &Item,
) -> Result<String, ConfigError> {
    let value = item.as_str().ok_or_else(|| {
        syntax_error(
            contents,
            span.clone(),
            format!("`{SCHEMA_KEY}` must be a database schema name string"),
        )
    })?;

    let name = value.trim();

    if name.is_empty() {
        return Err(syntax_error(
            contents,
            span,
            format!("`{SCHEMA_KEY}` requires a database schema name"),
        ));
    }

    Ok(name.to_owned())
}

/// The span covering the declaration of `key` inside `table`.
///
/// The key carries the span of its own spelling, while a table declared by a
/// header carries the span of that header, so the item is consulted whenever the
/// key itself has no span of its own. Both are recorded only by
/// [`toml_edit::ImDocument`]; the mutable document drops them.
fn key_span(table: &dyn TableLike, key: &str) -> Option<Range<usize>> {
    let (entry, item) = table.get_key_value(key)?;
    entry.span().or_else(|| item.span())
}

/// Builds a [`ConfigError::Syntax`] located at the start of `span`.
///
/// A declaration without a span — one the parser assembled rather than read — is
/// reported at the start of the file instead of at a guessed offset.
fn syntax_error(contents: &str, span: Option<Range<usize>>, message: String) -> ConfigError {
    let (line, column) = span.map_or((1, 1), |span| line_and_column(contents, span.start));
    ConfigError::Syntax {
        line,
        column,
        message,
    }
}

/// The one-based line and column of the byte at `offset`.
///
/// The prefix is sliced through [`str::get`] so an offset that is not a character
/// boundary — or one past the end of the file — falls back to the first line
/// instead of panicking. The column counts the characters preceding the offset on
/// its line, so a diagnostic lands on the declaration even when it is indented.
fn line_and_column(contents: &str, offset: usize) -> (usize, usize) {
    let prefix = contents.get(..offset).unwrap_or_default();
    let line = prefix.matches('\n').count() + 1;
    let column = prefix
        .rsplit('\n')
        .next()
        .map_or(1, |tail| tail.chars().count() + 1);
    (line, column)
}

/// Accepts only an explicit `table.column` path on the flat spelling of an entry.
fn require_schema_path(
    contents: &str,
    span: Option<Range<usize>>,
    key: &str,
) -> Result<(), ConfigError> {
    if !key.contains('.') || key.starts_with('.') || key.ends_with('.') {
        return Err(syntax_error(
            contents,
            span,
            format!("expected a `table.column` schema path, got `{key}`"),
        ));
    }

    Ok(())
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
    fn the_schema_key_names_the_described_schema() {
        let config = CodegenConfig::parse("schema = \"app\"\n").expect("the schema key parses");
        assert_eq!(config.schema(), Some("app"));

        let config = CodegenConfig::parse(
            "schema = \"  app  \"\n\
             [codegen]\n\
             domain_types = true\n",
        )
        .expect("the schema key parses beside the codegen section");
        assert_eq!(config.schema(), Some("app"));
        assert!(config.domain_types());
    }

    #[test]
    fn the_schema_selection_is_absent_by_default() {
        assert_eq!(
            CodegenConfig::parse("")
                .expect("an empty file is valid")
                .schema(),
            None
        );
        assert_eq!(
            CodegenConfig::parse("[codegen.type_overrides]\n")
                .expect("an empty overrides table is valid")
                .schema(),
            None
        );
    }

    #[test]
    fn a_blank_or_non_string_schema_is_rejected() {
        let blank = CodegenConfig::parse("schema = \"   \"\n")
            .expect_err("a blank schema name selects nothing");
        assert!(matches!(&blank, ConfigError::Syntax { line: 1, .. }), "{blank}");
        assert!(blank.to_string().contains("schema"), "{blank}");

        let numeric =
            CodegenConfig::parse("schema = 42\n").expect_err("a numeric schema name is rejected");
        assert!(
            matches!(&numeric, ConfigError::Syntax { line: 1, .. }),
            "{numeric}"
        );
        assert!(numeric.to_string().contains("schema"), "{numeric}");
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
            "schema = \"app\"\n[codegen.type_overrides]\n\"users.id\" = \"uuid::Uuid\"\n",
        )
        .expect("write configuration");

        let config = CodegenConfig::load(&path).expect("the file is valid");
        let _ = std::fs::remove_file(&path);

        assert_eq!(config.schema(), Some("app"));
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
        assert_eq!(config.schema(), None);
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

    #[test]
    fn diagnostics_carry_the_column_of_the_entry() {
        let err = CodegenConfig::parse(
            "[codegen.type_overrides]\n\
             \"users.id\" = \"\"\n",
        )
        .expect_err("an empty type is rejected");

        assert!(
            matches!(&err, ConfigError::Syntax { line: 2, column: 1, .. }),
            "{err}"
        );
        assert!(err.to_string().contains("line 2, column 1"), "{err}");
    }

    #[test]
    fn declarations_inside_a_string_literal_do_not_displace_the_diagnostic() {
        // The literal on lines 2-4 spells a complete override, which a scan of the
        // source text cannot tell apart from a real declaration; the span of the
        // parsed key can.
        let err = CodegenConfig::parse(
            "[codegen.type_overrides]\n\
             \"users.note\" = '''\n\
             \"users.id\" = \"uuid::Uuid\"\n\
             '''\n\
             \"users.id\" = \"\"\n",
        )
        .expect_err("an empty type is rejected");

        assert!(
            matches!(&err, ConfigError::Syntax { line: 5, column: 1, .. }),
            "{err}"
        );
        assert!(err.to_string().contains("cannot be empty"));
    }
}
