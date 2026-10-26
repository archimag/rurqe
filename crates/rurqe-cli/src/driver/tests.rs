//! Driver-level tests: the SQLite lifecycle, the PostgreSQL advisory lock wait,
//! and the pause strategy that wait delegates to.

use std::sync::Mutex;
use std::time::Duration;

use rurqe_migrate::error::Error;
use rurqe_migrate::migration::{Migration, MigrationKind};
use rurqe_migrate::runner::{MigrationBackend, MigrationTransaction, Migrator};
use rurqe_migrate::sleeper::{NoopSleeper, Sleeper};

use super::postgres::{apply_unlock_answer, poll_advisory_lock, MIGRATION_LOCK_ID};
use super::sqlite::{left_open_message, SqliteMigrationBackend};

/// A [`Sleeper`] recording the pause of every waiting round.
#[derive(Default)]
struct RecordingSleeper {
    pauses: Mutex<Vec<Duration>>,
}

impl RecordingSleeper {
    fn new() -> Self {
        Self::default()
    }

    fn pauses(&self) -> Vec<Duration> {
        self.pauses
            .lock()
            .map(|guard| guard.to_vec())
            .unwrap_or_default()
    }
}

impl Sleeper for RecordingSleeper {
    fn sleep(&self, duration: Duration) {
        if let Ok(mut guard) = self.pauses.lock() {
            guard.push(duration);
        }
    }
}

fn backend() -> SqliteMigrationBackend {
    SqliteMigrationBackend::new(rusqlite::Connection::open_in_memory().unwrap())
}

fn migrations(script: &str, sql: &str) -> Vec<Migration> {
    vec![Migration::parse(script, sql).unwrap()]
}

#[test]
fn sqlite_backend_applies_versioned_migrations_once() {
    let mut backend = backend();
    let scripts = migrations("V1__init.sql", "CREATE TABLE users (id INTEGER);");

    let report = Migrator::new().migrate(&mut backend, &scripts).unwrap();
    assert_eq!(report.applied_count, 1);

    let report = Migrator::new().migrate(&mut backend, &scripts).unwrap();
    assert_eq!(report.applied_count, 0);

    let history = backend.read_history().unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].script, "V1__init.sql");
    assert_eq!(history[0].kind, MigrationKind::Versioned);
    assert!(history[0].success);
}

#[test]
fn sqlite_backend_rolls_back_failed_migrations() {
    let mut backend = backend();
    let broken = migrations("V1__broken.sql", "CREATE TABLE oops (;");

    let error = Migrator::new()
        .migrate(&mut backend, &broken)
        .expect_err("the broken script fails to execute");
    assert!(matches!(
        error,
        rurqe_migrate::error::Error::MigrationExecutionFailed { .. }
    ));

    let history = backend.read_history().expect("history is readable");
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].script, "V1__broken.sql");
    assert!(!history[0].success);

    let blocked = Migrator::new()
        .migrate(&mut backend, &broken)
        .expect_err("the failed entry blocks the next run");
    assert!(matches!(
        blocked,
        rurqe_migrate::error::Error::FailedMigrationAlreadyApplied { .. }
    ));

    let migrator = Migrator::new();
    let fixed = migrations("V1__fixed.sql", "CREATE TABLE ok (id INTEGER);");
    let repair = migrator
        .repair(&mut backend, &fixed)
        .expect("repair purges the failed entry");
    assert_eq!(repair.removed_failed_entries, 1);

    let report = migrator
        .migrate(&mut backend, &fixed)
        .expect("the fixed script applies");
    assert_eq!(report.applied_count, 1);
}

#[test]
fn sqlite_backend_reports_the_digest_of_its_catalog() {
    let mut backend = backend();
    let scripts = migrations(
        "V1__users.sql",
        "CREATE TABLE users (id INTEGER NOT NULL PRIMARY KEY, email TEXT NOT NULL);",
    );
    Migrator::new()
        .migrate(&mut backend, &scripts)
        .expect("the script applies");

    let catalog = backend
        .introspect_catalog()
        .expect("the catalog of the connection is introspected");
    assert_eq!(catalog.tables.len(), 1);
    assert_eq!(catalog.tables[0].name, "users");

    // The history table stays outside the catalog the digest describes.
    assert_eq!(
        backend
            .catalog_checksum()
            .expect("the digest of the connection"),
        Some(catalog.checksum())
    );
}

