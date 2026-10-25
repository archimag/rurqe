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
//! A script marked non-transactional runs outside any transaction, so a stage
//! holding one cannot be applied atomically: the cold start then commits an
//! in-progress marker before the first such script and replaces it with the
//! applied one once every script has run. An interrupted cold start therefore
//! leaves a database whose partial initialisation is reported by the next
//! attempt instead of a schema the history knows nothing about.
//!
//! # Blocking entry points
//!
//! [`cold_start`] is a blocking lifecycle entry point, exactly like
//! [`Migrator::migrate`](crate::runner::Migrator::migrate),
//! [`Migrator::baseline`](crate::runner::Migrator::baseline),
//! [`Migrator::repair`](crate::runner::Migrator::repair), and
//! [`Migrator::status`](crate::runner::Migrator::status), which
//! [`EmbeddedMigrations`](crate::embedded::EmbeddedMigrations) mirrors in its
//! synchronous methods. Every one of them emits the runtime advisory through
//! [`warn_if_in_tokio_runtime`](crate::sleeper::warn_if_in_tokio_runtime) before
//! blocking the calling thread, so an application that embeds the lifecycle into
//! an asynchronous startup path is told to offload it through `spawn_blocking`.
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
use crate::migration::{parse_script_attributes, MigrationKind};
use crate::runner::{current_timestamp, LockGuard, MigrationBackend, MigrationTransaction};
use crate::scripts::sql_scripts;
use crate::sleeper::warn_if_in_tokio_runtime;
use crate::version::MigrationVersion;

/// The synthetic script name the cold-start marker is recorded under.
pub const STAGE_SCRIPT: &str = "<< Stage >>";

/// One DDL script of the declarative stage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageScript {
    name: String,
    sql: String,
    transactional: bool,
}

