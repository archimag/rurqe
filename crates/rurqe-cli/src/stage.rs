//! Verification of the Golden Equation between the declarative stage and the
//! active migration tail.
//!
//! Projects following the Stage + Tail paradigm describe their schema twice: the
//! declarative stage (`schema/`) cold environments are initialised from, and the
//! active migration tail (`migrations/`) that advances existing databases. Both
//! descriptions must converge on exactly the same state:
//!
//! ```text
//! State(schema/) == State(Checkpoint + migrations/)
//! ```
//!
//! [`GoldenEquation::verify`] materialises both sides in a disposable scratch
//! state — an in-memory SQLite database, or an ephemeral PostgreSQL database that
//! is dropped afterwards — introspects the resulting catalogs, and compares them
//! through the structural [`SchemaDiff`] of the drift verifier. Tables, columns,
//! defaults, nullability, keys, and indexes that drifted are reported as an
//! actionable diagnostic instead of surfacing as a production incident.
//!
//! The scratch state starts from nothing and is thrown away between the two
//! sides. `--scratch-url` selects it: a `sqlite:` URL or a `*.db` / `*.sqlite` /
//! `*.sqlite3` path is materialised in memory (`:memory:`) or in a file the
//! verification requires to be empty and deletes once its side ends, while a
//! PostgreSQL URL creates a throwaway `rurqe_scratch_…` database on the server
//! behind the URL, builds both sides inside it, and drops it once the
//! verification ends. Every script is replayed
//! under the transactionality it declares, so a `-- rurqe:no-transaction` script
//! commits statement by statement exactly as it does in production. Neither side
//! runs inside a transaction block of its own, so statements such as
//! `CREATE INDEX CONCURRENTLY` stay replayable, and the existing contents of any
//! other database stay untouched. The connected role therefore needs the
//! `CREATEDB` privilege for the duration of the verification. The tail passed to
//! [`GoldenEquation::new`] is the active set of scripts: migrations pruned below
//! the checkpoint are already folded into the base stage, so replaying them would
//! collide with it. The boundary itself is anchored with
//! [`GoldenEquation::with_base`]: `stage commit` writes the verbatim DDL of the
//! committed stage into a `checkpoint-stage/` directory next to
//! `checkpoint.toml`, and `stage verify` loads that snapshot when no explicit
//! base stage is named.
//!
//! ```
//! use rurqe_cli::stage::GoldenEquation;
//! use rurqe_migrate::{DeclarativeStage, Migration, StageScript};
//!
//! let stage = DeclarativeStage::new(vec![StageScript::new(
//!     "users.sql",
//!     "CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT NOT NULL);",
//! )]);
//! let tail = vec![Migration::parse(
//!     "V1__users.sql",
//!     "CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT NOT NULL);",
//! )
//! .expect("a valid script name")];
//!
//! let report = GoldenEquation::new(&stage, &tail)
//!     .verify(":memory:")
//!     .expect("the stage and the tail describe the same schema");
//!
//! assert!(report.is_equivalent());
//! ```

pub(crate) mod postgres;

use std::fmt;

use rurqe_migrate::catalog::SchemaCatalog;
use rurqe_migrate::checkpoint::Checkpoint;
use rurqe_migrate::migration::Migration;
use rurqe_migrate::runner::Migrator;
use rurqe_migrate::stage::DeclarativeStage;

use crate::cli::redact_database_url;
use crate::drift::{compute_catalog_diff, DiffWording, SchemaDiff};
use crate::introspect::introspect_sqlite;
use crate::target::DatabaseTarget;

use self::postgres::PostgresSandbox;

/// One labelled SQL fragment applied to a scratch state.
struct Script<'a> {
    /// The file name of a stage script or the script name of a migration.
    label: &'a str,
    /// The SQL the fragment contributes.
    sql: &'a str,
    /// Whether the fragment is replayed inside a transaction.
    transactional: bool,
}

/// The disposable database state one side of the equation is materialised in.
///
/// The variants are dispatched statically, so a verification pays for exactly
/// the engine it targets and no trait object sits on the hot path.
enum SandboxKind {
    /// An in-memory SQLite database, so the filesystem stays untouched.
    ///
    /// Every side opens its own memory database, so nothing is left to undo.
    SqliteMemory(rusqlite::Connection),
    /// A file-backed SQLite database, deleted once its side ends.
    ///
    /// Deleting the file is the only reset that also clears the database header
    /// (`PRAGMA user_version`, `PRAGMA application_id`) and the connection
    /// settings a replayed script can change.
    SqliteFile {
        /// The live connection of the current side.
        connection: rusqlite::Connection,
        /// The file the connection was opened in, removed again on discard.
        path: String,
    },
    /// An ephemeral PostgreSQL database, dropped once the verification ends.
    ///
    /// The sandbox carries live clients and a database name, so it is boxed to
    /// keep every value of this enum small regardless of the targeted engine.
    Postgres(Box<PostgresSandbox>),
}