#[test]
fn sqlite_transaction_introspects_the_catalog_it_sees() {
    let mut backend = backend();
    let scripts = migrations("V1__users.sql", "CREATE TABLE users (id INTEGER NOT NULL);");
    Migrator::new()
        .migrate(&mut backend, &scripts)
        .expect("the script applies");

    let mut tx = backend.begin().expect("the migration transaction opens");
    let catalog = tx
        .introspect_catalog()
        .expect("the catalog of the transaction is introspected");
    assert_eq!(catalog.tables.len(), 1);
    assert_eq!(catalog.tables[0].name, "users");

    // A change applied by the very transaction is visible to its own catalog.
    tx.execute("CREATE TABLE orders (id INTEGER NOT NULL);")
        .expect("the transaction applies the step");
    assert_eq!(
        tx.introspect_catalog()
            .expect("the catalog of the transaction is introspected")
            .tables
            .len(),
        2
    );

    tx.rollback().expect("the transaction is rolled back");
}

#[test]
fn the_run_lock_nests_every_step_in_a_savepoint() {
    let mut backend = backend();
    backend.acquire_lock().expect("the write lock is taken");

    {
        let mut step = backend.begin().expect("a step opens inside the lock");
        step.execute("CREATE TABLE users (id INTEGER NOT NULL);")
            .expect("the step applies");
        step.commit().expect("the step is released");
    }

    {
        let mut step = backend
            .begin()
            .expect("a second step opens inside the lock");
        step.execute("CREATE TABLE oops (;")
            .expect_err("the broken statement fails");
        step.rollback().expect("the step is rolled back");
    }

    backend.release_lock().expect("the lock commits the run");

    let users: i64 = backend
        .conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name = 'users'",
            [],
            |row| row.get(0),
        )
        .expect("the committed table is visible");
    assert_eq!(users, 1);

    let oops: i64 = backend
        .conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name = 'oops'",
            [],
            |row| row.get(0),
        )
        .expect("the rolled back table stays absent");
    assert_eq!(oops, 0);
}

#[test]
fn a_failed_non_transactional_script_keeps_its_own_error() {
    let mut backend = backend();
    backend.acquire_lock().expect("the write lock is taken");

    // `execute_batch` stops at the failing statement, so the `BEGIN` the script
    // opened stays active while the error of the malformed DDL is already known.
    let error = backend
        .execute_non_transactional("BEGIN; CREATE TABLE oops (;")
        .expect_err("the malformed statement fails the script");

    let message = match &error {
        Error::Backend(message) => message.clone(),
        other => panic!("unexpected error: {other:?}"),
    };
    assert!(
        message.contains("syntax error"),
        "the driver's own failure must reach the operator: {message}"
    );
    assert!(
        message.contains("left a transaction open"),
        "the leftover transaction is still reported as context: {message}"
    );

    // The rollback runs before the error is reported, so the connection stays
    // usable for recording the failed attempt.
    assert!(
        backend.conn.is_autocommit(),
        "the leftover transaction is rolled back"
    );
    backend
        .conn
        .execute_batch("CREATE TABLE users (id INTEGER NOT NULL);")
        .expect("the connection remains usable");
}

#[test]
fn a_script_that_never_commits_is_reported_as_an_imbalance() {
    let mut backend = backend();
    backend.acquire_lock().expect("the write lock is taken");

    // Every statement of the script succeeds, so the imbalance is the only
    // thing wrong with it and no driver error exists to report.
    let error = backend
        .execute_non_transactional("BEGIN; CREATE TABLE users (id INTEGER NOT NULL);")
        .expect_err("the script succeeded but never closed its transaction");

    let message = match &error {
        Error::Backend(message) => message.clone(),
        other => panic!("unexpected error: {other:?}"),
    };
    assert!(
        message.starts_with("non-transactional script left a transaction open"),
        "the imbalance is reported on its own: {message}"
    );

    let users: i64 = backend
        .conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name = 'users'",
            [],
            |row| row.get(0),
        )
        .expect("the rolled back table stays absent");
    assert_eq!(users, 0);
}

