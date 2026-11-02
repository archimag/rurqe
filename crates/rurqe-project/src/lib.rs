//! Project configuration of the RURQE toolchain.
//!
//! The `rurqe.toml` document is the single declaration of a project: the schemas
//! its catalog digests describe, the Rust types code generation emits for
//! individual columns, and the paths the lifecycle artifacts live under. The
//! model, the strict parser behind it, the resolution of every declared path, and
//! the discovery of the file itself live here — the one copy of that surface in
//! the workspace, shared by the toolchain and by the procedural macro crate, so a
//! key the parser accepts is a key every consumer understands.
//!
//! ```toml
//! primary_schema = "app"
//! schemas = ["auth", "billing"]
//!
//! [migrations]
//! dir = "rurqe/migrations"
//!
//! [stage]
//! dir = "rurqe/schema"
//! checkpoint = "rurqe/migrations/checkpoint.toml"
//!
//! [codegen]
//! output = "src/db/schema.rs"
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
//! # Artifact layout
//!
//! Every lifecycle artifact lives under one directory — the root-level `root`
//! key, `rurqe` by default — and every path declared in the file resolves against
//! the directory holding `rurqe.toml`, so a command reads the same layout
//! whatever its working directory is:
//!
//! A declared path is therefore relative to that directory: a value the host
//! reads as absolute or rooted, and one that climbs out of it with `..`, are
//! refused while the file is parsed, so a declaration relocates an artifact
//! inside the project but never outside it.
//!
//! ```text
//! project/
//! ├── rurqe.toml
//! ├── rurqe/
//! │   ├── schema/            declarative stage: the target DDL
//! │   ├── migrations/        active migration tail
//! │   │   ├── checkpoint.toml
//! │   │   └── checkpoint-stage/
//! │   └── schema.lock        catalog snapshot
//! └── src/
//! ```
//!
//! `[migrations].dir`, `[stage].dir`, and `[stage].checkpoint` override the
//! conventional artifact paths, while `[codegen].output` addresses generated
//! application sources rather than a toolchain artifact: its cascade ends at the
//! built-in default of the generator, never at the artifact layout.
//!
//! The document is read through [`toml_edit::ImDocument`], the immutable
//! representation that keeps the byte span of every key and value recorded while
//! parsing. Every rejected entry is therefore reported at the line and column of
//! its own declaration: the positions come from the parsed tree rather than from
//! a scan of the source text, so a declaration spelled inside a multi-line string
//! literal, inside a comment, or in the flat (`"users.id" = "..."`), dotted
//! (`users.id = ...`) and table (`[codegen.type_overrides.users]`) form is
//! located exactly where it is written, independently of how the file is
//! formatted.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

mod parse;

#[cfg(test)]
mod tests;

/// The configuration file read by default.
pub const DEFAULT_CONFIG_PATH: &str = "rurqe.toml";

/// The database schema a project describes when it names none.
///
/// `public` is the schema an ordinary single-schema PostgreSQL deployment keeps
/// its relations in, and the value the catalog statements bind when neither the
/// `--schema` argument nor the `primary_schema` key of the configuration names
/// one. A SQLite database holds a single unnamed catalog, so the constant
/// describes nothing there.
pub const DEFAULT_SCHEMA: &str = "public";

/// The artifact directory the project layout is built beneath by default.
const DEFAULT_ROOT: &str = "rurqe";

/// The name of the migration directory beneath the artifact directory.
const DEFAULT_MIGRATIONS_DIR: &str = "migrations";

/// The name of the declarative stage directory beneath the artifact directory.
const DEFAULT_STAGE_DIR: &str = "schema";

/// The name of the checkpoint marker inside the migration directory.
const DEFAULT_CHECKPOINT_NAME: &str = "checkpoint.toml";

/// The name of the catalog snapshot beneath the artifact directory.
const DEFAULT_LOCK_NAME: &str = "schema.lock";

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
    /// The artifact directory declared through the root-level `root` key.
    root: Option<String>,
    /// The migration directory declared through the `[migrations].dir` key.
    migrations_dir: Option<String>,
    /// The declarative stage directory declared through the `[stage].dir` key.
    stage_dir: Option<String>,
    /// The checkpoint marker declared through the `[stage].checkpoint` key.
    stage_checkpoint: Option<String>,
    /// The generated module declared through the `[codegen].output` key.
    codegen_output: Option<String>,
}

impl CodegenConfig {
    /// Reads the configuration file at `path`, defaulting when it is absent.
    ///
    /// A missing file means "no overrides, no schemas, no artifact paths, and no
    /// schema selection", while an unreadable or malformed file is an error:
    /// silently ignoring a typo would generate wrong types or describe the wrong
    /// schemas.
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

