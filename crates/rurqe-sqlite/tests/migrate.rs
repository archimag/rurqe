//! SQLite migration lifecycle tests.
//!
//! The suite needs no server: every scenario opens its own in-memory database, or a
//! scratch file where a second runner has to observe the write lock of the first
//! one. The coverage mirrors what the toolchain relied on while the protocol lived
//! in the command line crate.

#![cfg(feature = "migrate")]

use std::time::Duration;

use rurqe_sqlite::SqliteConnection;
use rurqe_sqlite::migrate::{self, SqliteConnectionMigrationBackend, SqliteMigrationBackend};
use rurqe_sqlite::rurqe_migrate::error::Error;
use rurqe_sqlite::rurqe_migrate::runner::{MigrationBackend, MigrationTransaction};
use rurqe_sqlite::rurqe_migrate::{Checksum, Migration, MigrationKind, Migrator};
use rurqe_sqlite::rusqlite;

/// A backend over a fresh in-memory database.
fn backend() -> SqliteMigrationBackend {
    SqliteMigrationBackend::new(
        rusqlite::Connection::open_in_memory().expect("the in-memory database opens"),
    )
}

/// The scripts of a scenario holding the single migration `script`.
fn migrations(script: &str, sql: &str) -> Vec<Migration> {
    vec![Migration::parse(script, sql).expect("a valid script name")]
}

#[test]
fn sqlite_backend_applies_versioned_migrations_once() {
    let mut backend = backend();
    let scripts = migrations("V1__init.sql", "CREATE TABLE users (id INTEGER);");

    let report = Migrator::new()
        .migrate(&mut backend, &scripts)
        .expect("the tail applies");
    assert_eq!(report.applied_count, 1);

    let report = Migrator::new()
        .migrate(&mut backend, &scripts)
        .expect("a second run finds nothing pending");
    assert_eq!(report.applied_count, 0);
    assert!(report.applied_scripts.is_empty());

    let history = backend.read_history().expect("the history is readable");
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
    assert!(matches!(error, Error::MigrationExecutionFailed { .. }));

    // The failing script left no table behind: its work was rolled back with the
    // transaction of the step.
    let created = backend
        .connection()
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name = 'oops'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .expect("the catalog answers");
    assert_eq!(created, 0, "the failed script leaves no table behind");

    let history = backend.read_history().expect("history is readable");
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].script, "V1__broken.sql");
    assert!(!history[0].success);

    let blocked = Migrator::new()
        .migrate(&mut backend, &broken)
        .expect_err("the failed entry blocks the next run");
    assert!(matches!(
        blocked,
        Error::FailedMigrationAlreadyApplied { .. }
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
        .connection()
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name = 'users'",
            [],
            |row| row.get(0),
        )
        .expect("the committed table is visible");
    assert_eq!(users, 1);

    let oops: i64 = backend
        .connection()
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
    assert!(!backend.is_poisoned(), "a successful rollback poisons nothing");
    backend
        .connection()
        .execute_batch("CREATE TABLE users (id INTEGER NOT NULL);")
        .expect("the connection remains usable");
}

#[test]
fn a_script_that_never_commits_is_reported_as_an_imbalance() {
    let mut backend = backend();
    backend.acquire_lock().expect("the write lock is taken");

    // Every statement of the script succeeds, so the imbalance is the only thing
    // wrong with it and no driver error exists to report.
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
    assert!(!backend.is_poisoned(), "a successful rollback poisons nothing");

    let users: i64 = backend
        .connection()
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name = 'users'",
            [],
            |row| row.get(0),
        )
        .expect("the rolled back table stays absent");
    assert_eq!(users, 0);
}

