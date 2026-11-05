use super::backend::MigrationTransaction;
use super::time::format_timestamp;
use super::{
    InMemoryBackend, InMemoryTransaction, LockGuard, MigrationBackend, MigrationStatusKind,
    Migrator,
};
use crate::checkpoint::Checkpoint;
use crate::checksum::Checksum;
use crate::error::Error;
use crate::history::SchemaHistoryEntry;
use crate::migration::{Migration, MigrationKind};

#[test]
fn lock_guard_manages_backend_locking_lifecycle() {
    let mut backend = InMemoryBackend::new();
    assert!(!backend.locked);
    {
        let guard = LockGuard::acquire(&mut backend).expect("lock acquired");
        assert!(guard.locked);
    }
    assert!(!backend.locked);
}

#[test]
fn epoch_converts_to_the_unix_epoch_date() {
    assert_eq!(format_timestamp(0), "1970-01-01 00:00:00");
}

#[test]
fn timestamps_convert_to_utc_civil_time() {
    assert_eq!(format_timestamp(1_700_000_000), "2023-11-14 22:13:20");
}

#[test]
fn leap_days_are_accounted_for() {
    assert_eq!(format_timestamp(951_782_400), "2000-02-29 00:00:00");
}

fn boundary(version: &str) -> Checkpoint {
    Checkpoint::new(
        version.parse().expect("a valid version"),
        Checksum::of(b"stage"),
    )
}

#[test]
fn checkpoint_rejects_targets_below_the_pruned_boundary() {
    let mut backend = InMemoryBackend::new();
    let init = Migration::parse("V1__init.sql", "CREATE TABLE users (id INTEGER);")
        .expect("a valid script name");
    Migrator::new()
        .migrate(&mut backend, &[init])
        .expect("the initial run succeeds");

    let tail = Migration::parse("V2__tail.sql", "CREATE TABLE orders (id INTEGER);")
        .expect("a valid script name");
    let error = Migrator::with_checkpoint(boundary("2"))
        .migrate(&mut backend, &[tail])
        .expect_err("the boundary rejects the outdated target");

    assert!(matches!(error, Error::OutdatedCheckpoint { .. }));
    assert_eq!(backend.history.len(), 1);
}

#[test]
fn checkpoint_allows_targets_at_the_pruned_boundary() {
    let mut backend = InMemoryBackend::new();
    let stage = Migration::parse("V3__stage.sql", "CREATE TABLE users (id INTEGER);")
        .expect("a valid script name");
    Migrator::new()
        .migrate(&mut backend, &[stage])
        .expect("the initial run succeeds");

    let tail = Migration::parse("V4__tail.sql", "CREATE TABLE orders (id INTEGER);")
        .expect("a valid script name");
    let report = Migrator::with_checkpoint(boundary("3"))
        .migrate(&mut backend, &[tail])
        .expect("the target sits at the boundary");

    assert_eq!(report.applied_count, 1);
    assert_eq!(report.applied_scripts, vec!["V4__tail.sql"]);
}

#[test]
fn checkpoint_rejects_diverged_stage_marker_hash() {
    let mut backend = InMemoryBackend::new();
    let stage_entry = SchemaHistoryEntry {
        installed_rank: 1,
        version: Some("3".parse().expect("a valid version")),
        description: "stage".to_owned(),
        kind: MigrationKind::Baseline,
        script: crate::stage::STAGE_SCRIPT.to_owned(),
        checksum: Some(Checksum::of(b"different_stage")),
        installed_on: "2024-01-01 00:00:00".to_owned(),
        execution_time_ms: 0,
        success: true,
    };
    backend.history.push(stage_entry);

    let tail = Migration::parse("V4__tail.sql", "CREATE TABLE orders (id INTEGER);")
        .expect("a valid script name");
    let error = Migrator::with_checkpoint(boundary("3"))
        .migrate(&mut backend, &[tail])
        .expect_err("the diverged stage marker hash must be rejected");

    assert!(matches!(error, Error::CheckpointHashMismatch { .. }));
    assert_eq!(backend.history.len(), 1);
}

