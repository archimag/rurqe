//! Integration tests for non-transactional migration scripts.
//!
//! Transactional scripts share the migration transaction, so their statements
//! commit or roll back together. A script marked non-transactional commits every
//! statement on its own, exactly as PostgreSQL demands for `CREATE INDEX
//! CONCURRENTLY`. These tests verify the mix end to end: scripts are loaded from
//! a directory, applied through the migration runner, recorded in
//! `__rurqe_schema_history`, and replayed by the Golden Equation verifier.
//!
//! The PostgreSQL scenarios need a live database and are skipped when neither
//! `TEST_DATABASE_URL` nor `DATABASE_URL` names one. They expect the connected
//! role to hold the `CREATEDB` privilege — the golden equation builds its replay
//! in a scratch database — and are skipped when the privilege is absent.

use std::fs;
use std::path::{Path, PathBuf};

use rurqe_cli::{
    load_migrations_from_dir, GoldenEquation, PostgresMigrationBackend, SqliteMigrationBackend,
    StageError,
};
use rurqe_migrate::error::Error;
use rurqe_migrate::migration::Migration;
use rurqe_migrate::runner::{MigrationBackend, Migrator};
use rurqe_migrate::stage::{DeclarativeStage, StageScript};

/// The users table shared by the SQLite and PostgreSQL scenarios.
const USERS: &str = "CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT NOT NULL);";

/// An orders table needed to prove a later transactional script still lands.
const ORDERS: &str = "CREATE TABLE orders (id INTEGER PRIMARY KEY, user_id INTEGER NOT NULL);";

/// An index over `users.email` applied transactionally on SQLite.
const INDEX: &str = "CREATE INDEX users_email_idx ON users (email);";

/// The same index declared non-transactional through a header directive.
const DIRECTIVE_INDEX: &str =
    "-- executeInTransaction=false\nCREATE INDEX users_email_idx ON users (email);";

/// A PostgreSQL stage declaring both the table and the index.
const PG_USERS_WITH_INDEX: &str =
    "CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT NOT NULL);\n\
                                   CREATE INDEX users_email_idx ON users (email);";

/// The PostgreSQL concurrent index, replayable only outside a transaction.
const PG_CONCURRENT_INDEX: &str = "CREATE INDEX CONCURRENTLY users_email_idx ON users (email);";

/// The concurrent index with the directive that keeps it out of the transaction.
const PG_DIRECTIVE_CONCURRENT_INDEX: &str =
    "-- executeInTransaction=false\nCREATE INDEX CONCURRENTLY users_email_idx ON users (email);";

/// Creates a fresh, empty workspace directory for one test.
fn temp_workspace(label: &str) -> PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    let sequence = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    let mut dir = std::env::temp_dir();
    dir.push(format!(
        "rurqe-notx-it-{label}-{}-{nanos}-{sequence}",
        std::process::id()
    ));
    if dir.exists() {
        let _ = fs::remove_dir_all(&dir);
    }
    fs::create_dir_all(&dir).expect("create the temp workspace");
    dir
}

/// Writes migration scripts into `dir`, creating it if needed.
fn write_scripts(dir: &Path, scripts: &[(&str, &str)]) {
    fs::create_dir_all(dir).expect("create the script directory");
    for (name, sql) in scripts {
        fs::write(dir.join(name), sql).expect("write the script");
    }
}

/// Returns the number of rows of `sqlite_master` matching the given filters.
fn sqlite_master_count(db: &Path, kind: &str, name: &str) -> i64 {
    let connection = rusqlite::Connection::open(db).expect("open the sqlite database");
    connection
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type = ?1 AND name = ?2",
            rusqlite::params![kind, name],
            |row| row.get::<_, i64>(0),
        )
        .expect("count the sqlite objects")
}

