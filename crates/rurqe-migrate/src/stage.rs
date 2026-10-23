//! Cold start of a database from the declarative stage.
//!
//! A project following the Stage + Tail paradigm keeps the target schema as
//! plain DDL scripts in a stage directory (the [`DeclarativeStage::DIRECTORY`]
//! `schema/` by default) instead of replaying the historical migration tail.
//! Cold environments — CI runners, local development databases, container
//! spin-ups — are initialised straight from that stage: every script is applied
//! inside a single transaction and a synthetic stage marker is recorded in
//! [`SCHEMA_HISTORY_TABLE`](crate::history::SCHEMA_HISTORY_TABLE).
//!
//! The marker is a [`MigrationKind::Baseline`] entry carrying the [`StageMarker`]
//! version and the digest of the engine-neutral
//! [`SchemaCatalog`](crate::catalog::SchemaCatalog), so the migration runner
//! treats every versioned migration at or below the marked version as already
//! applied while the active tail above it still runs.
//!
//! ```
//! use rurqe_migrate::{
//!     cold_start, DeclarativeStage, InMemoryBackend, SchemaCatalog, StageMarker, StageScript,
//! };
//!
//! let stage = DeclarativeStage::new(vec![StageScript::new(
//!     "users.sql",
//!     "CREATE TABLE users (id INTEGER PRIMARY KEY);",
//! )]);
//! let catalog = SchemaCatalog::default();
//! let marker = StageMarker::from_catalog("3.2".parse().expect("a valid version"), &catalog);
//!
//! let mut backend = InMemoryBackend::new();
//! let report = cold_start(&mut backend, &stage, &marker).expect("the database is cold");
//!
//! assert_eq!(report.stage_scripts, vec!["users.sql"]);
//! assert_eq!(backend.history.len(), 1);
//! ```

use std::path::Path;

use crate::catalog::SchemaCatalog;
use crate::checksum::Checksum;
use crate::error::Error;
use crate::history::SchemaHistoryEntry;
use crate::migration::MigrationKind;
use crate::runner::{current_timestamp, LockGuard, MigrationBackend, MigrationTransaction};
use crate::version::MigrationVersion;

/// The synthetic script name the cold-start marker is recorded under.
pub const STAGE_SCRIPT: &str = "<< Stage >>";

/// One DDL script of the declarative stage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageScript {
    name: String,
    sql: String,
}

impl StageScript {
    /// Creates a script from its file name and DDL contents.
    pub fn new(name: impl Into<String>, sql: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            sql: sql.into(),
        }
    }

    /// The file name of the script inside the stage directory.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The DDL the script applies.
    pub fn sql(&self) -> &str {
        &self.sql
    }
}

/// The declarative stage: the DDL scripts describing the target schema.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeclarativeStage {
    scripts: Vec<StageScript>,
}

impl DeclarativeStage {
    /// The directory holding the DDL scripts of the stage.
    pub const DIRECTORY: &str = "schema";

    /// The file extension of a stage script.
    const EXTENSION: &str = "sql";

    /// Collects the scripts, sorting them by file name.
    ///
    /// Directories are read in whatever order the file system reports, so the
    /// scripts are ordered here to keep the applied DDL deterministic.
    pub fn new(scripts: Vec<StageScript>) -> Self {
        let mut scripts = scripts;
        scripts.sort_by(|left, right| left.name.cmp(&right.name));
        Self { scripts }
    }

