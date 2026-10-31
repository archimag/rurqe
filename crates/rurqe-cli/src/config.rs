//! Project configuration for the code generation toolchain.
//!
//! `cargo db-codegen` reads the optional `[codegen]` table and its
//! `[codegen.type_overrides]` sub-table from the `rurqe.toml` file the command is
//! pointed at — the working directory of the process by default. Every override
//! entry maps a schema path onto the Rust type generated for that column, so
//! domain newtypes (`uuid::Uuid`, `chrono::NaiveDateTime`, money wrappers) survive
//! code generation instead of degrading to the engine-neutral fallback.
//!
//! The root-level `primary_schema` key names the one schema every catalog digest
//! of the project describes, and the optional `schemas` array names the secondary
//! schemas the project declares beside it. They are read by each command that
//! compares catalogs — `codegen`, `verify`, `stage init`, `stage commit`, and the
//! `migrate` actions — where an explicit `--schema` argument wins over them and the
//! libpq default `public` applies when neither names a schema, so a project keeping
//! its relations outside `public` never anchors a hash of an empty catalog. The
//! legacy `schema` key stays an alias of `primary_schema` and is refused whenever
//! both are declared, so one file cannot select two primary schemas at once.
//!
//! An override path names its column as `table.column` for a table of the primary
//! schema or as `schema.table.column` for a table of a secondary one, and every
//! segment of the path has to be non-empty.
//!
//! ```toml
//! primary_schema = "app"
//! schemas = ["auth", "billing"]
//!
//! [codegen]
//! domain_types = true
//!
//! [codegen.type_overrides]
//! "users.id" = "uuid::Uuid"
//! users.created_at = "chrono::NaiveDateTime"
//!
//! [codegen.type_overrides.auth.users]
//! id = "uuid::Uuid"
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
use std::path::Path;

mod parse;

#[cfg(test)]
mod tests;

/// The configuration file read by default.
pub const DEFAULT_CONFIG_PATH: &str = "rurqe.toml";

/// The project configuration read from `rurqe.toml`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CodegenConfig {
    /// The primary schema declared through `primary_schema` or its legacy alias.
    primary_schema: Option<String>,
    /// The secondary schemas declared through `schemas`, in document order.
    schemas: Vec<String>,
    /// Rust types keyed by their `schema.table.column` (or `table.column`) path.
    type_overrides: BTreeMap<String, String>,
    /// Whether domain types (`Uuid`, `Json`, `Timestamp`, `Date`) are enabled.
    domain_types: bool,
}

impl CodegenConfig {
    /// Reads the configuration file at `path`, defaulting when it is absent.
    ///
    /// A missing file means "no overrides, no schemas, and no schema selection",
    /// while an unreadable or malformed file is an error: silently ignoring a typo
    /// would generate wrong types or describe the wrong schemas.
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

    /// Parses the root-level schema keys, the `[codegen]` table, and the
    /// `[codegen.type_overrides]` sub-table out of `contents`.
    ///
    /// `primary_schema` and its legacy alias `schema` both name the primary schema,
    /// so declaring the two at once is refused where the alias sits and the
    /// diagnostic carries the position of both keys. `schemas` may neither repeat a
    /// name nor name the primary schema itself — the declared one, or the `public`
    /// default when neither key names one — because a secondary list containing the
    /// primary schema would describe the same catalog twice.
    ///
    /// An override path is written in one of three equivalent spellings: the quoted
    /// `"auth.users.id" = "..."` form declares one path directly, the dotted
    /// `auth.users.id = "..."` form nests the column one level per dot, and the
    /// table `[codegen.type_overrides.auth.users]` form nests the column one level
    /// per table. Every spelling flattens onto the same `schema.table.column`
    /// entry, so the collision between two of them is reported instead of one
    /// silently overwriting the other, and every rejected entry is reported where
    /// its own key is declared.
    pub fn parse(contents: &str) -> Result<Self, ConfigError> {
        parse::document(contents)
    }

    /// Returns the Rust type declared for `table`.`column` of the primary schema.
    pub fn type_override(&self, table: &str, column: &str) -> Option<&str> {
        self.type_overrides
            .get(&format!("{table}.{column}"))
            .map(String::as_str)
    }

