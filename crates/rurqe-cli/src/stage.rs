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
//! The scratch state starts from nothing and is emptied again between the two
//! sides. `--scratch-url` selects it: a `sqlite:` URL or a `*.db` / `*.sqlite` /
//! `*.sqlite3` path is materialised in memory (`:memory:`) or in a file the
//! verification requires to be empty, while a PostgreSQL URL creates a throwaway
//! `rurqe_scratch_…` database on the server behind the URL, builds both sides
//! inside it, and drops it once the verification ends. Neither side runs inside a
//! transaction block, so statements such as `CREATE INDEX CONCURRENTLY` stay
//! replayable, and the existing contents of any other database stay untouched. The
//! connected role therefore needs the `CREATEDB` privilege for the duration of the
//! verification. The tail passed to
//! [`GoldenEquation::new`] is the active set of scripts: migrations pruned below
//! the checkpoint are already folded into the base stage, so replaying them would
//! collide with it. A project that still keeps the stage snapshot of its
//! checkpoint anchors the replay with [`GoldenEquation::with_base`].
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

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

use rurqe_migrate::catalog::SchemaCatalog;
use rurqe_migrate::migration::Migration;
use rurqe_migrate::runner::Migrator;
use rurqe_migrate::stage::DeclarativeStage;

use crate::cli::{connect, redact_database_url, Connection};
use crate::drift::{compute_catalog_diff, DiffWording, SchemaDiff};
use crate::introspect::{
    detect_dialect, introspect_postgres, introspect_sqlite, DatabaseDialect,
};

/// One labelled SQL fragment applied to a scratch state.
struct Script<'a> {
    /// The file name of a stage script or the script name of a migration.
    label: &'a str,
    /// The SQL the fragment contributes.
    sql: &'a str,
}

/// The disposable database state one side of the equation is materialised in.
///
/// The variants are dispatched statically, so a verification pays for exactly
/// the engine it targets and no trait object sits on the hot path.
enum SandboxKind {
    /// An in-memory SQLite database, so the file behind the URL is untouched.
    Sqlite(rusqlite::Connection),
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
        match detect_dialect(Some(scratch_url)) {
            DatabaseDialect::Sqlite => open_sqlite_scratch(scratch_url),
            DatabaseDialect::Postgres => PostgresSandbox::connect(scratch_url)
                .map(Box::new)
                .map(SandboxKind::Postgres)
                .map_err(|message| StageError::Sandbox { message }),
        }
    }

    /// Empties the scratch state once a side of the equation was introspected.
    ///
    /// A file-backed SQLite scratch is reused by the next side, so everything the
    /// replay created is dropped again. The ephemeral PostgreSQL database is
    /// dropped together with its sandbox, so nothing is left to undo there.
    fn reset(&mut self) -> Result<(), StageError> {
        match self {
            SandboxKind::Sqlite(connection) => reset_sqlite_scratch(connection),
            SandboxKind::Postgres(_) => Ok(()),
        }
    }

    /// Applies one SQL script to the scratch state.
    fn apply(&mut self, sql: &str) -> Result<(), String> {
        match self {
            SandboxKind::Sqlite(connection) => {
                connection.execute_batch(sql).map_err(|error| error.to_string())
            }
            SandboxKind::Postgres(sandbox) => sandbox.apply(sql),
        }
    }

    /// Introspects the catalog currently materialised by the scratch state.
    fn catalog(&mut self) -> Result<SchemaCatalog, String> {
        match self {
            SandboxKind::Sqlite(connection) => {
                introspect_sqlite(connection).map_err(|error| error.to_string())
            }
            SandboxKind::Postgres(sandbox) => sandbox.catalog(),
        }
    }
}

/// The user objects a SQLite scratch database currently holds.
const SQLITE_SCRATCH_OBJECTS: &str = "SELECT type, name FROM sqlite_master \
     WHERE type IN ('table', 'view') AND name NOT LIKE 'sqlite_%' \
     ORDER BY name";