#[test]
fn an_interrupted_cold_start_is_reported_as_failed_under_a_checkpoint() {
    let mut backend = InMemoryBackend::new();
    let interrupted = SchemaHistoryEntry {
        installed_rank: 1,
        version: Some("3".parse().expect("a valid version")),
        description: "stage".to_owned(),
        kind: MigrationKind::Baseline,
        script: crate::stage::STAGE_SCRIPT.to_owned(),
        checksum: Some(Checksum::of(b"stage")),
        installed_on: "2024-01-01 00:00:00".to_owned(),
        execution_time_ms: 0,
        success: false,
    };
    backend.history.push(interrupted);

    let tail = Migration::parse("V4__tail.sql", "CREATE TABLE orders (id INTEGER);")
        .expect("a valid script name");

    // Without a boundary the failed entry is what the run reports ...
    let unbounded = Migrator::new()
        .migrate(&mut backend, std::slice::from_ref(&tail))
        .expect_err("an interrupted cold start blocks the run");
    assert!(
        matches!(
            &unbounded,
            Error::FailedMigrationAlreadyApplied {
                script,
                installed_rank: 1
            } if script == crate::stage::STAGE_SCRIPT
        ),
        "got {unbounded:?}"
    );
    assert!(unbounded.to_string().contains("repair"), "got {unbounded}");

    // ... and an attached checkpoint answers exactly the same way. The boundary
    // reads the successful rows alone, so the failed marker is invisible to it:
    // reporting the failure first is what keeps the actionable remedy visible.
    let bounded = Migrator::with_checkpoint(boundary("3"))
        .migrate(&mut backend, &[tail])
        .expect_err("the failed entry blocks a bounded run too");
    assert!(
        !matches!(&bounded, Error::OutdatedCheckpoint { .. }),
        "the boundary must not mask the failed entry: {bounded:?}"
    );
    assert_eq!(
        unbounded, bounded,
        "the attached boundary never changes the diagnostic of a failed entry"
    );

    assert_eq!(backend.history.len(), 1, "no script of the run was applied");
    assert!(!backend.locked, "the lock is released on the way out");
}

#[test]
fn checkpoint_accepts_a_catalog_matching_its_hash() {
    let mut backend = InMemoryBackend::new();
    let stage = Migration::parse("V3__stage.sql", "CREATE TABLE users (id INTEGER);")
        .expect("a valid script name");
    Migrator::new()
        .migrate(&mut backend, &[stage])
        .expect("the stage migration applies");

    backend.catalog_checksum = Some(Checksum::of(b"stage"));

    let tail = Migration::parse("V4__tail.sql", "CREATE TABLE orders (id INTEGER);")
        .expect("a valid script name");
    let report = Migrator::with_checkpoint(boundary("3"))
        .migrate(&mut backend, &[tail])
        .expect("the live catalog matches the checkpoint hash");

    assert_eq!(report.applied_count, 1);
    assert_eq!(report.applied_scripts, vec!["V4__tail.sql"]);
}

#[test]
fn checkpoint_rejects_a_catalog_that_diverged_from_its_hash() {
    let mut backend = InMemoryBackend::new();
    let stage = Migration::parse("V3__stage.sql", "CREATE TABLE users (id INTEGER);")
        .expect("a valid script name");
    Migrator::new()
        .migrate(&mut backend, &[stage])
        .expect("the stage migration applies");

    backend.catalog_checksum = Some(Checksum::of(b"drifted"));

    let tail = Migration::parse("V4__tail.sql", "CREATE TABLE orders (id INTEGER);")
        .expect("a valid script name");
    let error = Migrator::with_checkpoint(boundary("3"))
        .migrate(&mut backend, &[tail])
        .expect_err("the diverged catalog stops the run");

    match &error {
        Error::CheckpointHashMismatch {
            pruned_up_to_version,
            recorded_hash,
            expected_hash,
        } => {
            assert_eq!(pruned_up_to_version, "3");
            assert_eq!(recorded_hash, &Checksum::of(b"drifted").to_hex());
            assert_eq!(expected_hash, &Checksum::of(b"stage").to_hex());
        }
        other => panic!("expected CheckpointHashMismatch, got {other:?}"),
    }
    assert_eq!(backend.history.len(), 1, "the tail never ran");
}

#[test]
fn a_non_transactional_migration_runs_outside_a_transaction() {
    let mut backend = InMemoryBackend::new();
    let migration = Migration::parse(
        "V1__index.sql",
        "-- executeInTransaction=false\nCREATE INDEX CONCURRENTLY users_id ON users (id);",
    )
    .expect("a valid script name");

    let report = Migrator::new()
        .migrate(&mut backend, &[migration])
        .expect("the non-transactional script applies");

    assert_eq!(report.applied_count, 1);
    assert_eq!(backend.non_transactional_sql.len(), 1);
    assert_eq!(backend.executed_sql.len(), 1);
    assert_eq!(backend.history.len(), 1);
    assert!(backend.history[0].success);
}