    /// Reads every `*.sql` file of `dir`, returning `None` when it is absent.
    ///
    /// A missing directory means "this project has no declarative stage", while
    /// an unreadable file is an error: silently skipping a script would
    /// initialise the database with an incomplete schema.
    pub fn load(dir: &Path) -> Result<Option<Self>, Error> {
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(stage_error(format!(
                    "failed to read `{}`: {error}",
                    dir.display()
                )))
            }
        };

        let mut scripts = Vec::new();

        for entry in entries {
            let entry = entry.map_err(|error| {
                stage_error(format!("failed to read `{}`: {error}", dir.display()))
            })?;

            let path = entry.path();
            let is_script = path.extension().and_then(|extension| extension.to_str())
                == Some(Self::EXTENSION);

            if !is_script || !path.is_file() {
                continue;
            }

            let sql = std::fs::read_to_string(&path).map_err(|error| {
                stage_error(format!("failed to read `{}`: {error}", path.display()))
            })?;
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
                .to_owned();

            scripts.push(StageScript::new(name, sql));
        }

        Ok(Some(Self::new(scripts)))
    }

    /// The scripts of the stage in application order.
    pub fn scripts(&self) -> &[StageScript] {
        &self.scripts
    }

    /// Whether the stage declares no DDL at all.
    pub fn is_empty(&self) -> bool {
        self.scripts.is_empty()
    }

    /// The SHA-256 digest of the stage in application order.
    ///
    /// File names and bodies are hashed in the order they are applied, so
    /// reordering the directory never changes the digest while any edit of a
    /// script does. The digest anchors a cold start: a database initialised from
    /// a diverged stage is rejected instead of being silently overwritten.
    pub fn checksum(&self) -> Checksum {
        let mut digest = String::new();

        for script in &self.scripts {
            digest.push_str(&script.name);
            digest.push('\n');
            digest.push_str(&script.sql);
            digest.push('\n');
        }

        Checksum::of(digest.as_bytes())
    }
}

/// The synthetic history entry a cold start records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageMarker {
    version: MigrationVersion,
    description: String,
    checksum: Checksum,
}

impl StageMarker {
    /// The description recorded when the caller names no other one.
    pub const DESCRIPTION: &str = "stage";

    /// Creates a marker from the stage version, its description, and the
    /// digest of the stage it was cut at.
    pub fn new(
        version: MigrationVersion,
        description: impl Into<String>,
        checksum: Checksum,
    ) -> Self {
        Self {
            version,
            description: description.into(),
            checksum,
        }
    }

    /// Creates a marker carrying the digest of `catalog`.
    ///
    /// The catalog digest is the SHA-256 of the canonical `schema.lock`
    /// snapshot, so two cold starts of the same schema always record the same
    /// checksum.
    pub fn from_catalog(version: MigrationVersion, catalog: &SchemaCatalog) -> Self {
        Self::new(version, Self::DESCRIPTION, catalog.checksum())
    }

    /// The version every migration at or below is treated as passed.
    pub fn version(&self) -> &MigrationVersion {
        &self.version
    }

    /// The description recorded in the history table.
    pub fn description(&self) -> &str {
        &self.description
    }

    /// The digest of the stage the marker anchors.
    pub fn checksum(&self) -> Checksum {
        self.checksum
    }

    /// Builds the history row a cold start records over `checksum`.
    fn entry_with_checksum(
        &self,
        installed_rank: i64,
        installed_on: impl Into<String>,
        checksum: Checksum,
    ) -> SchemaHistoryEntry {
        SchemaHistoryEntry {
            installed_rank,
            version: Some(self.version.clone()),
            description: self.description.clone(),
            kind: MigrationKind::Baseline,
            script: STAGE_SCRIPT.to_owned(),
            checksum: Some(checksum),
            installed_on: installed_on.into(),
            execution_time_ms: 0,
            success: true,
        }
    }
}

/// Summary of a cold start run.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ColdStartReport {
    /// Names of the DDL scripts applied in this run, in application order.
    pub stage_scripts: Vec<String>,
    /// The rank the stage marker was recorded at, `None` when it already existed.
    pub installed_rank: Option<i64>,
    /// Total execution time of the DDL scripts in milliseconds.
    pub total_time_ms: i64,
}