/// Opens the SQLite scratch database addressed by `scratch_url`.
///
/// Without a path — `:memory:`, an empty value, or a bare `sqlite:` prefix — the
/// replay happens in memory and never touches the filesystem. A real path opens
/// that file and is required to be empty.
fn open_sqlite_scratch(scratch_url: &str) -> Result<SandboxKind, StageError> {
    let path = scratch_url
        .strip_prefix("sqlite://")
        .or_else(|| scratch_url.strip_prefix("sqlite:"))
        .unwrap_or(scratch_url);

    let in_memory = path.is_empty() || path == ":memory:";

    let connection = if in_memory {
        rusqlite::Connection::open_in_memory()
    } else {
        rusqlite::Connection::open(path)
    }
    .map_err(|error| StageError::Sandbox {
        message: format!(
            "failed to open the SQLite scratch database `{}`: {error}",
            redact_database_url(scratch_url)
        ),
    })?;

    if !in_memory {
        ensure_scratch_is_empty(&connection, scratch_url)?;
    }

    Ok(SandboxKind::Sqlite(connection))
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

    let names: Vec<&str> = objects.iter().map(|(_, name)| name.as_str()).collect();

    Err(StageError::Sandbox {
        message: format!(
            "the SQLite scratch database `{}` is not empty (it holds {}); point \
             `--scratch-url` at a fresh file or at `:memory:`",
            redact_database_url(scratch_url),
            names.join(", ")
        ),
    })
}

/// The `(kind, name)` pairs of the user objects of a SQLite scratch database.
fn scratch_objects(connection: &rusqlite::Connection) -> Result<Vec<(String, String)>, StageError> {
    let mut statement =
        connection
            .prepare(SQLITE_SCRATCH_OBJECTS)
            .map_err(|error| StageError::Sandbox {
                message: format!("failed to inspect the SQLite scratch database: {error}"),
            })?;

    let objects = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(|error| StageError::Sandbox {
            message: format!("failed to inspect the SQLite scratch database: {error}"),
        })?
        .collect::<Result<Vec<(String, String)>, _>>()
        .map_err(|error| StageError::Sandbox {
            message: format!("failed to read the SQLite scratch database: {error}"),
        })?;

    Ok(objects)
}

/// Drops every user object the replay left in a SQLite scratch database.
fn reset_sqlite_scratch(connection: &rusqlite::Connection) -> Result<(), StageError> {
    let foreign_keys_enabled = connection
        .query_row("PRAGMA foreign_keys", [], |row| row.get::<_, i64>(0))
        .map(|value| value != 0)
        .unwrap_or(false);

    if foreign_keys_enabled {
        connection
            .execute_batch("PRAGMA foreign_keys = OFF;")
            .map_err(|error| StageError::Sandbox {
                message: format!("failed to disable foreign keys in the SQLite scratch database: {error}"),
            })?;
    }

    let drop_result = (|| -> Result<(), StageError> {
        for (kind, name) in scratch_objects(connection)? {
            let quoted = name.replace('"', "\"\"");
            let statement = if kind == "view" {
                format!("DROP VIEW IF EXISTS \"{quoted}\"")
            } else {
                format!("DROP TABLE IF EXISTS \"{quoted}\"")
            };

            connection
                .execute_batch(&statement)
                .map_err(|error| StageError::Sandbox {
                    message: format!("failed to empty the SQLite scratch database: {error}"),
                })?;
        }
        Ok(())
    })();

    let restore_result = if foreign_keys_enabled {
        connection
            .execute_batch("PRAGMA foreign_keys = ON;")
            .map_err(|error| StageError::Sandbox {
                message: format!("failed to restore foreign keys in the SQLite scratch database: {error}"),
            })
    } else {
        Ok(())
    };

    drop_result.and(restore_result)
}

/// Scratch state backed by an ephemeral PostgreSQL database of its own.
///
/// Every verification creates a freshly named database on the server behind the
/// configured URL, connects to it, and replays both sides of the equation there.
/// A database is a hard isolation boundary, so nothing the replay does can reach
/// the user's data and no other session observes the scratch tables. Dropping the
/// sandbox removes the database together with everything built inside it.
///
/// Creating a database is a server-level operation, so the connected role needs
/// the `CREATEDB` privilege; a missing privilege is reported as an actionable
/// message instead of an opaque driver error.
struct PostgresSandbox {
    /// The server-level connection the scratch database is created and dropped through.
    admin: postgres::Client,
    /// The replay connection, closed before the scratch database is dropped.
    client: Option<postgres::Client>,
    /// The generated name of the scratch database.
    database: String,
}