#[test]
fn the_composed_diagnostic_reports_the_imbalance_alone() {
    let message = left_open_message(Ok(()), Ok(()));

    assert!(
        message.starts_with("non-transactional script left a transaction open"),
        "{message}"
    );
    assert!(
        message.contains("balance the BEGIN statements"),
        "{message}"
    );
    assert!(!message.contains("failed"), "{message}");
}

#[test]
fn the_composed_diagnostic_keeps_the_script_cause_over_a_successful_rollback() {
    let message = left_open_message(
        Err(Error::Backend("near \"oops\": syntax error".to_owned())),
        Ok(()),
    );

    assert!(
        message.starts_with("near \"oops\": syntax error"),
        "{message}"
    );
    assert!(
        message.contains("its applied statements were rolled back"),
        "{message}"
    );
    assert!(!message.contains("rolling it back failed"), "{message}");
}

#[test]
fn the_composed_diagnostic_reports_a_rollback_that_could_not_undo_the_script() {
    let message = left_open_message(Ok(()), Err(Error::Backend("disk I/O error".to_owned())));

    assert!(
        message.starts_with("non-transactional script left a transaction open"),
        "{message}"
    );
    assert!(
        message.contains("rolling it back failed: disk I/O error"),
        "{message}"
    );
    assert!(message.contains("fresh connection"), "{message}");
}

#[test]
fn the_composed_diagnostic_carries_both_the_script_and_the_rollback_failure() {
    let message = left_open_message(
        Err(Error::Backend("near \"oops\": syntax error".to_owned())),
        Err(Error::Backend("disk I/O error".to_owned())),
    );

    assert!(
        message.starts_with("near \"oops\": syntax error"),
        "{message}"
    );
    assert!(
        message.contains("rolling it back failed: disk I/O error"),
        "{message}"
    );
    assert!(
        message.contains("balance the BEGIN statements"),
        "{message}"
    );
}

#[test]
fn a_backend_a_failed_rollback_left_unusable_reports_the_cause_instead_of_stepping() {
    let mut backend = backend();
    backend.poisoned = Some(
        "disk I/O error; the script also left a transaction open and rolling it back failed"
            .to_owned(),
    );

    let error = match backend.begin() {
        Ok(_) => panic!("a poisoned backend opens no step"),
        Err(error) => error,
    };
    let message = match &error {
        Error::Backend(message) => message.clone(),
        other => panic!("unexpected error: {other:?}"),
    };
    assert!(message.contains("unusable"), "{message}");
    assert!(message.contains("disk I/O error"), "{message}");

    let error = backend
        .execute_non_transactional("CREATE TABLE users (id INTEGER NOT NULL);")
        .expect_err("a poisoned backend executes no script");
    let message = match &error {
        Error::Backend(message) => message.clone(),
        other => panic!("unexpected error: {other:?}"),
    };
    assert!(message.contains("unusable"), "{message}");
    assert!(message.contains("rolling it back failed"), "{message}");
}