#[test]
fn the_history_of_a_non_transactional_migration_is_written_after_it_runs() {
    let mut backend = InMemoryBackend::new();
    let first = Migration::parse("V1__users.sql", "CREATE TABLE users (id INTEGER);")
        .expect("a valid script name");
    let second = Migration::parse(
        "V2__index.notx.sql",
        "CREATE INDEX CONCURRENTLY users_id ON users (id);",
    )
    .expect("a valid script name");

    let report = Migrator::new()
        .migrate(&mut backend, &[first, second])
        .expect("both scripts apply");

    assert_eq!(
        report.applied_scripts,
        vec!["V1__users.sql", "V2__index.notx.sql"]
    );
    assert_eq!(backend.non_transactional_sql.len(), 1);
    assert_eq!(backend.history.len(), 2);
    assert!(backend.history.iter().all(|entry| entry.success));
}

#[test]
fn duplicate_script_names_are_rejected_before_any_migration_runs() {
    let mut backend = InMemoryBackend::new();
    let first =
        Migration::parse("R__view.sql", "CREATE VIEW v AS SELECT 1;").expect("a valid script name");
    let second =
        Migration::parse("R__view.sql", "CREATE VIEW v AS SELECT 2;").expect("a valid script name");

    let error = Migrator::new()
        .migrate(&mut backend, &[first, second])
        .expect_err("two scripts under one name must be rejected");

    assert_eq!(
        error,
        Error::DuplicateScriptName {
            script: "R__view.sql".to_owned()
        }
    );
    assert!(backend.history.is_empty(), "no script ran");
    assert!(
        !backend.table_created,
        "the metadata table is never created for a rejected run"
    );
    assert!(!backend.locked, "the lock is released on the way out");
}

#[test]
fn a_failed_non_transactional_migration_is_recorded_as_failed() {
    let mut backend = InMemoryBackend::new();
    backend.failure_script = Some("BOOM".to_owned());
    let migration = Migration::parse("V1__broken.notx.sql", "BOOM;").expect("a valid script name");

    let error = Migrator::new()
        .migrate(&mut backend, &[migration])
        .expect_err("the failing script aborts the run");

    assert!(matches!(error, Error::MigrationExecutionFailed { .. }));
    assert_eq!(backend.history.len(), 1);
    assert!(!backend.history[0].success);
    assert!(!backend.locked);
}

/// A backend that keeps every script inside a transaction.
///
/// It leaves `execute_non_transactional` at its default, so it behaves exactly
/// like a driver that cannot commit a statement outside the runner's
/// transaction.
struct TransactionOnlyBackend(InMemoryBackend);

impl MigrationBackend for TransactionOnlyBackend {
    type Transaction<'a>
        = InMemoryTransaction<'a>
    where
        Self: 'a;

    fn ensure_schema_history_table(&mut self) -> Result<(), Error> {
        self.0.ensure_schema_history_table()
    }

    fn read_history(&mut self) -> Result<Vec<SchemaHistoryEntry>, Error> {
        self.0.read_history()
    }

    fn begin(&mut self) -> Result<Self::Transaction<'_>, Error> {
        self.0.begin()
    }
}

#[test]
fn a_backend_without_non_transactional_support_names_the_refused_script() {
    let mut backend = TransactionOnlyBackend(InMemoryBackend::new());
    let first = Migration::parse("V1__users.sql", "CREATE TABLE users (id INTEGER);")
        .expect("a valid script name");
    let index = Migration::parse(
        "V2__index.notx.sql",
        "CREATE INDEX CONCURRENTLY users_id ON users (id);",
    )
    .expect("a valid script name");

    let error = Migrator::new()
        .migrate(&mut backend, &[first, index])
        .expect_err("the backend keeps the script inside a transaction");

    match &error {
        Error::NonTransactionalUnsupported { script } => {
            assert_eq!(script.as_deref(), Some("V2__index.notx.sql"));
        }
        other => panic!("expected NonTransactionalUnsupported, got {other:?}"),
    }
    assert!(
        !error.to_string().contains("CREATE INDEX"),
        "the diagnostic never echoes the script body: {error}"
    );
    assert_eq!(
        backend.0.history.len(),
        1,
        "the refused script leaves no failed entry behind"
    );
    assert!(backend.0.history[0].success);
    assert!(
        backend.0.non_transactional_sql.is_empty(),
        "the refused script never reached the backend"
    );
    assert!(!backend.0.locked, "the lock is released on the way out");
}

#[test]
fn the_in_progress_row_of_a_non_transactional_script_precedes_it() {
    let mut backend = InMemoryBackend::new();
    let migration = Migration::parse(
        "V1__index.notx.sql",
        "CREATE INDEX CONCURRENTLY users_id ON users;",
    )
    .expect("a valid script name");

    Migrator::new()
        .migrate(&mut backend, std::slice::from_ref(&migration))
        .expect("the non-transactional script applies");

    assert_eq!(
        backend.history_len_at_execution,
        vec![1],
        "the in-progress row is committed before the script runs"
    );
    assert_eq!(backend.history.len(), 1);
    assert!(backend.history[0].success);
    assert_eq!(backend.history[0].installed_rank, 1);
}