/// A fresh database file no concurrent test can collide with.
fn scratch_database(label: &str) -> std::path::PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let sequence = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut path = std::env::temp_dir();
    path.push(format!(
        "rurqe-sqlite-{label}-{}-{sequence}.db",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    path
}

#[test]
fn the_write_lock_excludes_a_second_runner_on_the_same_database() {
    let path = scratch_database("lock");

    let mut first =
        SqliteMigrationBackend::new(rusqlite::Connection::open(&path).expect("the first connection opens"));
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
fn the_history_statements_bind_the_recorded_columns() {
    let columns = migrate::HISTORY_COLUMN_COUNT;

    let insert = migrate::insert_entry_sql();
    assert!(insert.contains("?1"), "{insert}");
    assert!(
        insert.contains(&format!("?{columns}")),
        "the insert binds every recorded column: {insert}"
    );
    assert!(
        !insert.contains(&format!("?{}", columns + 1)),
        "the insert binds no further parameter: {insert}"
    );

    let select = migrate::select_history_sql();
    assert!(select.contains("ORDER BY installed_rank"), "{select}");
    assert!(!select.contains('?'), "the select binds nothing: {select}");
}

#[test]
fn the_delete_and_update_statements_bind_their_parameters() {
    let delete = migrate::delete_entry_sql();
    assert!(delete.contains("installed_rank = ?1"), "{delete}");

    let update = migrate::update_checksum_sql();
    assert!(update.contains("checksum = ?1"), "{update}");
    assert!(update.contains("script = ?2"), "{update}");
}

/// The raw row of a versioned migration recorded successfully.
fn versioned_row() -> migrate::RawHistoryRow {
    let checksum = Checksum::of(b"CREATE TABLE orders ();");

    migrate::RawHistoryRow {
        installed_rank: 1,
        version: Some("2.1".to_owned()),
        description: "add orders".to_owned(),
        kind: "VERSIONED".to_owned(),
        script: "V2_1__add_orders.sql".to_owned(),
        checksum: Some(checksum.to_hex()),
        installed_on: "2024-01-01 00:00:00".to_owned(),
        execution_time_ms: 12,
        success: true,
    }
}

#[test]
fn a_versioned_history_row_decodes_into_its_entry() {
    let entry = versioned_row().decode().expect("a well-formed row decodes");

    let version = entry.version.as_ref().map(ToString::to_string);
    assert_eq!(entry.installed_rank, 1);
    assert_eq!(version.as_deref(), Some("2.1"));
    assert_eq!(entry.description, "add orders");
    assert_eq!(entry.kind, MigrationKind::Versioned);
    assert_eq!(entry.script, "V2_1__add_orders.sql");
    assert_eq!(entry.checksum, Some(Checksum::of(b"CREATE TABLE orders ();")));
    assert_eq!(entry.execution_time_ms, 12);
    assert!(entry.success);
}

#[test]
fn a_baseline_row_carries_no_version() {
    let row = migrate::RawHistoryRow {
        installed_rank: 1,
        version: None,
        description: "the pruned boundary".to_owned(),
        kind: "BASELINE".to_owned(),
        script: "<< Baseline >>".to_owned(),
        checksum: None,
        installed_on: "2024-01-01 00:00:00".to_owned(),
        execution_time_ms: 0,
        success: true,
    };

    let entry = row.decode().expect("a baseline row decodes");

    assert_eq!(entry.version, None);
    assert_eq!(entry.kind, MigrationKind::Baseline);
    assert_eq!(entry.checksum, None);
}

#[test]
fn an_unknown_recorded_kind_is_rejected() {
    let mut row = versioned_row();
    row.kind = "SOMETHING".to_owned();

    let error = row.decode().expect_err("an unknown kind is rejected");

    assert!(
        matches!(&error, Error::Backend(message) if message.contains("SOMETHING")),
        "got {error:?}"
    );
}

#[test]
fn a_malformed_recorded_version_is_rejected() {
    let mut row = versioned_row();
    row.version = Some("2.a".to_owned());

    let error = row.decode().expect_err("a malformed version is rejected");

    assert!(
        matches!(&error, Error::InvalidVersion { version } if version == "2.a"),
        "got {error:?}"
    );
}

#[test]
fn a_malformed_recorded_checksum_is_rejected() {
    let mut row = versioned_row();
    row.checksum = Some("abc".to_owned());

    let error = row.decode().expect_err("a malformed checksum is rejected");

    assert!(
        matches!(&error, Error::InvalidChecksum { .. }),
        "got {error:?}"
    );
}

/// A client adapter over a fresh in-memory database.
fn client_connection() -> SqliteConnection {
    SqliteConnection::new(
        rusqlite::Connection::open_in_memory().expect("the in-memory database opens"),
    )
}

#[test]
fn sqlite_client_backend_applies_versioned_migrations_once() {
    let mut connection = client_connection();
    let scripts = migrations("V1__init.sql", "CREATE TABLE users (id INTEGER);");

    {
        let mut backend = SqliteConnectionMigrationBackend::new(&mut connection);
        let report = Migrator::new()
            .migrate(&mut backend, &scripts)
            .expect("the tail applies");
        assert_eq!(report.applied_count, 1);

        let history = backend.read_history().expect("the history is readable");
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].script, "V1__init.sql");
        assert_eq!(history[0].kind, MigrationKind::Versioned);
        assert!(history[0].success);
    }

    // The run lock is committed when the run ends, so the client keeps a
    // connection in autocommit mode.
    assert!(connection.connection().is_autocommit());

    {
        let mut backend = SqliteConnectionMigrationBackend::new(&mut connection);
        let report = Migrator::new()
            .migrate(&mut backend, &scripts)
            .expect("a second run finds nothing pending");
        assert_eq!(report.applied_count, 0);
        assert!(report.applied_scripts.is_empty());
    }
}