/// Initialises an empty database from the declarative stage.
///
/// The DDL scripts of `stage` are applied in a single transaction — a failure
/// anywhere leaves the database untouched — and the resulting schema is anchored
/// by a synthetic [`StageMarker`] row in the schema history. Because the marker
/// is recorded as a baseline, every versioned migration at or below
/// [`StageMarker::version`] is reported as applied while the active tail above
/// it still runs.
///
/// The digest the marker anchors is the one the backend derives from the catalog
/// the stage produced — see [`MigrationTransaction::catalog_checksum`] — so it
/// matches the checkpoint `stage commit` writes whatever engine executed the
/// DDL. A backend that cannot introspect itself keeps the digest of the marker.
///
/// A database that already carries a stage marker for the same version and the
/// same catalog digest is left untouched and reported as a no-op; any other
/// history is rejected, because a cold start never overwrites an initialised
/// database.
pub fn cold_start<B: MigrationBackend>(
    backend: &mut B,
    stage: &DeclarativeStage,
    marker: &StageMarker,
) -> Result<ColdStartReport, Error> {
    let mut guard = LockGuard::acquire(backend)?;
    guard.ensure_schema_history_table()?;
    let history = guard.read_history()?;

    let recorded = history
        .iter()
        .filter(|entry| entry.success && entry.script == STAGE_SCRIPT)
        .max_by_key(|entry| entry.installed_rank);

    if let Some(existing) = recorded {
        // The backend reports the digest of the catalog really present, so a
        // database whose schema drifted from the recorded marker is rejected
        // instead of being reported as initialised. A backend that cannot
        // introspect itself keeps the digest the caller supplied.
        let current = match guard.catalog_checksum() {
            Ok(Some(checksum)) => checksum,
            Ok(None) => marker.checksum(),
            Err(error) => return Err(error),
        };

        if existing.checksum == Some(current) && existing.version.as_ref() == Some(&marker.version) {
            tracing::debug!(version = %marker.version, "stage is already recorded");
            return Ok(ColdStartReport::default());
        }

        // The database was initialised from a stage that no longer matches the
        // one on disk, so the two digests name different schemas and a cold start
        // would silently overwrite an initialised database.
        return Err(stage_error(format!(
            "the database already carries a stage marker for version `{}` with a diverged digest; \
             re-initialise it from scratch or commit the current stage first",
            existing
                .version
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_default()
        )));
    }

    if let Some(existing) = history.iter().max_by_key(|entry| entry.installed_rank) {
        return Err(Error::BaselineAlreadyExists {
            version: existing
                .version
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_else(|| existing.script.clone()),
        });
    }

    let mut tx = guard.begin()?;
    let start_time = std::time::Instant::now();

    for script in stage.scripts() {
        if let Err(error) = tx.execute(script.sql()) {
            let _ = tx.rollback();
            tracing::error!(script = %script.name(), error = %error, "stage script failed");
            return Err(Error::MigrationExecutionFailed {
                script: script.name().to_owned(),
                source: error.to_string(),
            });
        }
    }

    let elapsed_ms = start_time.elapsed().as_millis() as i64;

    // The marker anchors the digest of the catalog the stage produced, so the
    // checkpoint written by `stage commit` recognises the initialised database
    // whatever engine executed the DDL. A backend that cannot introspect itself
    // keeps the digest the caller supplied.
    let checksum = match tx.catalog_checksum() {
        Ok(Some(checksum)) => checksum,
        Ok(None) => marker.checksum(),
        Err(error) => {
            let _ = tx.rollback();
            return Err(error);
        }
    };

    let entry = marker.entry_with_checksum(1, current_timestamp(), checksum);

    if let Err(error) = tx.record_entry(&entry) {
        let _ = tx.rollback();
        return Err(error);
    }

    tx.commit()?;

    tracing::info!(
        version = %marker.version,
        scripts = stage.scripts().len(),
        elapsed_ms = elapsed_ms,
        "initialised the database from the declarative stage"
    );

    Ok(ColdStartReport {
        stage_scripts: stage
            .scripts()
            .iter()
            .map(|script| script.name().to_owned())
            .collect(),
        installed_rank: Some(entry.installed_rank),
        total_time_ms: elapsed_ms,
    })
}