#[test]
fn a_non_transactional_script_whose_applied_row_is_lost_names_the_script() {
    let mut backend = InMemoryBackend::new();
    backend.fail_success_entry = true;
    let migration = Migration::parse(
        "V1__index.notx.sql",
        "CREATE INDEX CONCURRENTLY users_id ON users;",
    )
    .expect("a valid script name");

    let error = Migrator::new()
        .migrate(&mut backend, std::slice::from_ref(&migration))
        .expect_err("the lost history row is reported instead of a backend error");

    match &error {
        Error::HistoryNotRecorded { script, .. } => assert_eq!(script, "V1__index.notx.sql"),
        other => panic!("expected HistoryNotRecorded, got {other:?}"),
    }
    assert!(error.to_string().contains("repair"), "got {error}");
    assert_eq!(
        backend.non_transactional_sql.len(),
        1,
        "the script really ran before its applied row was written"
    );
    assert_eq!(backend.history.len(), 1);
    assert!(!backend.history[0].success);
    assert_eq!(backend.history[0].script, "V1__index.notx.sql");
    assert!(!backend.locked);

    // The in-progress row blocks the next run until `repair` clears it.
    let blocked = Migrator::new()
        .migrate(&mut backend, std::slice::from_ref(&migration))
        .expect_err("the in-progress entry blocks the next run");
    assert!(matches!(
        blocked,
        Error::FailedMigrationAlreadyApplied { .. }
    ));

    let repaired = Migrator::new()
        .repair(&mut backend, std::slice::from_ref(&migration))
        .expect("repair clears the in-progress entry");
    assert_eq!(repaired.removed_failed_entries, 1);

    backend.fail_success_entry = false;
    let report = Migrator::new()
        .migrate(&mut backend, &[migration])
        .expect("the repaired run applies the script");
    assert_eq!(report.applied_count, 1);
    assert!(backend.history.iter().all(|entry| entry.success));
}

#[test]
fn a_non_transactional_script_whose_applied_row_cannot_commit_is_reported() {
    let mut backend = InMemoryBackend::new();
    backend.fail_success_commit = true;
    let migration = Migration::parse(
        "V1__index.notx.sql",
        "CREATE INDEX CONCURRENTLY users_id ON users;",
    )
    .expect("a valid script name");

    let error = Migrator::new()
        .migrate(&mut backend, std::slice::from_ref(&migration))
        .expect_err("the failing commit is reported");

    assert!(
        matches!(
            &error,
            Error::HistoryNotRecorded { script, .. } if script == "V1__index.notx.sql"
        ),
        "got {error:?}"
    );
    assert_eq!(backend.history.len(), 1);
    assert!(
        !backend.history[0].success,
        "the in-progress row survives the failed commit"
    );
    assert!(!backend.locked);
}

/// A backend recording whether the run lock was held while the history table
/// was ensured.
#[derive(Default)]
struct LockObservingBackend {
    inner: InMemoryBackend,
    lock_states_at_ddl: Vec<bool>,
}

impl MigrationBackend for LockObservingBackend {
    type Transaction<'a>
        = InMemoryTransaction<'a>
    where
        Self: 'a;

    fn ensure_schema_history_table(&mut self) -> Result<(), Error> {
        self.lock_states_at_ddl.push(self.inner.locked);
        self.inner.ensure_schema_history_table()
    }

    fn read_history(&mut self) -> Result<Vec<SchemaHistoryEntry>, Error> {
        self.inner.read_history()
    }

    fn begin(&mut self) -> Result<Self::Transaction<'_>, Error> {
        self.inner.begin()
    }

    fn acquire_lock(&mut self) -> Result<(), Error> {
        self.inner.acquire_lock()
    }

    fn release_lock(&mut self) -> Result<(), Error> {
        self.inner.release_lock()
    }
}

#[test]
fn status_ensures_the_history_table_under_the_run_lock() {
    let mut backend = LockObservingBackend::default();
    let migrations = [
        Migration::parse("V1__users.sql", "CREATE TABLE users (id INTEGER);")
            .expect("a valid script name"),
    ];

    let statuses = Migrator::new()
        .status(&mut backend, &migrations)
        .expect("the status read succeeds on an uninitialized database");

    assert_eq!(statuses.len(), 1);
    assert_eq!(statuses[0].status, MigrationStatusKind::Pending);
    assert_eq!(
        backend.lock_states_at_ddl,
        vec![true],
        "the metadata write runs under the same run lock as the other cores"
    );
    assert!(
        !backend.inner.locked,
        "the lock is released once the history is read"
    );
}