#[test]
fn mixed_transactional_and_non_transactional_scripts_apply_in_one_run() {
    let workspace = temp_workspace("sqlite-mixed");
    let migrations_dir = workspace.join("migrations");
    write_scripts(
        &migrations_dir,
        &[
            ("V1__users.sql", USERS),
            ("V2__index.notx.sql", INDEX),
            ("V3__orders.sql", ORDERS),
        ],
    );

    let tail = load_migrations_from_dir(&migrations_dir).expect("the scripts are loaded");
    assert_eq!(tail.len(), 3);
    assert!(
        tail[0].is_transactional(),
        "V1 stays inside the transaction"
    );
    assert!(
        !tail[1].is_transactional(),
        "the `.notx.sql` suffix keeps V2 outside the transaction"
    );
    assert!(
        tail[2].is_transactional(),
        "V3 stays inside the transaction"
    );

    let db = workspace.join("app.db");
    let connection = rusqlite::Connection::open(&db).expect("open the sqlite database");
    let mut backend = SqliteMigrationBackend::new(connection);

    let report = Migrator::new()
        .migrate(&mut backend, &tail)
        .expect("every script applies under its declared mode");

    assert_eq!(
        report.applied_scripts,
        vec!["V1__users.sql", "V2__index.notx.sql", "V3__orders.sql"]
    );

    let history = backend.read_history().expect("the history is readable");
    assert_eq!(history.len(), 3);
    assert!(history.iter().all(|entry| entry.success));

    assert_eq!(sqlite_master_count(&db, "table", "users"), 1);
    assert_eq!(sqlite_master_count(&db, "table", "orders"), 1);
    assert_eq!(sqlite_master_count(&db, "index", "users_email_idx"), 1);
}

#[test]
fn the_header_directive_keeps_a_script_out_of_the_transaction() {
    let workspace = temp_workspace("sqlite-directive");
    let migrations_dir = workspace.join("migrations");
    write_scripts(
        &migrations_dir,
        &[("V1__users.sql", USERS), ("V2__index.sql", DIRECTIVE_INDEX)],
    );

    let tail = load_migrations_from_dir(&migrations_dir).expect("the scripts are loaded");
    assert!(tail[0].is_transactional());
    assert!(
        !tail[1].is_transactional(),
        "an `executeInTransaction=false` comment disables the transaction"
    );

    let db = workspace.join("app.db");
    let connection = rusqlite::Connection::open(&db).expect("open the sqlite database");
    let mut backend = SqliteMigrationBackend::new(connection);

    let report = Migrator::new()
        .migrate(&mut backend, &tail)
        .expect("the directive keeps the index replayable");

    assert_eq!(report.applied_count, 2);
    assert_eq!(sqlite_master_count(&db, "index", "users_email_idx"), 1);
}

#[test]
fn a_failing_transactional_script_keeps_earlier_non_transactional_work() {
    let workspace = temp_workspace("sqlite-partial-failure");
    let migrations_dir = workspace.join("migrations");
    write_scripts(
        &migrations_dir,
        &[
            ("V1__users.sql", USERS),
            ("V2__index.notx.sql", INDEX),
            ("V3__broken.sql", "CREATE TABLE broken (;"),
        ],
    );

    let tail = load_migrations_from_dir(&migrations_dir).expect("the scripts are loaded");
    let db = workspace.join("app.db");
    let connection = rusqlite::Connection::open(&db).expect("open the sqlite database");
    let mut backend = SqliteMigrationBackend::new(connection);

    let error = Migrator::new()
        .migrate(&mut backend, &tail)
        .expect_err("the malformed DDL aborts the run");
    assert!(
        matches!(error, Error::MigrationExecutionFailed { .. }),
        "got {error:?}"
    );

    let history = backend.read_history().expect("the history is readable");
    let applied: Vec<&str> = history
        .iter()
        .filter(|entry| entry.success)
        .map(|entry| entry.script.as_str())
        .collect();
    assert_eq!(applied, vec!["V1__users.sql", "V2__index.notx.sql"]);
    assert!(
        history
            .iter()
            .any(|entry| entry.script == "V3__broken.sql" && !entry.success),
        "the failed attempt is recorded"
    );

    assert_eq!(
        sqlite_master_count(&db, "index", "users_email_idx"),
        1,
        "the non-transactional index stays committed"
    );
    assert_eq!(
        sqlite_master_count(&db, "table", "broken"),
        0,
        "the transactional script rolled back"
    );
}