impl SandboxKind {
    /// Opens the scratch state addressed by `scratch_url`.
    ///
    /// A file-backed SQLite scratch has to be empty, so a misconfigured
    /// `--scratch-url` never replays schema DDL on top of somebody's data.
    fn open(scratch_url: &str) -> Result<Self, StageError> {
        match DatabaseTarget::parse(scratch_url) {
            DatabaseTarget::Postgres => PostgresSandbox::connect(scratch_url)
                .map(Box::new)
                .map(SandboxKind::Postgres)
                .map_err(|message| StageError::Sandbox { message }),
            DatabaseTarget::SqliteMemory => open_sqlite_memory(scratch_url),
            DatabaseTarget::SqliteFile(path) => open_sqlite_file(&path, scratch_url),
        }
    }

    /// Throws the scratch state away once a side of the equation was introspected.
    ///
    /// The in-memory SQLite database closes with its connection, the file-backed
    /// one is deleted together with its write-ahead log, and the ephemeral
    /// PostgreSQL database is dropped with its sandbox. A script that committed
    /// half of its statements therefore leaves nothing behind for the next side.
    fn discard(self) -> Result<(), StageError> {
        match self {
            // Closing the connection disposes of the whole in-memory database.
            SandboxKind::SqliteMemory(_) => Ok(()),
            SandboxKind::SqliteFile { connection, path } => {
                drop(connection);
                remove_scratch_file(&path)
            }
            // The sandbox drops the ephemeral database together with its clients.
            SandboxKind::Postgres(_) => Ok(()),
        }
    }

    /// Applies one SQL script to the scratch state.
    ///
    /// A transactional script runs inside a transaction, so its statements commit
    /// together or not at all; a non-transactional one commits every statement on
    /// its own, exactly as the migration runner replays it in production.
    fn apply(&mut self, sql: &str, transactional: bool) -> Result<(), String> {
        match self {
            SandboxKind::SqliteMemory(connection) | SandboxKind::SqliteFile { connection, .. } => {
                apply_sqlite(connection, sql, transactional)
            }
            SandboxKind::Postgres(sandbox) => {
                if transactional {
                    sandbox.apply_transactional(sql)
                } else {
                    sandbox.apply_non_transactional(sql)
                }
            }
        }
    }

    /// Introspects the catalog currently materialised by the scratch state.
    fn catalog(&mut self) -> Result<SchemaCatalog, String> {
        match self {
            SandboxKind::SqliteMemory(connection) | SandboxKind::SqliteFile { connection, .. } => {
                introspect_sqlite(connection).map_err(|error| error.to_string())
            }
            SandboxKind::Postgres(sandbox) => sandbox.catalog(),
        }
    }
}

/// The user objects a SQLite scratch database currently holds.
const SQLITE_SCRATCH_OBJECTS: &str = "SELECT name FROM sqlite_master \
     WHERE type IN ('table', 'view') AND name NOT LIKE 'sqlite_%' \
     ORDER BY name";

/// Applies one SQL script to a SQLite scratch database under the declared mode.
fn apply_sqlite(
    connection: &mut rusqlite::Connection,
    sql: &str,
    transactional: bool,
) -> Result<(), String> {
    if !transactional {
        return connection
            .execute_batch(sql)
            .map_err(|error| error.to_string());
    }

    let transaction = connection
        .transaction()
        .map_err(|error| error.to_string())?;

    transaction
        .execute_batch(sql)
        .map_err(|error| error.to_string())?;

    transaction.commit().map_err(|error| error.to_string())
}

/// Opens the in-memory SQLite scratch database a bare `sqlite:` target names.
///
/// `:memory:`, an empty value, and a bare `sqlite:` prefix the target has
/// already reduced to nothing all end up here, so the replay never touches the
/// filesystem.
fn open_sqlite_memory(scratch_url: &str) -> Result<SandboxKind, StageError> {
    rusqlite::Connection::open_in_memory()
        .map(SandboxKind::SqliteMemory)
        .map_err(|error| StageError::Sandbox {
            message: format!(
                "failed to open the SQLite scratch database `{}`: {error}",
                redact_database_url(scratch_url)
            ),
        })
}

/// Opens the file-backed SQLite scratch database the target names.
///
/// The file is required to be empty, and it is deleted once the side that opened
/// it ends, so a file that outlives its verification can never feed one side of
/// the equation with the state of another.
fn open_sqlite_file(path: &str, scratch_url: &str) -> Result<SandboxKind, StageError> {
    let connection = rusqlite::Connection::open(path).map_err(|error| StageError::Sandbox {
        message: format!(
            "failed to open the SQLite scratch database `{}`: {error}",
            redact_database_url(scratch_url)
        ),
    })?;

    ensure_scratch_is_empty(&connection, scratch_url)?;

    Ok(SandboxKind::SqliteFile {
        connection,
        path: path.to_owned(),
    })
}