/// The history write a fixture transaction is made to fail.
///
/// The two knobs of [`InMemoryBackend`] (`fail_success_entry` and
/// `fail_success_commit`) inject a failure at the applied-row write and at the
/// commit. This enum adds the mutations [`Migrator::repair`] performs, which the
/// in-memory backend always accepts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum HistoryFailure {
    /// Every history write succeeds beyond the knobs of [`InMemoryBackend`].
    #[default]
    None,
    /// Deleting a recorded history entry fails.
    DeleteEntry,
    /// Updating a recorded checksum fails.
    UpdateChecksum,
}

/// How the fixture transactions of a run terminated.
#[derive(Debug, Default)]
struct TransactionState {
    /// Transactions opened and not terminated yet.
    open: usize,
    /// Commits that succeeded.
    commits: usize,
    /// Commits the fixture failed.
    failed_commits: usize,
    /// Transactions ended by an explicit rollback.
    rollbacks: usize,
}

/// An [`InMemoryBackend`] recording how every transaction terminates.
///
/// The staged rows live in the wrapped [`InMemoryTransaction`], which discards
/// them on rollback and on drop, so an abandoned transaction leaves the recorded
/// history untouched and `open` above zero.
#[derive(Debug, Default)]
struct ObservingBackend {
    inner: InMemoryBackend,
    failure: HistoryFailure,
    state: TransactionState,
}

impl MigrationBackend for ObservingBackend {
    type Transaction<'a> = ObservingTransaction<'a>;

    fn ensure_schema_history_table(&mut self) -> Result<(), Error> {
        self.inner.ensure_schema_history_table()
    }

    fn read_history(&mut self) -> Result<Vec<SchemaHistoryEntry>, Error> {
        self.inner.read_history()
    }

    fn begin(&mut self) -> Result<Self::Transaction<'_>, Error> {
        let inner = self.inner.begin()?;
        self.state.open += 1;
        Ok(ObservingTransaction {
            inner,
            failure: self.failure,
            state: &mut self.state,
        })
    }
}

/// A transaction of [`ObservingBackend`] delegating its history writes.
struct ObservingTransaction<'a> {
    inner: InMemoryTransaction<'a>,
    failure: HistoryFailure,
    state: &'a mut TransactionState,
}

impl MigrationTransaction for ObservingTransaction<'_> {
    fn execute(&mut self, sql: &str) -> Result<(), Error> {
        self.inner.execute(sql)
    }

    fn record_entry(&mut self, entry: &SchemaHistoryEntry) -> Result<(), Error> {
        self.inner.record_entry(entry)
    }

    fn delete_entry(&mut self, installed_rank: i64) -> Result<(), Error> {
        if self.failure == HistoryFailure::DeleteEntry {
            return Err(Error::Backend(format!(
                "simulated failure deleting the history entry at rank {installed_rank}"
            )));
        }
        self.inner.delete_entry(installed_rank)
    }

    fn update_checksum(&mut self, script: &str, checksum: &Checksum) -> Result<(), Error> {
        if self.failure == HistoryFailure::UpdateChecksum {
            return Err(Error::Backend(format!(
                "simulated failure updating the recorded checksum of `{script}`"
            )));
        }
        self.inner.update_checksum(script, checksum)
    }

    fn commit(self) -> Result<(), Error> {
        let ObservingTransaction { inner, state, .. } = self;
        state.open = state.open.saturating_sub(1);
        let outcome = inner.commit();
        if outcome.is_ok() {
            state.commits += 1;
        } else {
            state.failed_commits += 1;
        }
        outcome
    }

    fn rollback(self) -> Result<(), Error> {
        let ObservingTransaction { inner, state, .. } = self;
        state.open = state.open.saturating_sub(1);
        state.rollbacks += 1;
        inner.rollback()
    }
}

#[test]
fn a_transactional_script_whose_history_write_fails_names_the_script_and_rolls_back() {
    let mut backend = ObservingBackend::default();
    backend.inner.fail_success_entry = true;
    let migration = Migration::parse("V1__users.sql", "CREATE TABLE users (id INTEGER);")
        .expect("a valid script name");

    let error = Migrator::new()
        .migrate(&mut backend, std::slice::from_ref(&migration))
        .expect_err("the failing history write aborts the run");

    match &error {
        Error::MigrationExecutionFailed { script, source } => {
            assert_eq!(script, "V1__users.sql");
            assert!(
                source.contains("V1__users.sql"),
                "the backend diagnostic travels inside the script-naming error: {source}"
            );
        }
        other => panic!("expected MigrationExecutionFailed, got {other:?}"),
    }

    assert_eq!(
        backend.state.rollbacks, 1,
        "the transaction of the script is rolled back"
    );
    assert_eq!(
        backend.state.commits, 1,
        "only the best-effort failed row commits"
    );
    assert_eq!(backend.state.failed_commits, 0);
    assert_eq!(
        backend.state.open, 0,
        "every transaction is terminated before the run returns"
    );
    assert_eq!(backend.inner.history.len(), 1);
    assert_eq!(backend.inner.history[0].script, "V1__users.sql");
    assert!(
        !backend.inner.history[0].success,
        "the attempt is recorded as failed, not as applied"
    );
}

