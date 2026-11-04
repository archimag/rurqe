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
//! A declared path is therefore relative to that directory, and it is written the
//! way it resolves: a value the host reads as absolute or rooted, and a value
//! carrying a `..` segment — a round trip that ends back inside the directory
//! included — are refused while the file is parsed, so a declaration relocates an
//! artifact inside the project but never rewrites where it lands.
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

mod discover;
mod parse;

pub use discover::{config_path_in, discover_config, discover_or_default};

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
    /// The per-column Rust types declared for the project, keyed by their path segments.
    ///
    /// The index keeps the segments the document spelled rather than a flattened
    /// path, so [`CodegenConfig::type_override`] answers a column from the parts a
    /// catalog already carries and no lookup renders a key of its own.
    type_overrides: OverrideIndex,
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
    /// Reads the configuration file at `path`.
    ///
    /// A missing file is an error in its own right — a [`ConfigError::NotFound`]
    /// carrying the path — exactly like an unreadable or malformed one, so a
    /// mistyped explicit path can never degrade into the engine-neutral defaults:
    /// running with default schemas, the conventional artifact layout, and no type
    /// overrides is precisely the silent misconfiguration this crate refuses.
    ///
    /// A malformed document is reported with the position of the rejected
    /// declaration and with the file it was read from: this is the entry point
    /// that knows the path, so it is the one that attaches it to the refusal
    /// [`CodegenConfig::parse`] produced.
    ///
    /// Defaulting on absence has exactly one home, and it is not here: a caller
    /// that reads a known conventional path falls back through
    /// [`CodegenConfig::load_or_default`], and a caller that walks the directory
    /// tree falls back through [`discover_or_default`]. Both spell the rule out at
    /// the call site, where the tier the path came from is known, instead of
    /// hiding it behind a read that cannot tell the two apart.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        match std::fs::read_to_string(path) {
            Ok(contents) => Self::parse(&contents).map_err(|error| error.locate(path)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Err(ConfigError::NotFound {
                    path: path.display().to_string(),
                })
            }
            Err(error) => Err(ConfigError::Io {
                path: path.display().to_string(),
                source: error,
            }),
        }
    }

    /// Reads the configuration file at `path`, falling back to the default when it is absent.
    ///
    /// This is the conventional zero-config entry point, for a location the caller already
    /// knows: the `rurqe.toml` of a working directory, or the file [`discover_config`]
    /// answered with. A project that keeps no configuration is an ordinary project, so an
    /// absent file means the engine-neutral defaults — and nothing else does. Every other
    /// failure is reported exactly as [`CodegenConfig::load`] reports it: a file that exists
    /// but cannot be read is a [`ConfigError::Io`], and a malformed one is a
    /// [`ConfigError::Syntax`] carrying the position of the rejected declaration
    /// and the file it was read from.
    ///
    /// The 'absent means default' rule lives here and not in `load`, which reads the file
    /// once and reports an absence in its own right through [`ConfigError::NotFound`], so a
    /// path an operator named explicitly — a mistyped `--config` argument — can never be
    /// mistaken for a project that declares nothing.
    pub fn load_or_default(path: &Path) -> Result<Self, ConfigError> {
        match Self::load(path) {
            Err(ConfigError::NotFound { .. }) => Ok(Self::default()),
            result => result,
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
    /// containing the primary schema would describe the same catalog twice. No
    /// declaration names a schema containing a `.` either: the override grammar
    /// reserves the character as the separator of a `schema.table.column` path, so
    /// such a name could never be addressed by a `codegen.type_overrides` entry,
    /// and the declaration is refused where it is written.
    ///
    /// The artifact keys follow the same rules as every other key of the
    /// document: `root` and the `dir`, `checkpoint`, and `output` keys of their
    /// sections are trimmed non-empty strings, a declared path resolves against
    /// the directory holding the file, and a key the parser does not know is
    /// rejected where it is written.
    ///
    /// The resolution never replaces that directory: a declared path the host
    /// reads as absolute or rooted — `root = "/var/rurqe"` — is refused at the
    /// key that declares it, exactly like a path carrying a `..` segment, a round
    /// trip that ends back inside the directory included. A declaration relocates
    /// an artifact inside the project; it never spells one location through the
    /// arithmetic of another.
    ///
    /// An override path is written in one of three equivalent spellings: the
    /// quoted `"auth.users.id" = "..."` form declares one path directly, the
    /// dotted `auth.users.id = "..."` form nests the column one level per dot,
    /// and the table `[codegen.type_overrides.auth.users]` form nests the column
    /// one level per table. Every spelling flattens onto the same
    /// `schema.table.column` entry, so the collision between two of them is
    /// reported instead of one silently overwriting the other, and every rejected
    /// entry is reported where its own key is declared.
    ///
    /// A three-segment path names a schema the document declares in `schemas`,
    /// because a secondary-schema column is addressed as `schema.table.column`
    /// while a primary-schema column is addressed as `table.column`. A path
    /// naming a schema the project does not declare — a misspelled secondary
    /// schema, or the primary schema itself — could never reach a column and is
    /// refused where it is declared, so a typo cannot silently leave the
    /// generated module on the type the default mapping produces. The
    /// cross-check runs once the whole document has been read, because a dotted
    /// root key may declare an override before the `schemas` key that
    /// authorises it.
    ///
    /// Every refusal carries the position of the declaration that caused it but
    /// no file, because the document arrives as text: [`CodegenConfig::load`] is
    /// the entry point that knows the path it read and attaches it.
    pub fn parse(contents: &str) -> Result<Self, ConfigError> {
        parse::document(contents)
    }

    /// Returns the Rust type declared for `table`.`column` of `schema`.
    ///
    /// `schema` names the schema owning the column, and `None` addresses the
    /// primary one, because that is how the file spells an override path: a column
    /// of the primary schema is declared as `table.column` and a column of a
    /// secondary schema as `schema.table.column` for a schema the document lists in
    /// [`schemas`](CodegenConfig::schemas). A schema the document does not declare
    /// holds no override, so the answer is `None` rather than a default mapping.
    ///
    /// The lookup matches the parts, so a consumer resolving the type of every
    /// column of a catalog pays no key spelling and no allocation per column: the
    /// schema, the table, and the column a catalog already reports are compared
    /// against the segments the document declared, and the caller never assembles
    /// `schema.table.column` itself only to look it back up. A consumer whose own
    /// model is keyed by that path renders the whole set once through
    /// [`CodegenConfig::type_overrides`].
    pub fn type_override(&self, schema: Option<&str>, table: &str, column: &str) -> Option<&str> {
        self.type_overrides.get(schema, table, column)
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

    /// Renders the declared overrides onto their schema paths.
    ///
    /// A primary-schema column is keyed as `table.column` and a secondary-schema
    /// one as `schema.table.column` for a schema declared in `schemas`, with every
    /// segment trimmed. The rendering is a projection of the very entries
    /// [`CodegenConfig::type_override`] answers, so a consumer keyed by the path and
    /// a consumer resolving one column never describe different overrides.
    ///
    /// The rendering exists for such a consumer — the code generator carries its
    /// overrides in a map of its own — while a per-column lookup belongs to
    /// [`CodegenConfig::type_override`], which neither spells a key nor allocates
    /// one per column.
    pub fn type_overrides(&self) -> BTreeMap<String, String> {
        self.type_overrides.flatten()
    }

    /// Consumes the configuration, yielding the overrides for code generation.
    ///
    /// The entries are rendered onto their schema paths exactly as
    /// [`CodegenConfig::type_overrides`] renders them.
    pub fn into_type_overrides(self) -> BTreeMap<String, String> {
        self.type_overrides.flatten()
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

/// One declared override path: the schema owning the column, its table, and its column.
///
/// The document spells a path as `table.column` or `schema.table.column`, and the
/// path is kept as the segments it carries rather than as the flattened string:
/// the index is keyed by those segments, so a column is answered from the parts a
/// catalog reports instead of from a key the caller assembles. The flattened
/// spelling stays a rendering of them, so a diagnostic and a lookup can never
/// disagree about how a declaration is written.
#[derive(Debug)]
struct OverridePath {
    /// The secondary schema owning the column, or `None` for the primary schema.
    schema: Option<String>,
    /// The table declaring the column.
    table: String,
    /// The column the override addresses.
    column: String,
}

impl fmt::Display for OverridePath {
    /// Renders the path the way `rurqe.toml` spells it.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.schema.as_deref() {
            Some(schema) => write!(f, "{schema}.{}.{}", self.table, self.column),
            None => write!(f, "{}.{}", self.table, self.column),
        }
    }
}

/// The Rust types declared for individual columns, keyed by their path segments.
///
/// The primary schema owns a `table -> column` map and every secondary schema owns
/// the same map one level deeper, which is the shape a lookup driven by a catalog
/// walks: the schema selects the namespace, the table selects the columns of one
/// relation, and the column selects the type. Keeping the segments is what leaves
/// the lookup allocation-free, because nothing has to be rendered into a key
/// before an entry can be found. The flat spelling is a rendering of the same
/// entries ([`OverrideIndex::flatten`]), so the two views cannot drift apart.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct OverrideIndex {
    /// The overrides of the primary schema, keyed by table and then by column.
    primary: BTreeMap<String, BTreeMap<String, String>>,
    /// The overrides of every secondary schema, keyed the same way one level deeper.
    secondary: BTreeMap<String, BTreeMap<String, BTreeMap<String, String>>>,
}

impl OverrideIndex {
    /// Answers the Rust type declared for `table`.`column` of `schema`.
    ///
    /// `None` addresses the primary schema, exactly as the document spells its
    /// overrides; a schema holding no override answers `None` without walking the
    /// tables of any other.
    fn get(&self, schema: Option<&str>, table: &str, column: &str) -> Option<&str> {
        let columns = match schema {
            Some(schema) => self.secondary.get(schema)?.get(table)?,
            None => self.primary.get(table)?,
        };

        columns.get(column).map(String::as_str)
    }

    /// Records one override, answering the type it displaced when the path repeats.
    fn insert(&mut self, path: &OverridePath, rust_type: String) -> Option<String> {
        match &path.schema {
            Some(schema) => self
                .secondary
                .entry(schema.clone())
                .or_default()
                .entry(path.table.clone())
                .or_default()
                .insert(path.column.clone(), rust_type),
            None => self
                .primary
                .entry(path.table.clone())
                .or_default()
                .insert(path.column.clone(), rust_type),
        }
    }

    /// Renders every entry onto its schema path, in canonical path order.
    ///
    /// The rendering is what a consumer keyed by the path reads, and it is derived
    /// from the very segments the lookup answers, so the two views of one entry
    /// cannot describe different overrides.
    fn flatten(&self) -> BTreeMap<String, String> {
        let mut overrides = BTreeMap::new();

        for (table, columns) in &self.primary {
            for (column, rust_type) in columns {
                overrides.insert(format!("{table}.{column}"), rust_type.clone());
            }
        }

        for (schema, tables) in &self.secondary {
            for (table, columns) in tables {
                for (column, rust_type) in columns {
                    let path = format!("{schema}.{table}.{column}");

                    overrides.insert(path, rust_type.clone());
                }
            }
        }

        overrides
    }
}

/// Error raised while reading the project configuration.
#[derive(Debug)]
pub enum ConfigError {
    /// The configuration file could not be read, or a candidate of the discovery walk
    /// could not be classified.
    ///
    /// The operating-system failure is kept as the
    /// [`source`](std::error::Error::source) of the error rather than flattened
    /// into a string, so the caller still observes the
    /// [`ErrorKind`](std::io::ErrorKind) that decided the read failed —
    /// `PermissionDenied` for a file the process may not read, `InvalidData`
    /// for contents that are not UTF-8 — and an error reporter down the chain
    /// receives the underlying cause.
    Io {
        /// The path that was read.
        path: String,
        /// The underlying operating-system error.
        source: std::io::Error,
    },
    /// A declaration of the configuration is refused.
    ///
    /// The position is the one of the rejected declaration. A rejection that
    /// concerns two declarations — the legacy `schema` declared beside
    /// `primary_schema` — reports the position of the alias here and names the
    /// position of the other key in the message, so both are readable at once.
    ///
    /// The file the declaration was read from is carried whenever the caller
    /// knew it: a command reads the document through [`CodegenConfig::load`],
    /// which attaches its path, while [`CodegenConfig::parse`] has no path to
    /// attach and leaves the field empty. The rendered line names the file
    /// exactly when it is known, so the diagnostic of a direct parse keeps the
    /// position-and-message wording it has always had.
    Syntax {
        /// The one-based line number of the offending declaration.
        line: usize,
        /// The one-based column number within that line.
        column: usize,
        /// What is wrong with the declaration.
        message: String,
        /// The file the declaration was read from, when the caller knew it.
        path: Option<String>,
    },
    /// The configuration file named by the path does not exist.
    ///
    /// An absence is reported in its own right instead of as an [`Io`](ConfigError::Io)
    /// failure, so the caller that reads the file decides what it means: a conventional path
    /// falls back to the defaults through [`CodegenConfig::load_or_default`], while a path an
    /// operator named explicitly is the mistake it is.
    NotFound {
        /// The path that does not exist.
        path: String,
    },
    /// Something occupies the configuration path that is not a regular file.
    ///
    /// A directory, a socket, or a device at `<directory>/rurqe.toml` is not a
    /// configuration a project can declare, so the discovery walk reports it instead
    /// of passing over it: skipping such a candidate would continue to an ancestor's
    /// configuration or to the engine-neutral defaults, which is exactly the silent
    /// misconfiguration this crate refuses.
    NotARegularFile {
        /// The path that is occupied by something other than a regular file.
        path: String,
    },
}

impl ConfigError {
    /// Attaches the file the document was read from to a position-bearing refusal.
    ///
    /// [`CodegenConfig::parse`] reads a document it was handed and knows no file,
    /// so its refusals carry a position and a message alone. [`CodegenConfig::load`]
    /// is the entry point that knows which file it read, so it attaches the path
    /// here; that is what lets a diagnostic name the configuration that is
    /// actually malformed, whether the file came from a discovery walk that may
    /// select an ancestor's `rurqe.toml` among several or from an explicit
    /// `--config` path naming any file at all. The other variants already carry a
    /// path of their own and pass through unchanged.
    fn locate(self, path: &Path) -> Self {
        match self {
            ConfigError::Syntax { line, column, message, .. } => ConfigError::Syntax {
                line,
                column,
                message,
                path: Some(path.display().to_string()),
            },
            other => other,
        }
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::Io { path, source } => {
                write!(f, "failed to read configuration `{path}`: {source}")
            }
            ConfigError::Syntax {
                line,
                column,
                message,
                path,
            } => match path {
                Some(path) => write!(
                    f,
                    "invalid configuration in `{path}` at line {line}, column {column}: {message}"
                ),
                None => write!(
                    f,
                    "invalid configuration at line {line}, column {column}: {message}"
                ),
            },
            ConfigError::NotFound { path } => {
                write!(f, "configuration file `{path}` does not exist")
            }
            ConfigError::NotARegularFile { path } => write!(
                f,
                "configuration path `{path}` exists but is not a regular file; remove or rename \
                 the entry so the project keeps a readable `rurqe.toml` there"
            ),
        }
    }
}

impl std::error::Error for ConfigError {
    /// The operating-system failure behind a configuration that could not be read.
    ///
    /// `Syntax` is a failure of the document's own content, `NotFound` is an absence
    /// at the named path, and `NotARegularFile` is the shape of the entry occupying it,
    /// so none of them carries a cause beneath it.
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ConfigError::Io { source, .. } => Some(source),
            ConfigError::Syntax { .. }
            | ConfigError::NotFound { .. }
            | ConfigError::NotARegularFile { .. } => None,
        }
    }
}