    /// Returns whether domain types are enabled in configuration.
    pub fn domain_types(&self) -> bool {
        self.domain_types
    }

    /// Returns the primary schema the project's catalogs describe, if named.
    ///
    /// The name comes from the `primary_schema` key or from its legacy `schema`
    /// alias, and `None` means neither names one, so the command layer applies its
    /// `public` default. A project keeping its relations outside that default names
    /// its schema here, so the codegen snapshot, the drift gate, the cold-start
    /// marker, and the checkpoint hash all describe the same catalog. The
    /// resolution of the selection — an explicit `--schema` argument winning over
    /// this key — belongs to the command layer, which reads this accessor.
    pub fn primary_schema(&self) -> Option<&str> {
        self.primary_schema.as_deref()
    }

    /// Returns the primary schema under the historical name of the accessor.
    ///
    /// `schema` and `primary_schema` are one setting: the legacy `schema` key is an
    /// alias of `primary_schema`, so the two accessors read the same value and a
    /// caller of either observes the selection the file declares once.
    pub fn schema(&self) -> Option<&str> {
        self.primary_schema()
    }

    /// Returns the secondary schemas declared through `schemas`, in document order.
    ///
    /// The list holds every schema the project declares beside the primary one and
    /// never repeats it: an entry naming the primary schema is refused while the
    /// file is parsed, so iterating the list visits each secondary schema exactly
    /// once.
    pub fn schemas(&self) -> &[String] {
        &self.schemas
    }

    /// Returns the declared overrides keyed by their schema path.
    ///
    /// A primary-schema column is keyed as `table.column` and a secondary-schema
    /// one as `schema.table.column`, with every segment trimmed.
    pub fn type_overrides(&self) -> &BTreeMap<String, String> {
        &self.type_overrides
    }

    /// Consumes the configuration, yielding the overrides for code generation.
    pub fn into_type_overrides(self) -> BTreeMap<String, String> {
        self.type_overrides
    }
}

/// The database schemas a project's catalog digests describe.
///
/// A project names one primary schema — the one its relations render without a
/// schema qualifier — and any number of secondary schemas beside it. The
/// selection carries both lists in the order the catalog statements bind them:
/// the primary schema first, then every secondary schema in declaration order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaSelection {
    /// The primary schema: the one relations render without a qualifier.
    primary: String,
    /// The secondary schemas, in declaration order.
    secondary: Vec<String>,
}

impl SchemaSelection {
    /// Builds a selection out of its primary schema and its secondary schemas.
    ///
    /// The two lists never overlap: the command layer refuses a secondary schema
    /// that repeats the effective primary one — a collision an explicit
    /// `--schema` argument can introduce even when the configuration file
    /// declares none — before a selection is built.
    pub fn new(primary: impl Into<String>, secondary: Vec<String>) -> Self {
        Self {
            primary: primary.into(),
            secondary,
        }
    }

    /// Builds a selection describing `name` alone.
    ///
    /// Every entry point that names one schema — a single-schema introspection
    /// call, a call site without a project configuration — describes that schema
    /// and nothing beside it.
    pub fn single(name: impl Into<String>) -> Self {
        Self::new(name, Vec::new())
    }

    /// The primary schema: the one relations render without a qualifier.
    pub fn primary(&self) -> &str {
        &self.primary
    }

    /// The secondary schemas, in declaration order.
    pub fn secondary(&self) -> &[String] {
        &self.secondary
    }

    /// Every selected schema, the primary one first.
    ///
    /// The order is stable for a given selection, so the array a catalog
    /// statement binds and the list a notice names describe the same schemas on
    /// every run.
    pub fn all(&self) -> Vec<String> {
        let mut schemas = Vec::with_capacity(self.secondary.len() + 1);
        schemas.push(self.primary.clone());
        schemas.extend(self.secondary.iter().cloned());
        schemas
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
    ///
    /// The position is the one of the rejected declaration. A rejection that
    /// concerns two declarations — the legacy `schema` declared beside
    /// `primary_schema` — reports the position of the alias here and names the
    /// position of the other key in the message, so both are readable at once.
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