#[test]
fn a_transactional_script_whose_commit_fails_names_the_script_and_leaves_no_applied_row() {
    let mut backend = ObservingBackend::default();
    backend.inner.fail_success_commit = true;
    let migration = Migration::parse("V1__users.sql", "CREATE TABLE users (id INTEGER);")
        .expect("a valid script name");

    let error = Migrator::new()
        .migrate(&mut backend, std::slice::from_ref(&migration))
        .expect_err("the failing commit aborts the run");

    match &error {
        Error::MigrationExecutionFailed { script, source } => {
            assert_eq!(script, "V1__users.sql");
            assert!(source.contains("commit"), "got {source}");
        }
        other => panic!("expected MigrationExecutionFailed, got {other:?}"),
    }

    assert_eq!(
        backend.state.failed_commits, 1,
        "the commit of the script transaction fails"
    );
    assert_eq!(
        backend.state.rollbacks, 0,
        "a failed commit consumes its transaction"
    );
    assert_eq!(
        backend.state.commits, 1,
        "only the best-effort failed row commits"
    );
    assert_eq!(
        backend.state.open, 0,
        "every transaction is terminated before the run returns"
    );
    assert_eq!(backend.inner.history.len(), 1);
    assert_eq!(backend.inner.history[0].script, "V1__users.sql");
    assert!(
        !backend.inner.history[0].success,
        "the history holds no applied row for the script"
    );
}

#[test]
fn a_baseline_whose_history_write_fails_propagates_the_backend_error_and_rolls_back() {
    let mut backend = ObservingBackend::default();
    backend.inner.fail_success_entry = true;

    let error = Migrator::new()
        .baseline(
            &mut backend,
            "1.0.0".parse().expect("a valid version"),
            "legacy baseline",
        )
        .expect_err("the failing history write aborts the baseline");

    match &error {
        Error::Backend(message) => assert!(message.contains("recording"), "got {message}"),
        other => panic!("expected Backend, got {other:?}"),
    }

    assert!(
        backend.inner.history.is_empty(),
        "the baseline row is rolled back with its transaction"
    );
    assert_eq!(backend.state.rollbacks, 1);
    assert_eq!(backend.state.commits, 0);
    assert_eq!(backend.state.open, 0);
}

#[test]
fn a_repair_whose_purge_fails_rolls_back_the_staged_checksum_update() {
    let mut backend = ObservingBackend {
        failure: HistoryFailure::DeleteEntry,
        ..ObservingBackend::default()
    };

    let original = Migration::parse("V1__users.sql", "CREATE TABLE users (id INTEGER);")
        .expect("a valid script name");
    let edited = Migration::parse("V1__users.sql", "CREATE TABLE users (id BIGINT);")
        .expect("a valid script name");
    let failed = Migration::parse("V2__broken.sql", "BOOM;").expect("a valid script name");

    let history = vec![
        SchemaHistoryEntry::applied(1, &original, "2024-01-01 00:00:00", 3),
        SchemaHistoryEntry::failed(2, &failed, "2024-01-01 00:00:00", 1),
    ];
    backend.inner.history = history.clone();

    let error = Migrator::new()
        .repair(&mut backend, &[edited, failed])
        .expect_err("the failing purge aborts the repair");

    match &error {
        Error::Backend(message) => assert!(message.contains("deleting"), "got {message}"),
        other => panic!("expected Backend, got {other:?}"),
    }

    assert_eq!(
        backend.inner.history, history,
        "the whole repair transaction is rolled back"
    );
    assert_eq!(
        backend.inner.history[0].checksum,
        Some(original.checksum),
        "the staged checksum update is not persisted"
    );
    assert!(
        !backend.inner.history[1].success,
        "the failed entry is not purged"
    );
    assert_eq!(backend.state.rollbacks, 1);
    assert_eq!(backend.state.commits, 0);
    assert_eq!(backend.state.open, 0);
}

