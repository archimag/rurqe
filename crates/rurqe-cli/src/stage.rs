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
//! state — an in-memory SQLite database, or a PostgreSQL transaction that is
//! rolled back afterwards — introspects the resulting catalogs, and compares them
//! through the structural [`SchemaDiff`] of the drift verifier. Tables, columns,
//! defaults, nullability, and keys that drifted are reported as an actionable
//! diagnostic instead of surfacing as a production incident.
//!
//! The scratch state starts from nothing, so `database_url` must address an empty
//! database; SQLite is always materialised in memory and never touches the file
//! behind the URL, while a PostgreSQL database is only borrowed inside a
//! transaction that is rolled back. The tail passed to [`GoldenEquation::new`] is
//! the active set of scripts: migrations pruned below the checkpoint are already
//! folded into the base stage, so replaying them would collide with it. A project
//! that still keeps the stage snapshot of its checkpoint anchors the replay with
//! [`GoldenEquation::with_base`].
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

use rurqe_migrate::catalog::SchemaCatalog;
use rurqe_migrate::migration::Migration;
use rurqe_migrate::runner::Migrator;
use rurqe_migrate::stage::DeclarativeStage;

use crate::cli::{connect, redact_database_url, Connection};
use crate::drift::{compute_catalog_diff, SchemaDiff};
use crate::introspect::{detect_dialect, introspect_postgres, introspect_sqlite, DatabaseDialect};

/// One labelled SQL fragment applied to a scratch state.
struct Script<'a> {
    /// The file name of a stage script or the script name of a migration.
    label: &'a str,
    /// The SQL the fragment contributes.
    sql: &'a str,
}

/// A disposable database state one side of the equation is materialised in.
trait Sandbox {
    /// Applies one SQL script to the scratch state.
    fn apply(&mut self, sql: &str) -> Result<(), String>;

    /// Introspects the catalog currently materialised by the scratch state.
    fn catalog(&mut self) -> Result<SchemaCatalog, String>;
}

/// Scratch state backed by an in-memory SQLite database.
struct SqliteSandbox {
    connection: rusqlite::Connection,
}

impl SqliteSandbox {
    /// Opens a fresh in-memory database, so the file behind the URL is untouched.
    fn new() -> Result<Self, String> {
        rusqlite::Connection::open_in_memory()
            .map(|connection| Self { connection })
            .map_err(|error| error.to_string())
    }
}

impl Sandbox for SqliteSandbox {
    fn apply(&mut self, sql: &str) -> Result<(), String> {
        self.connection
            .execute_batch(sql)
            .map_err(|error| error.to_string())
    }

    fn catalog(&mut self) -> Result<SchemaCatalog, String> {
        introspect_sqlite(&self.connection).map_err(|error| error.to_string())
    }
}

/// Scratch state backed by a PostgreSQL transaction that is always rolled back.
struct PostgresSandbox {
    client: postgres::Client,
}

impl PostgresSandbox {
    /// Connects to `url` and opens the transaction the verification runs inside.
    fn connect(url: &str) -> Result<Self, String> {
        let mut client = match connect(url).map_err(|error| error.to_string())? {
            Connection::Postgres(client) => client,
            Connection::Sqlite(_) => {
                return Err(format!(
                    "`{}` does not address a PostgreSQL database",
                    redact_database_url(url)
                ))
            }
        };

        client
            .batch_execute("BEGIN")
            .map_err(|error| format!("failed to open a scratch transaction: {error}"))?;

        Ok(Self { client })
    }
}

impl Sandbox for PostgresSandbox {
    fn apply(&mut self, sql: &str) -> Result<(), String> {
        self.client
            .batch_execute(sql)
            .map_err(|error| error.to_string())
    }

    fn catalog(&mut self) -> Result<SchemaCatalog, String> {
        introspect_postgres(&mut self.client).map_err(|error| error.to_string())
    }
}