    /// Parses the root-level schema and artifact keys, the `[migrations]` and
    /// `[stage]` tables, the `[codegen]` table, and the `[codegen.type_overrides]`
    /// sub-table out of `contents`.
    ///
    /// `primary_schema` and its legacy alias `schema` both name the primary
    /// schema, so declaring the two at once is refused where the alias sits and
    /// the diagnostic carries the position of both keys. `schemas` may neither
    /// repeat a name nor name the primary schema itself — the declared one, or
    /// the `public` default when neither key names one — because a secondary list
    /// containing the primary schema would describe the same catalog twice.
    ///
    /// The artifact keys follow the same rules as every other key of the
    /// document: `root` and the `dir`, `checkpoint`, and `output` keys of their
    /// sections are trimmed non-empty strings, a declared path resolves against
    /// the directory holding the file, and a key the parser does not know is
    /// rejected where it is written.
    ///
    /// The resolution never replaces that directory: a declared path the host
    /// reads as absolute or rooted — `root = "/var/rurqe"` — and a path climbing
    /// out of it with `..` are both refused at the key that declares them, so a
    /// declaration relocates an artifact inside the project only.
    ///
    /// An override path is written in one of three equivalent spellings: the
    /// quoted `"auth.users.id" = "..."` form declares one path directly, the
    /// dotted `auth.users.id = "..."` form nests the column one level per dot,
    /// and the table `[codegen.type_overrides.auth.users]` form nests the column
    /// one level per table. Every spelling flattens onto the same
    /// `schema.table.column` entry, so the collision between two of them is
    /// reported instead of one silently overwriting the other, and every rejected
    /// entry is reported where its own key is declared.
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
    /// `public` default. A project keeping its relations outside that default
    /// names its schema here, so the codegen snapshot, the drift gate, the
    /// cold-start marker, and the checkpoint hash all describe the same catalog.
    /// The resolution of the selection — an explicit `--schema` argument winning
    /// over this key — belongs to the command layer, which reads this accessor.
    pub fn primary_schema(&self) -> Option<&str> {
        self.primary_schema.as_deref()
    }

    /// Returns the primary schema under the historical name of the accessor.
    ///
    /// `schema` and `primary_schema` are one setting: the legacy `schema` key is
    /// an alias of `primary_schema`, so the two accessors read the same value and
    /// a caller of either observes the selection the file declares once.
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

    /// Returns the artifact directory the project layout is built beneath.
    ///
    /// The name comes from the root-level `root` key, and the conventional
    /// `rurqe` applies when the file names none. The declarative stage, the
    /// migration tail, the checkpoint marker, and the catalog snapshot all
    /// default beneath this directory, so one key relocates the whole layout
    /// without restating it path by path.
    pub fn root(&self) -> &str {
        self.root.as_deref().unwrap_or(DEFAULT_ROOT)
    }

    /// Resolves the directory the migration scripts live in.
    ///
    /// The `[migrations].dir` value resolved against `directory` — the directory
    /// holding `rurqe.toml` — when the file declares one, and `<root>/migrations`
    /// beneath it otherwise.
    pub fn migrations_dir(&self, directory: &Path) -> PathBuf {
        self.artifact_path(
            directory,
            self.migrations_dir.as_deref(),
            DEFAULT_MIGRATIONS_DIR,
        )
    }

    /// Resolves the declarative stage directory.
    ///
    /// The `[stage].dir` value resolved against `directory` when the file declares
    /// one, and `<root>/schema` beneath it otherwise.
    pub fn stage_dir(&self, directory: &Path) -> PathBuf {
        self.artifact_path(directory, self.stage_dir.as_deref(), DEFAULT_STAGE_DIR)
    }

    /// Resolves the checkpoint marker guarding the migration tail.
    ///
    /// The `[stage].checkpoint` value resolved against `directory` when the file
    /// declares one, and `checkpoint.toml` inside the resolved migration directory
    /// otherwise, so relocating the migrations relocates their marker with them.
    pub fn checkpoint_path(&self, directory: &Path) -> PathBuf {
        match self.stage_checkpoint.as_deref() {
            Some(declared) => directory.join(declared),
            None => self
                .migrations_dir(directory)
                .join(DEFAULT_CHECKPOINT_NAME),
        }
    }

    /// Resolves the catalog snapshot of the project.
    ///
    /// The snapshot carries no key of its own, so it lives at `<root>/schema.lock`
    /// beneath the directory of the configuration file.
    pub fn schema_lock_path(&self, directory: &Path) -> PathBuf {
        directory.join(self.root()).join(DEFAULT_LOCK_NAME)
    }

    /// Resolves the generated schema module declared through `[codegen].output`.
    ///
    /// `None` means the file declares no output, so the built-in default of the
    /// generator is the last step of the cascade. The declared path resolves
    /// against `directory` like every other declared path, and it is not an
    /// artifact of the toolchain: it addresses generated application sources,
    /// which is why the artifact directory never contributes to it.
    pub fn codegen_output(&self, directory: &Path) -> Option<PathBuf> {
        self.codegen_output
            .as_deref()
            .map(|declared| directory.join(declared))
    }

    /// The path of an artifact: its declared value resolved against `directory`,
    /// or its conventional name beneath the artifact directory.
    fn artifact_path(
        &self,
        directory: &Path,
        declared: Option<&str>,
        default_name: &str,
    ) -> PathBuf {
        match declared {
            Some(declared) => directory.join(declared),
            None => directory.join(self.root()).join(default_name),
        }
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

/// The configuration file of the project rooted at `directory`.
///
/// A command run from a working directory reads the conventional `rurqe.toml`
/// beside it, which is exactly this path, while an explicitly supplied `--config`
/// argument replaces it with the file it names.
pub fn config_path_in(directory: &Path) -> PathBuf {
    directory.join(DEFAULT_CONFIG_PATH)
}

/// The nearest configuration file at or above `start`.
///
/// The walk climbs from `start` through its ancestors and answers with the first
/// `rurqe.toml` that exists, so a crate of a workspace finds the configuration of
/// the workspace root while a standalone crate finds its own. A procedural macro
/// starts at `CARGO_MANIFEST_DIR` — the one directory the compiler hands it — and
/// reaches the project holding the configuration through this walk. `None` means
/// no ancestor declares one, and the caller then falls back to the conventional
/// artifact layout beneath `start`.
pub fn discover_config(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .map(config_path_in)
        .find(|candidate| candidate.is_file())
}