#[test]
fn a_repair_whose_checksum_update_fails_rolls_back_the_staged_purge() {
    let mut backend = ObservingBackend {
        failure: HistoryFailure::UpdateChecksum,
        ..ObservingBackend::default()
    };

    let original = Migration::parse("V1__users.sql", "CREATE TABLE users (id INTEGER);")
        .expect("a valid script name");
    let edited = Migration::parse("V1__users.sql", "CREATE TABLE users (id BIGINT);")
        .expect("a valid script name");
    let failed = Migration::parse("V2__broken.sql", "BOOM;").expect("a valid script name");

    // The failed entry is recorded ahead of the edited one on purpose: its purge
    // is staged before the checksum update fails, so the rollback has to discard
    // the purge as well.
    let history = vec![
        SchemaHistoryEntry::failed(1, &failed, "2024-01-01 00:00:00", 1),
        SchemaHistoryEntry::applied(2, &original, "2024-01-01 00:00:00", 3),
    ];
    backend.inner.history = history.clone();

    let error = Migrator::new()
        .repair(&mut backend, &[edited, failed])
        .expect_err("the failing checksum update aborts the repair");

    match &error {
        Error::Backend(message) => assert!(message.contains("checksum"), "got {message}"),
        other => panic!("expected Backend, got {other:?}"),
    }

    assert_eq!(
        backend.inner.history, history,
        "the whole repair transaction is rolled back"
    );
    assert!(
        !backend.inner.history[0].success,
        "the staged purge is not persisted"
    );
    assert_eq!(
        backend.inner.history[1].checksum,
        Some(original.checksum),
        "the recorded checksum is unchanged"
    );
    assert_eq!(backend.state.rollbacks, 1);
    assert_eq!(backend.state.commits, 0);
    assert_eq!(backend.state.open, 0);
}

#[test]
fn duplicate_script_names_are_rejected_before_a_repair_rewrites_a_checksum() {
    let mut backend = InMemoryBackend::new();
    let recorded = Migration::parse("V1__users.sql", "CREATE TABLE users (id INTEGER);")
        .expect("a valid script name");
    Migrator::new()
        .migrate(&mut backend, &[recorded])
        .expect("the initial run succeeds");
    let history = backend.history.clone();

    let first = Migration::parse("V1__users.sql", "CREATE TABLE users (id BIGINT);")
        .expect("a valid script name");
    let second = Migration::parse("V1__users.sql", "CREATE TABLE users (id SMALLINT);")
        .expect("a valid script name");

    let error = Migrator::new()
        .repair(&mut backend, &[first, second])
        .expect_err("two scripts under one name must be rejected");

    assert_eq!(
        error,
        Error::DuplicateScriptName {
            script: "V1__users.sql".to_owned()
        }
    );
    assert_eq!(
        backend.history, history,
        "the recorded checksum is never rewritten against an arbitrary survivor"
    );
    assert!(
        !backend.locked,
        "the run lock is never taken for a rejected repair"
    );
}

#[test]
fn duplicate_script_names_are_rejected_before_a_status_is_computed() {
    let mut backend = InMemoryBackend::new();
    let first =
        Migration::parse("R__view.sql", "CREATE VIEW v AS SELECT 1;").expect("a valid script name");
    let second =
        Migration::parse("R__view.sql", "CREATE VIEW v AS SELECT 2;").expect("a valid script name");

    let error = Migrator::new()
        .status(&mut backend, &[first, second])
        .expect_err("two scripts under one name must be rejected");

    assert_eq!(
        error,
        Error::DuplicateScriptName {
            script: "R__view.sql".to_owned()
        }
    );
    assert!(
        !backend.table_created,
        "the metadata table is never created for a rejected status"
    );
    assert!(
        !backend.locked,
        "the run lock is never taken for a rejected status"
    );
}

/// A backend whose clean protocol is observable.
///
/// The manifest it reports is the list it holds, and both protocol calls record
/// whether the run lock was held while they ran: that is how a test sees `clean`
/// serialised with `migrate` and `clean_manifest` reading freely.
#[derive(Debug, Default)]
struct CleanObservingBackend {
    inner: InMemoryBackend,
    manifest: Vec<String>,
    lock_states_at_manifest: Vec<bool>,
    lock_states_at_clean: Vec<bool>,
}

impl MigrationBackend for CleanObservingBackend {
    type Transaction<'a>
        = InMemoryTransaction<'a>
    where
        Self: 'a;

    fn ensure_schema_history_table(&mut self) -> Result<(), Error> {
        self.inner.ensure_schema_history_table()
    }

    fn read_history(&mut self) -> Result<Vec<SchemaHistoryEntry>, Error> {
        self.inner.read_history()
    }

    fn begin(&mut self) -> Result<Self::Transaction<'_>, Error> {
        self.inner.begin()
    }

    fn acquire_lock(&mut self) -> Result<(), Error> {
        self.inner.acquire_lock()
    }

    fn release_lock(&mut self) -> Result<(), Error> {
        self.inner.release_lock()
    }

    fn clean_manifest(&mut self) -> Result<Vec<String>, Error> {
        self.lock_states_at_manifest.push(self.inner.locked);
        Ok(self.manifest.clone())
    }

    fn clean(&mut self) -> Result<Vec<String>, Error> {
        self.lock_states_at_clean.push(self.inner.locked);
        Ok(self.manifest.clone())
    }
}