/// Refuses a scratch database that already holds user objects.
fn ensure_scratch_is_empty(
    connection: &rusqlite::Connection,
    scratch_url: &str,
) -> Result<(), StageError> {
    let objects = scratch_objects(connection)?;

    if objects.is_empty() {
        return Ok(());
    }

    let names: Vec<&str> = objects.iter().map(String::as_str).collect();

    Err(StageError::Sandbox {
        message: format!(
            "the SQLite scratch database `{}` is not empty (it holds {}); point \
             `--scratch-url` at a fresh file or at `:memory:`",
            redact_database_url(scratch_url),
            names.join(", ")
        ),
    })
}

/// The names of the user objects of a SQLite scratch database.
fn scratch_objects(connection: &rusqlite::Connection) -> Result<Vec<String>, StageError> {
    let mut statement = connection
        .prepare(SQLITE_SCRATCH_OBJECTS)
        .map_err(|error| StageError::Sandbox {
            message: format!("failed to inspect the SQLite scratch database: {error}"),
        })?;

    let objects = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| StageError::Sandbox {
            message: format!("failed to inspect the SQLite scratch database: {error}"),
        })?
        .collect::<Result<Vec<String>, _>>()
        .map_err(|error| StageError::Sandbox {
            message: format!("failed to read the SQLite scratch database: {error}"),
        })?;

    Ok(objects)
}

/// Deletes the file a scratch database was materialised in.
///
/// The write-ahead log and the shared-memory index are removed together with the
/// database itself, because a sidecar that outlives its database would otherwise
/// reach the file a later side creates under the same name.
fn remove_scratch_file(path: &str) -> Result<(), StageError> {
    for suffix in ["", "-wal", "-shm"] {
        let target = format!("{path}{suffix}");

        match std::fs::remove_file(&target) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(StageError::Sandbox {
                    message: format!(
                        "failed to delete the SQLite scratch database `{target}`: {error}"
                    ),
                })
            }
        }
    }

    Ok(())
}

/// One side of the Golden Equation: the declarative stage and the tail that must
/// reproduce it.
#[derive(Debug, Clone, Copy)]
pub struct GoldenEquation<'a> {
    base: Option<&'a DeclarativeStage>,
    checkpoint: Option<&'a Checkpoint>,
    stage: &'a DeclarativeStage,
    tail: &'a [Migration],
}

impl<'a> GoldenEquation<'a> {
    /// Creates the equation for a project whose pruned history is unavailable.
    ///
    /// `stage` is the declarative state (`schema/`) and `tail` the active
    /// migration scripts above the checkpoint, in whatever order they were read.
    pub fn new(stage: &'a DeclarativeStage, tail: &'a [Migration]) -> Self {
        Self {
            base: None,
            checkpoint: None,
            stage,
            tail,
        }
    }

    /// Anchors the replay to the stage snapshot the checkpoint was cut at.
    ///
    /// The base stage is replayed before the tail, so a tail of incremental
    /// `ALTER TABLE` scripts is verified against the schema it actually advances.
    pub fn with_base(mut self, base: &'a DeclarativeStage) -> Self {
        self.base = Some(base);
        self
    }

    /// Anchors the replay to a committed checkpoint boundary.
    ///
    /// Migrations at or below the pruned version are skipped during tail replay,
    /// and if a base stage is provided, its catalog hash is verified against
    /// the checkpoint hash. A checkpoint whose boundary is not the empty catalog
    /// requires a base stage: without one, [`verify`](Self::verify) rejects the
    /// equation with [`StageError::BaseStageMissing`] instead of replaying the
    /// filtered tail over an empty scratch database.
    pub fn with_checkpoint(mut self, checkpoint: &'a Checkpoint) -> Self {
        self.checkpoint = Some(checkpoint);
        self
    }