// -------------------------------------------------------------------------
// PostgreSQL scenarios — skipped when no test database is available.
// -------------------------------------------------------------------------

/// The database URL the PostgreSQL scenarios run against, if one is configured.
fn postgres_url() -> Option<String> {
    let raw = std::env::var("TEST_DATABASE_URL")
        .or_else(|_| std::env::var("DATABASE_URL"))
        .ok()?;
    let url = raw.trim().to_owned();

    if url.is_empty() {
        return None;
    }

    match postgres::Client::connect(&url, postgres::NoTls) {
        Ok(client) => {
            drop(client);
            Some(url)
        }
        Err(error) => {
            eprintln!(
                "skipping the PostgreSQL scenario: cannot connect to the configured database: {error}"
            );
            None
        }
    }
}

/// Whether the golden equation failed only because the role cannot create databases.
fn is_createdb_missing(message: &str) -> bool {
    message.contains("CREATEDB")
}

/// A scratch schema in the configured database, so tests never share state.
struct SchemaFixture {
    admin: postgres::Client,
    name: String,
}

impl SchemaFixture {
    /// Creates a unique schema, returning `None` and printing a reason on failure.
    fn create(url: &str, label: &str) -> Option<Self> {
        let mut admin = match postgres::Client::connect(url, postgres::NoTls) {
            Ok(admin) => admin,
            Err(error) => {
                eprintln!("skipping the PostgreSQL scenario `{label}`: {error}");
                return None;
            }
        };

        let name = unique_schema_name(label);
        let quoted = quote_ident(&name);

        if let Err(error) = admin.batch_execute(&format!("CREATE SCHEMA {quoted}")) {
            eprintln!("skipping the PostgreSQL scenario `{label}`: {error}");
            return None;
        }

        Some(Self { admin, name })
    }

    /// Opens a second connection whose `search_path` points at the fixture schema.
    fn client(&self, url: &str, label: &str) -> Option<postgres::Client> {
        let mut client = match postgres::Client::connect(url, postgres::NoTls) {
            Ok(client) => client,
            Err(error) => {
                eprintln!("skipping the PostgreSQL scenario `{label}`: {error}");
                return None;
            }
        };

        let quoted = quote_ident(&self.name);
        if let Err(error) = client.batch_execute(&format!("SET search_path TO {quoted}")) {
            eprintln!("skipping the PostgreSQL scenario `{label}`: {error}");
            return None;
        }

        Some(client)
    }

    /// Whether the named index exists in the fixture schema.
    fn index_exists(&mut self, index: &str) -> bool {
        self.admin
            .query_one(
                "SELECT count(*) FROM pg_indexes WHERE schemaname = $1 AND indexname = $2",
                &[&self.name, &index],
            )
            .map(|row| row.get::<_, i64>(0) > 0)
            .unwrap_or(false)
    }
}

impl Drop for SchemaFixture {
    fn drop(&mut self) {
        let quoted = quote_ident(&self.name);
        if let Err(error) = self
            .admin
            .batch_execute(&format!("DROP SCHEMA IF EXISTS {quoted} CASCADE"))
        {
            eprintln!(
                "failed to drop the PostgreSQL test schema `{}`: {error}",
                self.name
            );
        }
    }
}

/// Hands out a schema name no concurrent test can collide with.
fn unique_schema_name(label: &str) -> String {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let sequence = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);

    format!(
        "rurqe_notx_{label}_{}_{nanos}_{sequence}",
        std::process::id()
    )
    .replace('-', "_")
}

/// Renders `name` as a quoted PostgreSQL identifier.
fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