#[test]
fn a_backend_without_clean_support_refuses_before_touching_the_database() {
    let mut backend = InMemoryBackend::new();

    let error = Migrator::new()
        .clean(&mut backend)
        .expect_err("the in-memory backend leaves the clean protocol at its default");

    assert_eq!(error, Error::CleanUnsupported);
    assert!(
        backend.executed_sql.is_empty(),
        "the refusal is raised before any statement reaches the backend"
    );
    assert!(!backend.table_created, "no metadata table is created");
    assert!(
        backend.history.is_empty(),
        "the recorded history is untouched"
    );
    assert!(backend.non_transactional_sql.is_empty());
    assert!(!backend.locked, "the lock is released on the way out");
}

#[test]
fn a_backend_without_clean_support_refuses_a_manifest_read() {
    let mut backend = InMemoryBackend::new();

    let error = Migrator::new()
        .clean_manifest(&mut backend)
        .expect_err("the in-memory backend leaves the clean protocol at its default");

    assert_eq!(error, Error::CleanUnsupported);
    assert!(backend.executed_sql.is_empty());
    assert!(!backend.locked, "the manifest read takes no run lock");
}

#[test]
fn clean_holds_the_run_lock_a_concurrent_migrate_takes() {
    let mut backend = CleanObservingBackend {
        manifest: vec!["users".to_owned(), "__rurqe_schema_history".to_owned()],
        ..CleanObservingBackend::default()
    };

    let dropped = Migrator::new()
        .clean(&mut backend)
        .expect("the clean succeeds");

    assert_eq!(
        dropped, backend.manifest,
        "the report names the manifest objects"
    );
    assert_eq!(
        backend.lock_states_at_clean,
        vec![true],
        "the clean runs under the run lock"
    );
    assert!(
        backend.lock_states_at_manifest.is_empty(),
        "the clean never consults the manifest of its own accord"
    );
    assert!(!backend.inner.locked, "the lock is released on the way out");
}

#[test]
fn a_clean_is_refused_while_another_run_holds_the_lock() {
    let mut backend = CleanObservingBackend {
        manifest: vec!["users".to_owned()],
        ..CleanObservingBackend::default()
    };
    backend.inner.locked = true;

    let error = Migrator::new()
        .clean(&mut backend)
        .expect_err("a concurrent run holds the run lock");

    assert!(matches!(error, Error::Backend(_)), "got {error:?}");
    assert!(
        backend.lock_states_at_clean.is_empty(),
        "nothing is dropped while the lock belongs to another run"
    );
    assert!(
        backend.inner.locked,
        "the lock of the concurrent run stays held"
    );
}

#[test]
fn clean_manifest_reads_the_target_without_taking_the_run_lock() {
    let mut backend = CleanObservingBackend {
        manifest: vec!["users".to_owned(), "orders".to_owned()],
        ..CleanObservingBackend::default()
    };

    let manifest = Migrator::new()
        .clean_manifest(&mut backend)
        .expect("the manifest read succeeds");

    assert_eq!(manifest, backend.manifest);
    assert_eq!(
        backend.lock_states_at_manifest,
        vec![false],
        "the manifest read holds no run lock"
    );
    assert!(
        backend.lock_states_at_clean.is_empty(),
        "the manifest read drops nothing"
    );
    assert!(!backend.inner.locked);
    assert!(!backend.inner.locked);
}

#[test]
fn a_baseline_description_beyond_the_history_column_is_rejected_before_the_backend() {
    let mut backend = InMemoryBackend::new();
    let description = "d".repeat(crate::history::DESCRIPTION_COLUMN_WIDTH + 1);

    let error = Migrator::new()
        .baseline(
            &mut backend,
            "1.0.0".parse().expect("a valid version"),
            &description,
        )
        .expect_err("the description does not fit the history column");

    assert_eq!(
        error,
        Error::MetadataTooLong {
            script: crate::history::BASELINE_SCRIPT.to_owned(),
            column: "description",
            actual: crate::history::DESCRIPTION_COLUMN_WIDTH + 1,
            limit: crate::history::DESCRIPTION_COLUMN_WIDTH,
        }
    );
    assert!(
        !backend.table_created,
        "the metadata table is never created for a rejected baseline"
    );
    assert!(
        backend.history.is_empty(),
        "the refused baseline records no entry"
    );
    assert!(!backend.locked, "the run lock is never taken");
}