    /// Materialises both sides of the equation and compares their catalogs.
    ///
    /// Both states are built in the scratch database named by `scratch_url` and
    /// introspected through the same engine adapter the codegen toolchain uses, so
    /// the comparison is bit-for-bit at the catalog level. A file-backed scratch
    /// database has to be empty before the verification starts and is deleted again
    /// between the two sides. Any divergence is returned as
    /// [`StageError::Diverged`] carrying the full report.
    pub fn verify(&self, scratch_url: &str) -> Result<GoldenReport, StageError> {
        // A checkpoint anchors a boundary the replay has to start from: without a
        // base stage the filtered tail would be replayed over an empty scratch
        // database and report a consistent project as diverged. An empty
        // boundary is the one exception, because an empty scratch database
        // reproduces it on its own.
        if let Some(checkpoint) = self.checkpoint {
            if self.base.is_none()
                && checkpoint.schema_hash() != SchemaCatalog::default().checksum()
            {
                return Err(StageError::BaseStageMissing {
                    pruned_up_to_version: checkpoint.pruned_up_to_version().to_string(),
                });
            }
        }

        if let (Some(checkpoint), Some(base)) = (self.checkpoint, self.base) {
            let base_catalog = materialise_stage(base, scratch_url)?;
            if base_catalog.checksum() != checkpoint.schema_hash() {
                return Err(StageError::BaseStageHashMismatch {
                    expected: checkpoint.schema_hash().to_hex(),
                    actual: base_catalog.schema_hash(),
                });
            }
        }

        let stage_catalog = materialise_stage(self.stage, scratch_url)?;

        let replayed = Migrator::sort_migrations(self.tail);
        let mut tail_scripts: Vec<Script<'_>> = Vec::new();
        if let Some(base) = self.base {
            tail_scripts.extend(base.scripts().iter().map(|script| Script {
                label: script.name(),
                sql: script.sql(),
                transactional: script.is_transactional(),
            }));
        }
        tail_scripts.extend(
            replayed
                .iter()
                .filter(|migration| match (&self.checkpoint, &migration.version) {
                    (Some(checkpoint), Some(version)) => {
                        version > checkpoint.pruned_up_to_version()
                    }
                    _ => true,
                })
                .map(|migration| Script {
                    label: migration.script.as_str(),
                    sql: migration.sql.as_str(),
                    transactional: migration.is_transactional(),
                }),
        );
        let tail_catalog = materialise(scratch_url, &tail_scripts)?;

        let diff = compute_catalog_diff(&tail_catalog, &stage_catalog);
        let report = GoldenReport {
            stage_catalog,
            tail_catalog,
            diff,
        };

        if report.is_equivalent() {
            Ok(report)
        } else {
            Err(StageError::Diverged(Box::new(report)))
        }
    }
}

/// The materialised state of both sides of the equation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoldenReport {
    /// The catalog built from the declarative stage (`schema/`).
    pub stage_catalog: SchemaCatalog,
    /// The catalog built from the base stage plus the replayed migration tail.
    pub tail_catalog: SchemaCatalog,
    /// Structural differences between the replayed tail and the stage.
    pub diff: SchemaDiff,
}

impl GoldenReport {
    /// Whether both sides describe exactly the same schema.
    pub fn is_equivalent(&self) -> bool {
        self.diff.is_empty()
    }

    /// The `schema.lock` hash of the declarative stage.
    pub fn stage_hash(&self) -> String {
        self.stage_catalog.schema_hash()
    }

    /// The `schema.lock` hash of the replayed migration tail.
    pub fn tail_hash(&self) -> String {
        self.tail_catalog.schema_hash()
    }

    /// Renders every discrepancy, one finding per line.
    fn write_details(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.diff.write_details(f, DiffWording::GOLDEN)
    }
}

/// Failure of the Golden Equation verification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StageError {
    /// The scratch database could not be opened or introspected.
    Sandbox {
        /// The underlying driver message.
        message: String,
    },
    /// A stage script or a migration failed to apply in the scratch database.
    Script {
        /// The stage file name or the migration script name.
        script: String,
        /// The underlying driver message.
        message: String,
    },
    /// A checkpoint anchors a boundary no base stage was supplied to replay.
    ///
    /// Replaying the filtered tail over an empty scratch database would report a
    /// consistent project as diverged, so the boundary has to be reproduced from
    /// the snapshot `stage commit` records next to the marker, or from an
    /// explicit `--base-stage-dir`.
    BaseStageMissing {
        /// The pruned boundary the checkpoint anchors.
        pruned_up_to_version: String,
    },
    /// The base stage catalog hash does not match the checkpoint hash.
    BaseStageHashMismatch {
        /// The hash expected by the checkpoint.
        expected: String,
        /// The actual catalog hash of the replayed base stage.
        actual: String,
    },
    /// Both sides of the equation describe different schemas.
    Diverged(Box<GoldenReport>),
}

impl fmt::Display for StageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StageError::Sandbox { message } => {
                write!(f, "failed to prepare the scratch database: {message}")
            }
            StageError::Script { script, message } => write!(
                f,
                "failed to apply `{script}` in the scratch database: {message}"
            ),
            StageError::BaseStageMissing { pruned_up_to_version } => write!(
                f,
                "checkpoint pruned up to version {pruned_up_to_version} requires a base stage \
                 to reproduce the boundary, but none was supplied; re-run `stage commit` to \
                 snapshot the boundary next to `checkpoint.toml`, or pass `--base-stage-dir`"
            ),
            StageError::BaseStageHashMismatch { expected, actual } => write!(
                f,
                "base stage catalog hash mismatch: expected {expected}, got {actual}"
            ),
            StageError::Diverged(report) => {
                writeln!(
                    f,
                    "the golden equation does not hold: State(schema/) != State(Checkpoint + migrations/)"
                )?;
                writeln!(f, "  stage hash: {}", report.stage_hash())?;
                writeln!(f, "  tail hash:  {}", report.tail_hash())?;
                report.write_details(f)?;
                write!(
                    f,
                    "Hint: update `schema/` to match the replayed tail, or add the migration that closes the gap."
                )
            }
        }
    }
}

impl std::error::Error for StageError {}