#[test]
fn postgres_concurrent_index_inside_a_transaction_is_rejected() {
    let Some(url) = postgres_url() else {
        return;
    };
    let Some(fixture) = SchemaFixture::create(&url, "pg_tx_fail") else {
        return;
    };
    let Some(client) = fixture.client(&url, "pg_tx_fail") else {
        return;
    };

    let tail = vec![
        Migration::parse("V1__users.sql", USERS).expect("a valid script name"),
        Migration::parse("V2__index.sql", PG_CONCURRENT_INDEX).expect("a valid script name"),
    ];

    let mut backend = PostgresMigrationBackend::new(client);
    let error = Migrator::new()
        .migrate(&mut backend, &tail)
        .expect_err("a concurrent index cannot run inside a transaction");

    assert!(
        matches!(error, Error::MigrationExecutionFailed { .. }),
        "got {error:?}"
    );
    assert!(error.to_string().contains("CONCURRENTLY"), "got {error}");

    let history = backend.read_history().expect("the history is readable");
    let applied: Vec<&str> = history
        .iter()
        .filter(|entry| entry.success)
        .map(|entry| entry.script.as_str())
        .collect();
    assert_eq!(applied, vec!["V1__users.sql"]);
}

#[test]
fn postgres_concurrent_index_runs_outside_a_transaction_when_declared() {
    let Some(url) = postgres_url() else {
        return;
    };
    let Some(mut fixture) = SchemaFixture::create(&url, "pg_tx_ok") else {
        return;
    };
    let Some(client) = fixture.client(&url, "pg_tx_ok") else {
        return;
    };

    let tail = vec![
        Migration::parse("V1__users.sql", USERS).expect("a valid script name"),
        Migration::parse("V2__index.sql", PG_DIRECTIVE_CONCURRENT_INDEX)
            .expect("a valid script name"),
    ];

    let mut backend = PostgresMigrationBackend::new(client);
    let report = Migrator::new()
        .migrate(&mut backend, &tail)
        .expect("the declared non-transactional index applies");

    assert_eq!(
        report.applied_scripts,
        vec!["V1__users.sql", "V2__index.sql"]
    );

    let history = backend.read_history().expect("the history is readable");
    assert_eq!(history.len(), 2);
    assert!(history.iter().all(|entry| entry.success));

    drop(backend);
    assert!(
        fixture.index_exists("users_email_idx"),
        "the concurrent index is present in the fixture schema"
    );
}

#[test]
fn postgres_golden_equation_rejects_a_transactional_concurrent_index() {
    let Some(url) = postgres_url() else {
        return;
    };

    let stage = DeclarativeStage::new(vec![StageScript::new("users.sql", USERS)]);
    let tail = vec![
        Migration::parse("V1__users.sql", USERS).expect("a valid script name"),
        Migration::parse("V2__index.sql", PG_CONCURRENT_INDEX).expect("a valid script name"),
    ];

    match GoldenEquation::new(&stage, &tail).verify(&url) {
        Err(StageError::Script { script, message }) => {
            assert_eq!(script, "V2__index.sql");
            assert!(message.contains("CONCURRENTLY"), "got {message}");
        }
        Err(StageError::Sandbox { message }) if is_createdb_missing(&message) => {
            eprintln!("skipping the PostgreSQL golden equation scenario: {message}");
        }
        Err(other) => panic!("expected a script failure, got {other:?}"),
        Ok(_) => panic!("a concurrent index inside a transaction must be rejected"),
    }
}

#[test]
fn postgres_golden_equation_accepts_a_declared_concurrent_index() {
    let Some(url) = postgres_url() else {
        return;
    };

    let stage = DeclarativeStage::new(vec![StageScript::new("users.sql", PG_USERS_WITH_INDEX)]);
    let tail = vec![
        Migration::parse("V1__users.sql", USERS).expect("a valid script name"),
        Migration::parse("V2__index.sql", PG_DIRECTIVE_CONCURRENT_INDEX)
            .expect("a valid script name"),
    ];

    match GoldenEquation::new(&stage, &tail).verify(&url) {
        Ok(report) => assert!(
            report.is_equivalent(),
            "the stage and the replayed tail converge"
        ),
        Err(StageError::Sandbox { message }) if is_createdb_missing(&message) => {
            eprintln!("skipping the PostgreSQL golden equation scenario: {message}");
        }
        Err(other) => panic!("expected the equation to hold, got {other}"),
    }
}