/// A fresh database file no concurrent test can collide with.
fn scratch_database(label: &str) -> std::path::PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let sequence = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut path = std::env::temp_dir();
    path.push(format!(
        "rurqe-driver-{label}-{}-{sequence}.db",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    path
}

#[test]
fn the_write_lock_excludes_a_second_runner_on_the_same_database() {
    let path = scratch_database("lock");

    let mut first = SqliteMigrationBackend::new(
        rusqlite::Connection::open(&path).expect("the first connection opens"),
    );
    let mut second = SqliteMigrationBackend::new(
        rusqlite::Connection::open(&path).expect("the second connection opens"),
    )
    .with_lock_timeout(Duration::ZERO);

    first
        .acquire_lock()
        .expect("the first runner takes the write lock");

    let error = second
        .acquire_lock()
        .expect_err("the second runner cannot take a held write lock");
    match &error {
        Error::Backend(message) => assert!(message.contains("write lock"), "{message}"),
        other => panic!("expected a backend failure, got {other:?}"),
    }

    first
        .release_lock()
        .expect("the first runner publishes and releases the lock");
    second
        .acquire_lock()
        .expect("the released lock is handed to the second runner");
    second
        .release_lock()
        .expect("the second runner releases the lock");

    drop(first);
    drop(second);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn the_advisory_lock_is_taken_on_the_first_attempt() {
    let mut calls = 0u32;
    let attempts = poll_advisory_lock(
        MIGRATION_LOCK_ID,
        Duration::from_secs(1),
        Duration::ZERO,
        &NoopSleeper,
        || {
            calls += 1;
            Ok(true)
        },
    )
    .expect("the free lock is taken at once");

    assert_eq!(attempts, 1);
    assert_eq!(calls, 1);
}

#[test]
fn the_advisory_lock_is_polled_until_it_becomes_free() {
    let mut pending = 2u32;
    let attempts = poll_advisory_lock(
        MIGRATION_LOCK_ID,
        Duration::from_secs(1),
        Duration::ZERO,
        &NoopSleeper,
        || {
            if pending == 0 {
                Ok(true)
            } else {
                pending -= 1;
                Ok(false)
            }
        },
    )
    .expect("the lock becomes free after the earlier rounds fail");

    assert_eq!(attempts, 3);
}

#[test]
fn every_waiting_round_pauses_through_the_configured_sleeper() {
    let sleeper = RecordingSleeper::new();
    let interval = Duration::from_millis(5);
    let mut pending = 2u32;

    let attempts = poll_advisory_lock(
        MIGRATION_LOCK_ID,
        Duration::from_secs(1),
        interval,
        &sleeper,
        || {
            if pending == 0 {
                Ok(true)
            } else {
                pending -= 1;
                Ok(false)
            }
        },
    )
    .expect("the lock becomes free after two waiting rounds");

    // The round that takes the lock never pauses, so only the two rejected
    // rounds ask the sleeper to wait.
    assert_eq!(attempts, 3);
    assert_eq!(sleeper.pauses(), vec![interval, interval]);
}

#[test]
fn a_held_advisory_lock_times_out_with_its_identifier() {
    let error = poll_advisory_lock(
        MIGRATION_LOCK_ID,
        Duration::ZERO,
        Duration::ZERO,
        &NoopSleeper,
        || Ok(false),
    )
    .expect_err("a lock nobody releases times out");

    let message = match error {
        Error::Backend(message) => message,
        other => panic!("unexpected error: {other:?}"),
    };

    assert!(
        message.contains(&MIGRATION_LOCK_ID.to_string()),
        "{message}"
    );
    assert!(
        message.contains(&format!("0x{MIGRATION_LOCK_ID:016x}")),
        "{message}"
    );
}

#[test]
fn a_backend_failure_stops_the_polling() {
    let error = poll_advisory_lock(
        MIGRATION_LOCK_ID,
        Duration::from_secs(1),
        Duration::ZERO,
        &NoopSleeper,
        || Err(Error::Backend("connection lost".to_owned())),
    )
    .expect_err("a failing backend is reported unchanged");

    assert_eq!(error, Error::Backend("connection lost".to_owned()));
}

#[test]
fn a_confirmed_unlock_forgets_the_lock() {
    let mut held = true;

    apply_unlock_answer(MIGRATION_LOCK_ID, &mut held, Ok(true))
        .expect("the confirmed release succeeds");

    assert!(!held, "a freed lock is not retried");
}

#[test]
fn an_unlock_the_session_never_held_is_reported() {
    let mut held = true;

    let error = apply_unlock_answer(MIGRATION_LOCK_ID, &mut held, Ok(false))
        .expect_err("a lock the session never held is reported, not treated as released");

    let message = match &error {
        Error::Backend(message) => message.clone(),
        other => panic!("unexpected error: {other:?}"),
    };
    assert!(
        message.contains(&MIGRATION_LOCK_ID.to_string()),
        "{message}"
    );
    assert!(
        message.contains(&format!("0x{MIGRATION_LOCK_ID:016x}")),
        "{message}"
    );
    assert!(
        !held,
        "the session answered, so no lock is left for `Drop` to retry"
    );
}

#[test]
fn an_unanswered_unlock_keeps_the_lock_for_the_drop_retry() {
    let mut held = true;

    let error = apply_unlock_answer(
        MIGRATION_LOCK_ID,
        &mut held,
        Err(Error::Backend("connection lost".to_owned())),
    )
    .expect_err("an unanswered statement is reported");

    assert_eq!(error, Error::Backend("connection lost".to_owned()));
    assert!(
        held,
        "a statement without an answer leaves the lock held so `Drop` retries it"
    );
}