impl Drop for PostgresSandbox {
    fn drop(&mut self) {
        // Dropping the client closes the connection, which makes the server
        // discard the transaction anyway; the explicit rollback keeps the scratch
        // database untouched for as long as the connection stays open.
        let _ = self.client.batch_execute("ROLLBACK");
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
    /// Both states are built in a disposable scratch database and introspected
    /// through the same engine adapter the codegen toolchain uses, so the
    /// comparison is bit-for-bit at the catalog level. Any divergence is returned
    /// as [`StageError::Diverged`] carrying the full report.
    pub fn verify(&self, database_url: &str) -> Result<GoldenReport, StageError> {
        let stage_scripts: Vec<Script<'a>> = self
            .stage
            .scripts()
            .iter()
            .map(|script| Script {
                label: script.name(),
                sql: script.sql(),
            })
            .collect();
        let stage_catalog = materialise(database_url, &stage_scripts)?;

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
        let tail_catalog = materialise(database_url, &tail_scripts)?;

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
        let diff = &self.diff;

        for table in &diff.missing_tables {
            writeln!(
                f,
                "  - table `{table}` is declared by the stage but never created by the tail."
            )?;
        }
        for table in &diff.extra_tables {
            writeln!(
                f,
                "  - table `{table}` is created by the tail but missing from the stage."
            )?;
        }
        for (table, table_diff) in &diff.table_diffs {
            for column in &table_diff.missing_columns {
                writeln!(
                    f,
                    "  - table `{table}`: column `{column}` is declared by the stage but missing from the tail."
                )?;
            }
            for column in &table_diff.extra_columns {
                writeln!(
                    f,
                    "  - table `{table}`: column `{column}` is created by the tail but missing from the stage."
                )?;
            }
            for column in &table_diff.altered_columns {
                writeln!(
                    f,
                    "  - table `{table}`: column `{column}` differs between the stage and the tail."
                )?;
            }
            if table_diff.primary_key_mismatch {
                writeln!(
                    f,
                    "  - table `{table}`: the primary key differs between the stage and the tail."
                )?;
            }
            if table_diff.foreign_key_mismatch {
                writeln!(
                    f,
                    "  - table `{table}`: the foreign keys differ between the stage and the tail."
                )?;
            }
        }

        Ok(())
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

/// Opens the scratch state matching the dialect of `database_url`.
fn open_sandbox(database_url: &str) -> Result<Box<dyn Sandbox>, StageError> {
    match detect_dialect(Some(database_url)) {
        DatabaseDialect::Sqlite => {
            let sandbox = SqliteSandbox::new().map_err(|message| StageError::Sandbox {
                message: format!("failed to open an in-memory SQLite database: {message}"),
            })?;
            Ok(Box::new(sandbox))
        }
        DatabaseDialect::Postgres => {
            let sandbox = PostgresSandbox::connect(database_url)
                .map_err(|message| StageError::Sandbox { message })?;
            Ok(Box::new(sandbox))
        }
    }
}

/// Verifies that the scratch state starts empty.
///
/// The equation is materialised from nothing, so replaying it over an already
/// initialised database would collide with the existing schema; the check turns
/// that collision into an actionable message.
fn require_empty(sandbox: &mut dyn Sandbox) -> Result<(), StageError> {
    let existing = sandbox
        .catalog()
        .map_err(|message| StageError::Sandbox { message })?;

    if existing.tables.is_empty() {
        return Ok(());
    }

    Err(StageError::Sandbox {
        message: format!(
            "the scratch database already contains {} table(s); the golden equation is \
             materialised from an empty state, so point the verification at an empty database",
            existing.tables.len()
        ),
    })
}

/// Materialises `scripts` in a fresh scratch state and introspects the result.
fn materialise(database_url: &str, scripts: &[Script<'_>]) -> Result<SchemaCatalog, StageError> {
    let mut sandbox = open_sandbox(database_url)?;
    require_empty(sandbox.as_mut())?;

    for script in scripts {
        sandbox
            .apply(script.sql)
            .map_err(|message| StageError::Script {
                script: script.label.to_owned(),
                message,
            })?;
    }

    sandbox
        .catalog()
        .map_err(|message| StageError::Sandbox { message })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurqe_migrate::stage::StageScript;

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
}