/// Hands out a scratch database name no concurrent verification can collide with.
///
/// The process id and the monotonic counter separate the verifications of one
/// process, while the nanosecond timestamp separates concurrent processes that
/// happen to share a process id on different hosts.
fn scratch_database_name() -> String {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);

    format!(
        "rurqe_scratch_{}_{nanos}_{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )
}

impl PostgresSandbox {
    /// Connects to `url` and creates the scratch database the verification runs in.
    fn connect(url: &str) -> Result<Self, String> {
        let mut admin = match connect(url).map_err(|error| error.to_string())? {
            Connection::Postgres(client) => client,
            Connection::Sqlite(_) => {
                return Err(format!(
                    "`{}` does not address a PostgreSQL database",
                    redact_database_url(url)
                ))
            }
        };

        let config: postgres::Config = url.parse().map_err(|error| {
            format!(
                "failed to read `{}` as a PostgreSQL connection URL: {error}",
                redact_database_url(url)
            )
        })?;

        let database = scratch_database_name();
        let create = format!("CREATE DATABASE \"{database}\"");

        // Database names cannot be bound as parameters, but the generated name only
        // carries ASCII alphanumerics and underscores, so rendering it is
        // injection-free.
        if let Err(error) = admin.batch_execute(&create) {
            return Err(format!(
                "failed to create the scratch database on `{}`: {error}; the connected role \
                 needs the CREATEDB privilege",
                redact_database_url(url)
            ));
        }

        match attach_scratch_database(config, &database) {
            Ok(client) => Ok(Self {
                admin,
                client: Some(client),
                database,
            }),
            Err(message) => {
                drop_scratch_database(&mut admin, &database);
                Err(message)
            }
        }
    }

    /// Applies one SQL script inside the scratch database.
    fn apply(&mut self, sql: &str) -> Result<(), String> {
        self.replay_connection()?
            .batch_execute(sql)
            .map_err(|error| error.to_string())
    }

    /// Introspects the `public` schema of the scratch database only.
    fn catalog(&mut self) -> Result<SchemaCatalog, String> {
        introspect_postgres(self.replay_connection()?).map_err(|error| error.to_string())
    }

    /// The still-open replay connection.
    fn replay_connection(&mut self) -> Result<&mut postgres::Client, String> {
        self.client
            .as_mut()
            .ok_or_else(|| format!("the scratch database `{}` is already closed", self.database))
    }
}

/// Opens the replay connection to the freshly created scratch database.
///
/// The replay is introspected through the `public` schema of the scratch
/// database, so the search path is pinned instead of following a schema named
/// after the connected role.
fn attach_scratch_database(
    mut config: postgres::Config,
    database: &str,
) -> Result<postgres::Client, String> {
    config.dbname(database);

    let mut client = config
        .connect(postgres::NoTls)
        .map_err(|error| format!("failed to connect to the scratch database: {error}"))?;

    client
        .batch_execute("SET search_path TO public")
        .map_err(|error| format!("failed to prepare the scratch database: {error}"))?;

    Ok(client)
}

/// Drops a scratch database, removing the data the verification built inside it.
///
/// `WITH (FORCE)` terminates sessions still attached to the database and needs
/// PostgreSQL 13 or newer, so a plain drop is the fallback for older servers.
fn drop_scratch_database(admin: &mut postgres::Client, database: &str) {
    let forced = format!("DROP DATABASE IF EXISTS \"{database}\" WITH (FORCE)");

    if admin.batch_execute(&forced).is_err() {
        let plain = format!("DROP DATABASE IF EXISTS \"{database}\"");
        let _ = admin.batch_execute(&plain);
    }
}