impl StageScript {
    /// Creates a script from its file name and DDL contents.
    ///
    /// The script is transactional unless its name or its comments declare
    /// otherwise, following the same attribute parser the migration tail uses.
    pub fn new(name: impl Into<String>, sql: impl Into<String>) -> Self {
        let name = name.into();
        let sql = sql.into();
        let transactional = parse_script_attributes(&name, &sql).is_transactional();

        Self {
            name,
            sql,
            transactional,
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

    /// Whether the script runs inside the transaction the cold start opens.
    pub fn is_transactional(&self) -> bool {
        self.transactional
    }

    /// Overrides whether the script runs inside a transaction.
    pub fn with_transactional(mut self, transactional: bool) -> Self {
        self.transactional = transactional;
        self
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
        let scripts = match sql_scripts(dir)? {
            Some(scripts) => scripts,
            None => return Ok(None),
        };

        let mut stage = Vec::with_capacity(scripts.len());

        for script in scripts {
            stage.push(StageScript::new(script.name(), script.read()?));
        }

        Ok(Some(Self::new(stage)))
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
    ///
    /// `success` tells the applied marker apart from the in-progress one a cold
    /// start commits before it runs a stage holding a non-transactional script.
    fn entry(
        &self,
        installed_rank: i64,
        installed_on: impl Into<String>,
        checksum: Checksum,
        success: bool,
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
            success,
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
/// The transactional DDL scripts of `stage` share one transaction — a failure
/// among them leaves the database untouched — while a script marked
/// non-transactional runs on its own, outside any transaction. The resulting
/// schema is anchored
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
///
/// A stage holding a script marked non-transactional cannot be applied
/// atomically, so the cold start commits an in-progress marker before the first
/// of those scripts and replaces it with the applied marker once every script has
/// run. A run interrupted in between leaves the in-progress marker behind, and
/// the next attempt reports the database as partially initialised instead of
/// treating it as an unknown schema or as one that already carries history.
///
/// The call blocks the calling thread for as long as the DDL takes, so it
/// reports the runtime advisory described in the module documentation before
/// reaching the backend.
pub fn cold_start<B: MigrationBackend>(
    backend: &mut B,
    stage: &DeclarativeStage,
    marker: &StageMarker,
) -> Result<ColdStartReport, Error> {
    warn_if_in_tokio_runtime("cold_start");
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

        if existing.checksum == Some(current) && existing.version.as_ref() == Some(&marker.version)
        {
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

    // A cold start that died while running a non-transactional script left the
    // in-progress marker it committed before the script ran, so the database is
    // reported as partially initialised instead of as one already carrying history.
    if let Some(interrupted) = history
        .iter()
        .filter(|entry| !entry.success && entry.script == STAGE_SCRIPT)
        .max_by_key(|entry| entry.installed_rank)
    {
        return Err(partial_initialisation(interrupted));
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

    let start_time = std::time::Instant::now();

    // A stage holding a non-transactional script is applied outside any
    // transaction, so nothing would record the attempt if the process died
    // midway. The in-progress marker is committed first and replaced by the
    // applied one at the end, so an interrupted cold start stays visible in the
    // history instead of leaving a schema nobody can name.
    let in_progress = stage
        .scripts()
        .iter()
        .any(|script| !script.is_transactional());
    if in_progress {
        record_marker(
            &mut guard,
            &marker.entry(1, current_timestamp(), marker.checksum(), false),
            None,
        )?;
    }

    apply_stage(&mut guard, stage)?;

    let elapsed_ms = start_time.elapsed().as_millis() as i64;

    // The marker anchors the digest of the catalog the stage produced, so the
    // checkpoint written by `stage commit` recognises the initialised database
    // whatever engine executed the DDL. A backend that cannot introspect itself
    // keeps the digest the caller supplied.
    let checksum = match guard.catalog_checksum() {
        Ok(Some(checksum)) => checksum,
        Ok(None) => marker.checksum(),
        Err(error) => return Err(error),
    };

    let entry = marker.entry(1, current_timestamp(), checksum, true);

    record_marker(&mut guard, &entry, in_progress.then_some(1))?;

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

/// Writes `entry` in a transaction of its own.
///
/// `replaces` names the rank of the in-progress marker the write supersedes: both
/// rows describe the same initialisation, so the superseded one is dropped inside
/// the same transaction instead of leaving a failed entry next to the applied one.
fn record_marker<B: MigrationBackend>(
    guard: &mut LockGuard<'_, B>,
    entry: &SchemaHistoryEntry,
    replaces: Option<i64>,
) -> Result<(), Error> {
    let mut tx = guard.begin()?;

    if let Some(installed_rank) = replaces {
        if let Err(error) = tx.delete_entry(installed_rank) {
            let _ = tx.rollback();
            return Err(error);
        }
    }

    if let Err(error) = tx.record_entry(entry) {
        let _ = tx.rollback();
        return Err(error);
    }

    tx.commit()
}

/// Builds the diagnostic of a cold start interrupted while applying the stage.
fn partial_initialisation(entry: &SchemaHistoryEntry) -> Error {
    let version = entry
        .version
        .as_ref()
        .map(ToString::to_string)
        .unwrap_or_default();

    tracing::error!(version = %version, "found an interrupted cold start");

    stage_error(format!(
        "an earlier cold start was interrupted while applying stage `{version}`: the database \
         carries the in-progress marker recorded before its non-transactional scripts ran, so it \
         may hold a partially initialised schema; `repair` clears the marker, and the database \
         itself is re-initialised from scratch before the cold start runs again"
    ))
}

/// Applies the DDL of `stage`, honouring each script's transactional attribute.
///
/// Consecutive transactional scripts share one transaction, so a failure among
/// them leaves the database untouched. A script marked non-transactional commits
/// the work staged so far and then runs on its own: engines refuse such
/// statements inside a transaction, so wrapping them would abort the cold start
/// on a perfectly valid stage.
fn apply_stage<B: MigrationBackend>(
    guard: &mut LockGuard<'_, B>,
    stage: &DeclarativeStage,
) -> Result<(), Error> {
    let scripts = stage.scripts();
    let mut index = 0usize;

    while let Some(script) = scripts.get(index) {
        if !script.is_transactional() {
            if let Err(error) = guard.execute_non_transactional(script.sql()) {
                return Err(stage_script_failed(script, &error));
            }
            index += 1;
            continue;
        }

        let start = index;
        while matches!(scripts.get(index), Some(next) if next.is_transactional()) {
            index += 1;
        }

        let batch = scripts.get(start..index).unwrap_or_default();
        let mut tx = guard.begin()?;

        for script in batch {
            if let Err(error) = tx.execute(script.sql()) {
                let _ = tx.rollback();
                return Err(stage_script_failed(script, &error));
            }
        }

        tx.commit()?;
    }

    Ok(())
}

/// Builds the failure a stage script is reported with.
fn stage_script_failed(script: &StageScript, error: &Error) -> Error {
    tracing::error!(script = %script.name(), error = %error, "stage script failed");
    Error::MigrationExecutionFailed {
        script: script.name().to_owned(),
        source: error.to_string(),
    }
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
        let path = std::env::temp_dir().join(format!("rurqe-stage-{}-{name}", std::process::id()));
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

        let changed = StageMarker::new(
            version("3.2"),
            StageMarker::DESCRIPTION,
            Checksum::of(b"other"),
        );
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

    #[test]
    fn cold_start_outside_a_tokio_runtime_does_not_panic() {
        let mut backend = InMemoryBackend::new();
        let report = cold_start(&mut backend, &stage(), &marker())
            .expect("a cold start outside a runtime succeeds");

        assert_eq!(report.installed_rank, Some(1));
        assert!(!backend.locked);
    }

    /// The advisory has to stay a warning: the blocking entry point keeps working
    /// inside a runtime, exactly like the migration lifecycle it belongs to.
    #[test]
    #[cfg(feature = "tokio")]
    fn cold_start_inside_a_tokio_runtime_still_succeeds() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("build tokio runtime");
        rt.block_on(async {
            let mut backend = InMemoryBackend::new();
            let report = cold_start(&mut backend, &stage(), &marker())
                .expect("the blocking entry point stays callable inside a runtime");

            assert_eq!(report.installed_rank, Some(1));
            assert!(!backend.locked);
        });
    }

    #[test]
    fn a_stage_script_keeps_its_declared_transactionality() {
        assert!(StageScript::new("users.sql", USERS).is_transactional());
        assert!(!StageScript::new("users.notx.sql", USERS).is_transactional());
        assert!(!StageScript::new(
            "users.sql",
            "-- executeInTransaction=false\nCREATE TABLE users (id INTEGER);"
        )
        .is_transactional());
        assert!(!StageScript::new("users.sql", USERS)
            .with_transactional(false)
            .is_transactional());
    }

    #[test]
    fn a_non_transactional_stage_script_runs_outside_a_transaction() {
        let mut backend = InMemoryBackend::new();
        let stage = DeclarativeStage::new(vec![
            StageScript::new("orders.sql", ORDERS),
            StageScript::new(
                "users.sql",
                "-- executeInTransaction=false\nCREATE INDEX CONCURRENTLY users_id ON users (id);",
            ),
        ]);

        let report = cold_start(&mut backend, &stage, &marker()).expect("the stage applies");

        assert_eq!(report.stage_scripts, vec!["orders.sql", "users.sql"]);
        assert_eq!(backend.non_transactional_sql.len(), 1);
        assert_eq!(backend.history.len(), 1);
        assert!(backend.history[0].success);
    }

    #[test]
    fn a_failing_non_transactional_stage_script_leaves_the_interrupted_marker() {
        let mut backend = InMemoryBackend::new();
        backend.failure_script = Some("BOOM".to_owned());
        let stage = DeclarativeStage::new(vec![StageScript::new("users.notx.sql", "BOOM;")]);

        let error = cold_start(&mut backend, &stage, &marker())
            .expect_err("the failing script aborts the cold start");

        assert!(matches!(error, Error::MigrationExecutionFailed { .. }));
        assert_eq!(backend.history.len(), 1);
        assert_eq!(backend.history[0].script, STAGE_SCRIPT);
        assert!(
            !backend.history[0].success,
            "the in-progress marker records the interrupted initialisation"
        );
        assert!(!backend.locked);

        backend.failure_script = None;
        let retry = cold_start(&mut backend, &stage, &marker())
            .expect_err("the interrupted marker stops a second cold start");

        assert!(matches!(retry, Error::InvalidStage { .. }), "got {retry:?}");
        assert!(retry.to_string().contains("interrupted"), "got {retry}");
        assert_eq!(backend.history.len(), 1);
    }

    #[test]
    fn the_in_progress_marker_of_a_non_transactional_stage_precedes_the_scripts() {
        let mut backend = InMemoryBackend::new();
        let stage = DeclarativeStage::new(vec![
            StageScript::new("orders.sql", ORDERS),
            StageScript::new("users.notx.sql", USERS),
        ]);

        let report = cold_start(&mut backend, &stage, &marker()).expect("the stage applies");

        assert_eq!(report.installed_rank, Some(1));
        assert_eq!(
            backend.history_len_at_execution,
            vec![1],
            "the in-progress marker exists before the non-transactional script runs"
        );
        assert_eq!(backend.history.len(), 1);
        assert!(backend.history[0].success);
        assert_eq!(backend.history[0].script, STAGE_SCRIPT);
    }

    #[test]
    fn a_cold_start_that_cannot_record_the_applied_marker_stays_visible() {
        let mut backend = InMemoryBackend::new();
        backend.fail_success_commit = true;
        let stage = DeclarativeStage::new(vec![StageScript::new("users.notx.sql", USERS)]);

        let error = cold_start(&mut backend, &stage, &marker())
            .expect_err("the applied marker cannot be committed");

        assert!(matches!(error, Error::Backend(_)), "got {error:?}");
        assert_eq!(backend.history.len(), 1);
        assert!(
            !backend.history[0].success,
            "the in-progress marker survives the failed commit"
        );
        assert!(!backend.locked);

        backend.fail_success_commit = false;
        let retry = cold_start(&mut backend, &stage, &marker())
            .expect_err("the interrupted marker stops a second cold start");

        assert!(matches!(retry, Error::InvalidStage { .. }), "got {retry:?}");
    }
}