#[test]
fn sqlite_client_backend_rolls_back_failed_migrations() {
    let mut connection = client_connection();
    let broken = migrations("V1__broken.sql", "CREATE TABLE oops (;");

    {
        let mut backend = SqliteConnectionMigrationBackend::new(&mut connection);
        let error = Migrator::new()
            .migrate(&mut backend, &broken)
            .expect_err("the broken script fails to execute");
        assert!(matches!(error, Error::MigrationExecutionFailed { .. }));

        let history = backend.read_history().expect("the history is readable");
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].script, "V1__broken.sql");
        assert!(!history[0].success);
    }

    // The failing script left no table behind: its work was rolled back to the
    // savepoint of the step.
    let created: i64 = connection
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name = 'oops'",
            [],
            |row| row.get(0),
        )
        .expect("the catalog answers");
    assert_eq!(created, 0, "the failed script leaves no table behind");

    let migrator = Migrator::new();
    let fixed = migrations("V1__fixed.sql", "CREATE TABLE ok (id INTEGER);");
    let mut backend = SqliteConnectionMigrationBackend::new(&mut connection);

    let blocked = migrator
        .migrate(&mut backend, &broken)
        .expect_err("the failed entry blocks the next run");
    assert!(matches!(
        blocked,
        Error::FailedMigrationAlreadyApplied { .. }
    ));

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
fn the_client_run_lock_nests_every_step_in_a_savepoint() {
    let mut connection = client_connection();

    {
        let mut backend = SqliteConnectionMigrationBackend::new(&mut connection);
        backend.acquire_lock().expect("the write lock is taken");

        {
            let mut step = backend.begin().expect("a step opens inside the lock");
            step.execute("CREATE TABLE users (id INTEGER NOT NULL);")
                .expect("the step applies");
            step.commit().expect("the step is released");
        }

        {
            let mut step = backend.begin().expect("a second step opens inside the lock");
            step.execute("CREATE TABLE oops (;")
                .expect_err("the broken statement fails");
            step.rollback().expect("the step is rolled back");
        }

        backend.release_lock().expect("the lock commits the run");
    }

    let users: i64 = connection
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name = 'users'",
            [],
            |row| row.get(0),
        )
        .expect("the committed table is visible");
    assert_eq!(users, 1);

    let oops: i64 = connection
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name = 'oops'",
            [],
            |row| row.get(0),
        )
        .expect("the rolled back table stays absent");
    assert_eq!(oops, 0);
}

#[test]
fn a_caller_transaction_serves_as_the_run_lock() {
    let mut connection = client_connection();
    connection
        .connection_mut()
        .execute_batch("BEGIN IMMEDIATE")
        .expect("the caller opens a transaction");

    let scripts = migrations("V1__users.sql", "CREATE TABLE users (id INTEGER NOT NULL);");

    {
        let mut backend = SqliteConnectionMigrationBackend::new(&mut connection);
        backend
            .acquire_lock()
            .expect("the caller transaction serialises the run");
        let report = Migrator::new()
            .migrate(&mut backend, &scripts)
            .expect("the tail applies inside the caller transaction");
        assert_eq!(report.applied_count, 1);
        backend
            .release_lock()
            .expect("the release leaves the caller transaction alone");
    }

    // The run committed nothing on the caller's behalf: the migration work becomes
    // visible exactly when the caller commits.
    assert!(!connection.connection().is_autocommit());

    connection
        .connection_mut()
        .execute_batch("COMMIT")
        .expect("the caller commits");

    let users: i64 = connection
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name = 'users'",
            [],
            |row| row.get(0),
        )
        .expect("the committed table is visible");
    assert_eq!(users, 1);
}

#[test]
fn a_non_transactional_script_is_refused_inside_a_caller_transaction() {
    let mut connection = client_connection();
    connection
        .connection_mut()
        .execute_batch("BEGIN IMMEDIATE")
        .expect("the caller opens a transaction");

    let mut backend = SqliteConnectionMigrationBackend::new(&mut connection);
    backend
        .acquire_lock()
        .expect("the caller transaction serialises the run");

    let error = backend
        .execute_non_transactional("CREATE TABLE users (id INTEGER NOT NULL);")
        .expect_err("the statements of such a script belong outside any transaction");

    let message = match &error {
        Error::Backend(message) => message.clone(),
        other => panic!("unexpected error: {other:?}"),
    };
    assert!(message.contains("non-transactional"), "{message}");
}

#[test]
fn sqlite_client_backend_reports_the_digest_of_its_catalog() {
    let mut connection = client_connection();
    let scripts = migrations(
        "V1__users.sql",
        "CREATE TABLE users (id INTEGER NOT NULL PRIMARY KEY, email TEXT NOT NULL);",
    );

    let mut backend = SqliteConnectionMigrationBackend::new(&mut connection);
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