impl Drop for PostgresSandbox {
    fn drop(&mut self) {
        // The replay connection is closed first, so a failed replay that left a
        // transaction open cannot keep the database alive on servers without the
        // `WITH (FORCE)` clause.
        if let Some(client) = self.client.take() {
            let _ = client.close();
        }

        drop_scratch_database(&mut self.admin, &self.database);
    }
}

/// One side of the Golden Equation: the declarative stage and the tail that must
/// reproduce it.
#[derive(Debug, Clone, Copy)]
pub struct GoldenEquation<'a> {
    base: Option<&'a DeclarativeStage>,
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

    /// Materialises both sides of the equation and compares their catalogs.
    ///
    /// Both states are built in the scratch database named by `scratch_url` and
    /// introspected through the same engine adapter the codegen toolchain uses, so
    /// the comparison is bit-for-bit at the catalog level. A file-backed scratch
    /// database has to be empty before the verification starts and is emptied again
    /// between the two sides. Any divergence is returned as
    /// [`StageError::Diverged`] carrying the full report.
    pub fn verify(&self, scratch_url: &str) -> Result<GoldenReport, StageError> {
        let stage_catalog = materialise_stage(self.stage, scratch_url)?;

        let replayed = Migrator::sort_migrations(self.tail);
        let mut tail_scripts: Vec<Script<'_>> = Vec::new();
        if let Some(base) = self.base {
            tail_scripts.extend(base.scripts().iter().map(|script| Script {
                label: script.name(),
                sql: script.sql(),
            }));
        }
        tail_scripts.extend(replayed.iter().map(|migration| Script {
            label: migration.script.as_str(),
            sql: migration.sql.as_str(),
        }));
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
        })
        .collect();
    materialise(scratch_url, &stage_scripts)
}

/// Materialises `scripts` in the scratch state and introspects the result.
///
/// The scratch state is emptied again afterwards, so a file-backed scratch
/// database is reusable by the next side of the equation.
fn materialise(scratch_url: &str, scripts: &[Script<'_>]) -> Result<SchemaCatalog, StageError> {
    let mut sandbox = SandboxKind::open(scratch_url)?;

    let replay = apply_scripts(&mut sandbox, scripts)
        .and_then(|()| sandbox.catalog().map_err(|message| StageError::Sandbox { message }));

    // A file-backed scratch database is reused by the next side of the equation,
    // so the replay is undone whether or not it succeeded: a replay that stopped
    // halfway would otherwise leave its tables for that side to build on.
    let reset = sandbox.reset();

    match (replay, reset) {
        (Ok(catalog), Ok(())) => Ok(catalog),
        // The replay failed, so its diagnostic describes the real problem and the
        // reset merely happened on the way out.
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(error),
    }
}

/// Applies every script of one side to the scratch state in the given order.
fn apply_scripts(sandbox: &mut SandboxKind, scripts: &[Script<'_>]) -> Result<(), StageError> {
    for script in scripts {
        sandbox
            .apply(script.sql)
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
        let catalog = materialise_stage(&stage, ":memory:")
            .expect("stage materialises in SQLite scratch");

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
            ("orders.sql", "CREATE TABLE orders (id INTEGER PRIMARY KEY);"),
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
    fn repeatable_scripts_stay_outside_the_catalog() {
        let stage = stage(&[("users.sql", USERS)]);
        let tail = tail(&[
            ("V1__users.sql", USERS),
            (
                "R__active_users.sql",
                "CREATE VIEW active_users AS SELECT id FROM users;",
            ),
        ]);

        let report = GoldenEquation::new(&stage, &tail)
            .verify(":memory:")
            .expect("views are not part of the catalog");

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
    fn a_file_scratch_database_is_emptied_between_the_sides() {
        let path = scratch_path("reuse");
        let url = path.to_string_lossy().into_owned();
        let stage = stage(&[("users.sql", USERS)]);
        let tail = tail(&[("V1__users.sql", USERS)]);

        GoldenEquation::new(&stage, &tail)
            .verify(&url)
            .expect("both sides converge in the file scratch");

        GoldenEquation::new(&stage, &tail)
            .verify(&url)
            .expect("the emptied scratch is reusable");

        let _ = std::fs::remove_file(&path);
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
}