/// Materialises the declarative `stage` in the disposable scratch database
/// addressed by `scratch_url` and introspects the resulting catalog.
pub fn materialise_stage(
    stage: &DeclarativeStage,
    scratch_url: &str,
) -> Result<SchemaCatalog, StageError> {
    let stage_scripts: Vec<Script<'_>> = stage
        .scripts()
        .iter()
        .map(|script| Script {
            label: script.name(),
            sql: script.sql(),
            transactional: script.is_transactional(),
        })
        .collect();
    materialise(scratch_url, &stage_scripts)
}

/// Materialises `scripts` in the scratch state and introspects the result.
///
/// A file-backed scratch database is deleted afterwards, so the next side of the
/// equation starts from a fresh file: no object, `PRAGMA`, or database-header
/// field the replay left behind can reach it.
fn materialise(scratch_url: &str, scripts: &[Script<'_>]) -> Result<SchemaCatalog, StageError> {
    let mut sandbox = SandboxKind::open(scratch_url)?;

    let replay = apply_scripts(&mut sandbox, scripts).and_then(|()| {
        sandbox
            .catalog()
            .map_err(|message| StageError::Sandbox { message })
    });

    // The sandbox is discarded whether or not the replay succeeded: a script that
    // stopped halfway would otherwise leave the part it committed — a table, a
    // header `PRAGMA`, a connection setting — for the next side to build on.
    let discard = sandbox.discard();

    match (replay, discard) {
        (Ok(catalog), Ok(())) => Ok(catalog),
        // The replay failed, so its diagnostic describes the real problem and the
        // discard merely happened on the way out.
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(error),
    }
}

/// Applies every script of one side to the scratch state in the given order.
fn apply_scripts(sandbox: &mut SandboxKind, scripts: &[Script<'_>]) -> Result<(), StageError> {
    for script in scripts {
        sandbox
            .apply(script.sql, script.transactional)
            .map_err(|message| StageError::Script {
                script: script.label.to_owned(),
                message,
            })?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurqe_migrate::checksum::Checksum;
    use rurqe_migrate::stage::StageScript;

    use crate::drift::TableDiff;

    const USERS: &str = "CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT NOT NULL);";

    fn stage(scripts: &[(&str, &str)]) -> DeclarativeStage {
        DeclarativeStage::new(
            scripts
                .iter()
                .map(|(name, sql)| StageScript::new(*name, *sql))
                .collect(),
        )
    }

    fn tail(scripts: &[(&str, &str)]) -> Vec<Migration> {
        scripts
            .iter()
            .map(|(name, sql)| Migration::parse(name, sql).expect("a valid script name"))
            .collect()
    }

    #[test]
    fn materialise_stage_introspects_catalog() {
        let stage = stage(&[("users.sql", USERS)]);
        let catalog =
            materialise_stage(&stage, ":memory:").expect("stage materialises in SQLite scratch");

        assert_eq!(catalog.tables.len(), 1);
        assert_eq!(catalog.tables[0].name, "users");
    }

    #[test]
    fn matching_stage_and_tail_are_equivalent() {
        let stage = stage(&[("users.sql", USERS)]);
        let tail = tail(&[("V1__users.sql", USERS)]);

        let report = GoldenEquation::new(&stage, &tail)
            .verify(":memory:")
            .expect("the stage and the tail describe the same schema");

        assert!(report.is_equivalent());
        assert_eq!(report.stage_hash(), report.tail_hash());
    }

    #[test]
    fn the_tail_must_create_every_stage_table() {
        let stage = stage(&[
            (
                "orders.sql",
                "CREATE TABLE orders (id INTEGER PRIMARY KEY);",
            ),
            ("users.sql", USERS),
        ]);
        let tail = tail(&[("V1__users.sql", USERS)]);

        let error = GoldenEquation::new(&stage, &tail)
            .verify(":memory:")
            .expect_err("`orders` is never created by the tail");

        match &error {
            StageError::Diverged(report) => {
                assert_eq!(report.diff.missing_tables, vec!["orders".to_owned()]);
            }
            other => panic!("expected Diverged, got {other:?}"),
        }
        assert!(error.to_string().contains("orders"));
        assert!(error.to_string().contains("golden equation"));
    }

    #[test]
    fn the_tail_must_declare_every_stage_column() {
        let stage = stage(&[("users.sql", USERS)]);
        let tail = tail(&[(
            "V1__users.sql",
            "CREATE TABLE users (id INTEGER PRIMARY KEY);",
        )]);

        match GoldenEquation::new(&stage, &tail).verify(":memory:") {
            Err(StageError::Diverged(report)) => {
                assert_eq!(
                    report.diff.table_diffs["users"].missing_columns,
                    vec!["email".to_owned()]
                );
            }
            other => panic!("expected a diverged report, got {other:?}"),
        }
    }

    #[test]
    fn differing_nullability_is_reported_as_an_altered_column() {
        let stage = stage(&[("users.sql", USERS)]);
        let tail = tail(&[(
            "V1__users.sql",
            "CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT);",
        )]);

        match GoldenEquation::new(&stage, &tail).verify(":memory:") {
            Err(StageError::Diverged(report)) => {
                assert_eq!(
                    report.diff.table_diffs["users"].altered_columns,
                    vec!["email".to_owned()]
                );
            }
            other => panic!("expected a diverged report, got {other:?}"),
        }
    }

    #[test]
    fn the_base_stage_anchors_the_replayed_tail() {
        let base = stage(&[("users.sql", "CREATE TABLE users (id INTEGER PRIMARY KEY);")]);
        let tail = tail(&[(
            "V2__email.sql",
            "ALTER TABLE users ADD COLUMN email TEXT NOT NULL DEFAULT '';",
        )]);
        let stage = stage(&[(
            "users.sql",
            "CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT NOT NULL DEFAULT '');",
        )]);

        let report = GoldenEquation::new(&stage, &tail)
            .with_base(&base)
            .verify(":memory:")
            .expect("the base plus the tail reproduces the stage");

        assert!(report.is_equivalent());
        assert_eq!(report.stage_hash(), report.tail_hash());
    }

    #[test]
    fn checkpoint_filters_outdated_tail_migrations() {
        let base = stage(&[("users.sql", "CREATE TABLE users (id INTEGER PRIMARY KEY);")]);
        let base_catalog = materialise_stage(&base, ":memory:").expect("base catalog");
        let cp = Checkpoint::new(
            "1.0".parse().expect("valid version"),
            base_catalog.checksum(),
        );

        // V1 would fail if applied on top of base because table users already exists
        let tail = tail(&[
            (
                "V1__users.sql",
                "CREATE TABLE users (id INTEGER PRIMARY KEY);",
            ),
            (
                "V2__email.sql",
                "ALTER TABLE users ADD COLUMN email TEXT NOT NULL DEFAULT '';",
            ),
            (
                "R__active_users.sql",
                "CREATE VIEW active_users AS SELECT id FROM users;",
            ),
        ]);
        // The repeatable script lives in the tail, so the stage has to declare
        // the view it creates as well.
        let stage = stage(&[
            (
                "users.sql",
                "CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT NOT NULL DEFAULT '');",
            ),
            (
                "views.sql",
                "CREATE VIEW active_users AS SELECT id FROM users;",
            ),
        ]);

        let report = GoldenEquation::new(&stage, &tail)
            .with_base(&base)
            .with_checkpoint(&cp)
            .verify(":memory:")
            .expect("checkpoint filters V1 and matches base hash");

        assert!(report.is_equivalent());
    }

    #[test]
    fn base_stage_hash_mismatch_is_rejected() {
        let base = stage(&[("users.sql", "CREATE TABLE users (id INTEGER PRIMARY KEY);")]);
        let cp = Checkpoint::new(
            "1.0".parse().expect("valid version"),
            Checksum::of(b"wrong_hash"),
        );

        let tail = tail(&[(
            "V2__email.sql",
            "ALTER TABLE users ADD COLUMN email TEXT NOT NULL DEFAULT '';",
        )]);
        let stage = stage(&[(
            "users.sql",
            "CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT NOT NULL DEFAULT '');",
        )]);

        let error = GoldenEquation::new(&stage, &tail)
            .with_base(&base)
            .with_checkpoint(&cp)
            .verify(":memory:")
            .expect_err("mismatched base stage hash must be rejected");

        match &error {
            StageError::BaseStageHashMismatch { expected, actual } => {
                assert_eq!(expected, &Checksum::of(b"wrong_hash").to_hex());
                assert_ne!(expected, actual);
            }
            other => panic!("expected BaseStageHashMismatch, got {other:?}"),
        }
        assert!(error
            .to_string()
            .contains("base stage catalog hash mismatch"));
    }

    #[test]
    fn a_checkpoint_without_a_base_stage_is_rejected() {
        let base = stage(&[("users.sql", "CREATE TABLE users (id INTEGER PRIMARY KEY);")]);
        let base_catalog =
            materialise_stage(&base, ":memory:").expect("the base stage materialises");
        let checkpoint = Checkpoint::new(
            "1".parse().expect("a valid version"),
            base_catalog.checksum(),
        );

        let stage = stage(&[(
            "users.sql",
            "CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT NOT NULL);",
        )]);
        let tail = tail(&[(
            "V2__email.sql",
            "ALTER TABLE users ADD COLUMN email TEXT NOT NULL DEFAULT '';",
        )]);

        let error = GoldenEquation::new(&stage, &tail)
            .with_checkpoint(&checkpoint)
            .verify(":memory:")
            .expect_err("the boundary is not reproduced by an empty scratch database");

        assert!(
            matches!(error, StageError::BaseStageMissing { .. }),
            "got {error:?}"
        );
        assert!(error.to_string().contains("base stage"), "got {error}");
    }

    #[test]
    fn an_empty_boundary_needs_no_base_stage() {
        // An empty scratch database reproduces an empty boundary on its own, so
        // requiring a snapshot for it would reject a valid empty cold start.
        let checkpoint = Checkpoint::new(
            "0".parse().expect("a valid version"),
            SchemaCatalog::default().checksum(),
        );

        let stage = stage(&[("users.sql", USERS)]);
        let tail = tail(&[("V1__users.sql", USERS)]);

        let report = GoldenEquation::new(&stage, &tail)
            .with_checkpoint(&checkpoint)
            .verify(":memory:")
            .expect("an empty boundary is reproduced by the empty scratch database");

        assert!(report.is_equivalent());
    }

    #[test]
    fn a_tail_without_its_base_cannot_be_replayed() {
        let stage = stage(&[("users.sql", USERS)]);
        let tail = tail(&[(
            "V2__email.sql",
            "ALTER TABLE users ADD COLUMN email TEXT NOT NULL DEFAULT '';",
        )]);

        let error = GoldenEquation::new(&stage, &tail)
            .verify(":memory:")
            .expect_err("the tail alters a table nothing created");

        assert!(matches!(error, StageError::Script { .. }), "got {error:?}");
        assert!(error.to_string().contains("V2__email.sql"));
    }

    #[test]
    fn a_repeatable_script_belongs_to_the_catalog_it_builds() {
        // The bindings avoid the names of the `stage` and `tail` helpers, because a
        // shadowing `let stage = stage(...)` would resolve the call to the value
        // already bound instead of the helper.
        let initial = stage(&[("users.sql", USERS)]);
        let tail = tail(&[
            ("V1__users.sql", USERS),
            (
                "R__active_users.sql",
                "CREATE VIEW active_users AS SELECT id FROM users;",
            ),
        ]);

        let error = GoldenEquation::new(&initial, &tail)
            .verify(":memory:")
            .expect_err("the stage never declares the view the tail creates");

        match &error {
            StageError::Diverged(report) => {
                assert_eq!(report.diff.extra_views, vec!["active_users".to_owned()]);
            }
            other => panic!("expected Diverged, got {other:?}"),
        }

        let declared = stage(&[
            ("users.sql", USERS),
            (
                "views.sql",
                "CREATE VIEW active_users AS SELECT id FROM users;",
            ),
        ]);

        let report = GoldenEquation::new(&declared, &tail)
            .verify(":memory:")
            .expect("the stage declares the view the tail creates");

        assert!(report.is_equivalent());
    }

    #[test]
    fn a_failing_script_is_reported_with_its_name() {
        let stage = stage(&[("users.sql", "CREATE TABLE users (;")]);

        let error = GoldenEquation::new(&stage, &[])
            .verify(":memory:")
            .expect_err("the stage DDL is malformed");

        assert!(matches!(error, StageError::Script { .. }), "got {error:?}");
        assert!(error.to_string().contains("users.sql"));
    }

    #[test]
    fn a_failed_replay_leaves_nothing_in_the_scratch_database() {
        let path = scratch_path("partial");
        let url = path.to_string_lossy().into_owned();

        // The first statement creates a table and the second one fails, so the
        // replay trips over DDL it has already committed to the scratch.
        let failing = stage(&[(
            "users.sql",
            "CREATE TABLE users (id INTEGER PRIMARY KEY);\n\
             CREATE TABLE users (id INTEGER PRIMARY KEY);",
        )]);

        let error = GoldenEquation::new(&failing, &[])
            .verify(&url)
            .expect_err("the stage cannot create the same table twice");

        assert!(matches!(error, StageError::Script { .. }), "got {error:?}");

        // The scratch database is empty again, so a sound stage still converges.
        let sound = stage(&[("users.sql", USERS)]);
        let tail = tail(&[("V1__users.sql", USERS)]);

        GoldenEquation::new(&sound, &tail)
            .verify(&url)
            .expect("the failed replay left no table behind");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn index_divergences_are_reported_by_name() {
        let catalog = SchemaCatalog::new(Vec::new());
        let mut diff = SchemaDiff::default();
        diff.table_diffs.insert(
            "users".to_owned(),
            TableDiff {
                missing_indexes: vec!["users_email_idx".to_owned()],
                extra_indexes: vec!["users_legacy_idx".to_owned()],
                altered_indexes: vec!["users_name_idx".to_owned()],
                ..TableDiff::default()
            },
        );

        let report = GoldenReport {
            stage_catalog: catalog.clone(),
            tail_catalog: catalog,
            diff,
        };

        assert!(!report.is_equivalent());

        let message = StageError::Diverged(Box::new(report)).to_string();
        assert!(message.contains("index"), "unexpected message: {message}");
        assert!(message.contains("users_email_idx"));
        assert!(message.contains("users_legacy_idx"));
        assert!(message.contains("users_name_idx"));
    }

    /// A unique, not-yet-created file scratch database for one test.
    fn scratch_path(label: &str) -> std::path::PathBuf {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let sequence = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        let mut path = std::env::temp_dir();
        path.push(format!(
            "rurqe-scratch-{label}-{}-{sequence}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        path
    }

    #[test]
    fn a_file_scratch_database_is_recreated_between_the_sides() {
        let path = scratch_path("reuse");
        let url = path.to_string_lossy().into_owned();
        let stage = stage(&[("users.sql", USERS)]);
        let tail = tail(&[("V1__users.sql", USERS)]);

        GoldenEquation::new(&stage, &tail)
            .verify(&url)
            .expect("both sides converge in the file scratch");

        GoldenEquation::new(&stage, &tail)
            .verify(&url)
            .expect("the deleted scratch is recreated by the next verification");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_file_scratch_database_keeps_no_state_between_the_sides() {
        let path = scratch_path("state");
        let url = path.to_string_lossy().into_owned();

        // `PRAGMA user_version` lives in the database header, so it outlives every
        // `DROP TABLE`, and the trigger disappears only together with its table. A
        // cleanup of `sqlite_master` would therefore carry both into the next side.
        let ddl = "PRAGMA user_version = 42;\n\
                   CREATE TABLE users (id INTEGER PRIMARY KEY);\n\
                   CREATE TRIGGER users_touch AFTER INSERT ON users BEGIN SELECT 1; END;";
        let stage = stage(&[("schema.sql", ddl)]);
        let tail = tail(&[("V1__schema.sql", ddl)]);

        GoldenEquation::new(&stage, &tail)
            .verify(&url)
            .expect("each side starts from a scratch that holds nothing");

        assert!(
            !path.exists(),
            "the file-backed scratch database is deleted after the verification"
        );
    }

    #[test]
    fn a_file_scratch_database_resets_with_foreign_keys_enabled() {
        let path = scratch_path("fk");
        let url = path.to_string_lossy().into_owned();
        let ddl = "PRAGMA foreign_keys = ON;\n\
                   CREATE TABLE customers (id INTEGER PRIMARY KEY);\n\
                   CREATE TABLE orders (id INTEGER PRIMARY KEY, customer_id INTEGER REFERENCES customers(id));";
        let stage = stage(&[("schema.sql", ddl)]);
        let tail = tail(&[("V1__schema.sql", ddl)]);

        GoldenEquation::new(&stage, &tail)
            .verify(&url)
            .expect("both sides converge with foreign keys enabled");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_dirty_scratch_database_is_rejected() {
        let path = scratch_path("dirty");
        {
            let connection = rusqlite::Connection::open(&path).expect("open the scratch");
            connection
                .execute_batch("CREATE TABLE leftover (id INTEGER PRIMARY KEY);")
                .expect("seed the scratch");
        }

        let url = path.to_string_lossy().into_owned();
        let stage = stage(&[("users.sql", USERS)]);
        let tail: Vec<Migration> = Vec::new();

        let error = GoldenEquation::new(&stage, &tail)
            .verify(&url)
            .expect_err("the scratch database is not empty");

        assert!(matches!(error, StageError::Sandbox { .. }), "got {error:?}");
        assert!(error.to_string().contains("leftover"), "got {error}");

        let _ = std::fs::remove_file(&path);
    }

    /// SQL whose first statement succeeds and whose second one always fails.
    const HALF_BROKEN: &str = "CREATE TABLE kept (id INTEGER PRIMARY KEY);\n\
                               CREATE TABLE kept (id INTEGER PRIMARY KEY);";

    #[test]
    fn a_failed_transactional_script_rolls_back_its_statements() {
        let mut sandbox = SandboxKind::open(":memory:").expect("an in-memory scratch opens");

        let error = sandbox
            .apply(HALF_BROKEN, true)
            .expect_err("the script cannot create the same table twice");
        assert!(error.contains("kept"), "unexpected message: {error}");

        let catalog = sandbox.catalog().expect("the scratch is introspectable");
        assert!(
            catalog.tables.is_empty(),
            "a transactional replay leaves nothing behind: {:?}",
            catalog.tables
        );
    }

    #[test]
    fn a_failed_non_transactional_script_keeps_what_it_committed() {
        let mut sandbox = SandboxKind::open(":memory:").expect("an in-memory scratch opens");

        sandbox
            .apply(HALF_BROKEN, false)
            .expect_err("the script cannot create the same table twice");

        let catalog = sandbox.catalog().expect("the scratch is introspectable");
        let names: Vec<&str> = catalog
            .tables
            .iter()
            .map(|table| table.name.as_str())
            .collect();

        assert_eq!(
            names,
            vec!["kept"],
            "a non-transactional replay commits statement by statement"
        );
    }

    #[test]
    fn a_non_transactional_stage_script_is_materialised() {
        let stage = DeclarativeStage::new(vec![
            StageScript::new("users.sql", USERS).with_transactional(false)
        ]);

        let catalog =
            materialise_stage(&stage, ":memory:").expect("the stage materialises in a scratch");

        assert_eq!(catalog.tables.len(), 1);
        assert_eq!(catalog.tables[0].name, "users");
    }
}