/// Builds an [`Error::InvalidStage`] from `message`.
fn stage_error(message: impl Into<String>) -> Error {
    Error::InvalidStage {
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::create_schema_history_table_sql;
    use crate::migration::Migration;
    use crate::runner::InMemoryBackend;
    use std::path::PathBuf;

    const ORDERS: &str = "CREATE TABLE orders (id INTEGER);";
    const USERS: &str = "CREATE TABLE users (id INTEGER);";

    fn version(text: &str) -> MigrationVersion {
        text.parse().expect("a valid version")
    }

    fn stage() -> DeclarativeStage {
        DeclarativeStage::new(vec![
            StageScript::new("orders.sql", ORDERS),
            StageScript::new("users.sql", USERS),
        ])
    }

    fn marker() -> StageMarker {
        StageMarker::from_catalog(version("3.2"), &SchemaCatalog::default())
    }

    fn stage_directory(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "rurqe-stage-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create the stage directory");
        path
    }

    #[test]
    fn scripts_are_read_from_the_stage_directory() {
        let dir = stage_directory("load");
        std::fs::write(dir.join("users.sql"), USERS).expect("write the script");
        std::fs::write(dir.join("orders.sql"), ORDERS).expect("write the script");
        std::fs::write(dir.join("notes.txt"), "not a script").expect("write the note");

        let stage = DeclarativeStage::load(&dir)
            .expect("the stage is readable")
            .expect("the directory exists");
        let _ = std::fs::remove_dir_all(&dir);

        let names: Vec<&str> = stage.scripts().iter().map(StageScript::name).collect();
        assert_eq!(names, vec!["orders.sql", "users.sql"]);
        assert_eq!(stage.scripts()[0].sql(), ORDERS);
    }

    #[test]
    fn a_missing_stage_directory_is_not_an_error() {
        let dir = stage_directory("absent");
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(DeclarativeStage::load(&dir), Ok(None));
    }

    #[test]
    fn scripts_are_applied_in_name_order() {
        let stage = DeclarativeStage::new(vec![
            StageScript::new("users.sql", USERS),
            StageScript::new("orders.sql", ORDERS),
        ]);

        let names: Vec<&str> = stage.scripts().iter().map(StageScript::name).collect();
        assert_eq!(names, vec!["orders.sql", "users.sql"]);
    }

    #[test]
    fn the_stage_digest_follows_names_and_bodies() {
        let stage = stage();
        let reordered = DeclarativeStage::new(vec![
            StageScript::new("users.sql", USERS),
            StageScript::new("orders.sql", ORDERS),
        ]);
        let changed = DeclarativeStage::new(vec![StageScript::new(
            "users.sql",
            "CREATE TABLE users (id BIGINT);",
        )]);

        assert_eq!(reordered.checksum(), stage.checksum());
        assert_ne!(changed.checksum(), stage.checksum());
    }

    #[test]
    fn cold_start_applies_the_stage_in_a_single_transaction() {
        let mut backend = InMemoryBackend::new();
        let catalog = SchemaCatalog::default();

        let report = cold_start(&mut backend, &stage(), &marker()).expect("the database is cold");

        assert_eq!(report.stage_scripts, vec!["orders.sql", "users.sql"]);
        assert_eq!(report.installed_rank, Some(1));

        assert_eq!(backend.executed_sql.len(), 3);
        assert_eq!(backend.executed_sql[0], create_schema_history_table_sql());
        assert_eq!(backend.executed_sql[1], ORDERS);
        assert_eq!(backend.executed_sql[2], USERS);

        assert_eq!(backend.history.len(), 1);
        let entry = &backend.history[0];
        assert_eq!(entry.installed_rank, 1);
        assert_eq!(
            entry.version.as_ref().map(ToString::to_string).as_deref(),
            Some("3.2")
        );
        assert_eq!(entry.description, StageMarker::DESCRIPTION);
        assert_eq!(entry.kind, MigrationKind::Baseline);
        assert_eq!(entry.script, STAGE_SCRIPT);
        assert_eq!(entry.checksum, Some(catalog.checksum()));
        assert!(entry.success);
    }

    #[test]
    fn repeating_the_same_stage_is_a_no_op() {
        let mut backend = InMemoryBackend::new();
        cold_start(&mut backend, &stage(), &marker())
            .expect("the first run initialises the schema");

        let report = cold_start(&mut backend, &stage(), &marker())
            .expect("the recorded stage is recognised");

        assert_eq!(report, ColdStartReport::default());
        assert_eq!(backend.history.len(), 1);
        let ddl_runs = backend
            .executed_sql
            .iter()
            .filter(|sql| sql.as_str() == USERS || sql.as_str() == ORDERS)
            .count();
        assert_eq!(ddl_runs, 2);
    }

    #[test]
    fn an_initialised_database_is_never_overwritten() {
        let mut backend = InMemoryBackend::new();
        let migration = Migration::parse("V1__init.sql", USERS).expect("a valid script name");
        backend.history.push(SchemaHistoryEntry::applied(
            1,
            &migration,
            "2024-01-01 00:00:00",
            1,
        ));

        let error = cold_start(&mut backend, &stage(), &marker())
            .expect_err("an initialised database is rejected");

        assert!(matches!(error, Error::BaselineAlreadyExists { .. }));
        assert_eq!(backend.history.len(), 1);
    }

    #[test]
    fn a_diverged_stage_digest_is_rejected() {
        let mut backend = InMemoryBackend::new();
        cold_start(&mut backend, &stage(), &marker())
            .expect("the first run initialises the schema");

        let changed =
            StageMarker::new(version("3.2"), StageMarker::DESCRIPTION, Checksum::of(b"other"));
        let error = cold_start(&mut backend, &stage(), &changed)
            .expect_err("a diverged stage cannot masquerade as a cold start");

        assert!(matches!(error, Error::InvalidStage { .. }));
    }

    #[test]
    fn a_failing_script_aborts_the_cold_start() {
        let mut backend = InMemoryBackend::new();
        backend.failure_script = Some("BOOM".to_owned());
        let failing = DeclarativeStage::new(vec![
            StageScript::new("orders.sql", ORDERS),
            StageScript::new("users.sql", "BOOM;"),
        ]);

        let error = cold_start(&mut backend, &failing, &marker())
            .expect_err("the failing script aborts the cold start");

        assert!(matches!(error, Error::MigrationExecutionFailed { .. }));
        assert!(backend.history.is_empty());
        assert_eq!(backend.executed_sql.len(), 1);
        assert_eq!(backend.executed_sql[0], create_schema_history_table_sql());
    }

    #[test]
    fn the_marker_carries_the_catalog_digest() {
        let catalog = SchemaCatalog::default();
        let marker = StageMarker::from_catalog(version("3.2"), &catalog);

        assert_eq!(marker.version().to_string(), "3.2");
        assert_eq!(marker.description(), StageMarker::DESCRIPTION);
        assert_eq!(marker.checksum(), catalog.checksum());
    }

    #[test]
    fn cold_start_acquires_and_releases_the_backend_lock() {
        let mut backend = InMemoryBackend::new();
        assert!(!backend.locked);

        let report = cold_start(&mut backend, &stage(), &marker()).expect("cold start succeeds");
        assert_eq!(report.installed_rank, Some(1));
        assert!(!backend.locked);

        backend.acquire_lock().expect("lock acquired");
        let error = cold_start(&mut backend, &stage(), &marker())
            .expect_err("cold start must fail when backend is locked");
        assert!(matches!(error, Error::Backend(_)));
        backend.release_lock().expect("lock released");
        assert!(!backend.locked);

        let mut failing_backend = InMemoryBackend::new();
        failing_backend.failure_script = Some("BOOM".to_owned());
        let failing = DeclarativeStage::new(vec![StageScript::new("users.sql", "BOOM;")]);
        let error = cold_start(&mut failing_backend, &failing, &marker())
            .expect_err("cold start fails on bad script");
        assert!(matches!(error, Error::MigrationExecutionFailed { .. }));
        assert!(!failing_backend.locked);
    }
}
