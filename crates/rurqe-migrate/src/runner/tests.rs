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
    let migration =
        Migration::parse("V1__index.notx.sql", "CREATE INDEX CONCURRENTLY users_id ON users;")
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
    let migration =
        Migration::parse("V1__index.notx.sql", "CREATE INDEX CONCURRENTLY users_id ON users;")
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
    let migration =
        Migration::parse("V1__index.notx.sql", "CREATE INDEX CONCURRENTLY users_id ON users;")
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
    let migrations = [Migration::parse(
        "V1__users.sql",
        "CREATE TABLE users (id INTEGER);",
    )
    .expect("a valid script name")];

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
