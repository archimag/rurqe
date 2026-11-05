//! Application-side path of the embedded migration lifecycle.
//!
//! A project bakes its migration artifacts at build time with
//! `embed_migrations!`, and this file drives the surface a binary uses at run
//! time: the facade re-exports the collection, and the driver feature of the
//! connection the application already holds brings the migration backend that
//! applies it. Nothing here names the command-line crate, so a service migrates
//! through the very handle it queries with.
//!
//! The file is compiled only when both halves are present — the collection
//! (`migrate`) and the SQLite adapter (`sqlite`) — which is the pairing the
//! default feature set enables.

#![cfg(all(feature = "sqlite", feature = "migrate"))]

use rurqe::migrate::{EmbeddedArtifacts, EmbeddedMigrations, MigrationBackend};
use rurqe::sqlite::migrate::SqliteConnectionMigrationBackend;
use rurqe::sqlite::rusqlite::Connection;
use rurqe::sqlite::SqliteConnection;

/// The `users` table the fixture collections declare.
const CREATE_USERS: &str =
    "CREATE TABLE users (id INTEGER NOT NULL PRIMARY KEY, name TEXT NOT NULL);";

/// The `orders` table the fixture tail adds beside `users`.
const CREATE_ORDERS: &str = "CREATE TABLE orders (
    id INTEGER NOT NULL PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES users (id)
);";

/// Opens an empty in-memory database wrapped in the facade adapter.
fn database() -> SqliteConnection {
    SqliteConnection::new(Connection::open_in_memory().expect("the in-memory database opens"))
}

#[test]
fn the_embedded_tail_applies_through_the_facade_connection() {
    let embedded = EmbeddedMigrations::new(&[
        ("V1__users.sql", CREATE_USERS),
        ("V2__orders.sql", CREATE_ORDERS),
    ])
    .expect("valid migrations");

    let mut connection = database();
    let mut backend = SqliteConnectionMigrationBackend::new(&mut connection);

    let report = embedded
        .migrate(&mut backend)
        .expect("the collection applies through the facade");

    assert_eq!(
        report.applied_scripts,
        vec!["V1__users.sql".to_owned(), "V2__orders.sql".to_owned()]
    );
    assert_eq!(report.applied_count, 2);
    assert!(
        report.cold_start.is_none(),
        "a collection without a stage initialises nothing"
    );

    let history = backend.read_history().expect("the history is readable");
    let scripts = history
        .iter()
        .map(|entry| entry.script.as_str())
        .collect::<Vec<_>>();
    assert_eq!(scripts, vec!["V1__users.sql", "V2__orders.sql"]);
    assert!(history.iter().all(|entry| entry.success));
    assert!(history.iter().all(|entry| entry.checksum.is_some()));

    // The run lock never outlives the run, so the handle stays usable and the
    // DDL of the tail really reached the database.
    drop(backend);
    connection
        .execute_batch(
            "INSERT INTO users (id, name) VALUES (1, 'root');
             INSERT INTO orders (id, user_id) VALUES (1, 1);",
        )
        .expect("the tail created both tables");
}

#[test]
fn a_baked_stage_cold_starts_an_empty_database_through_the_facade_connection() {
    let embedded = EmbeddedMigrations::from_artifacts(EmbeddedArtifacts {
        tail: &[],
        checkpoint: None,
        stage: &[("users.sql", CREATE_USERS)],
        schema_lock: None,
    })
    .expect("valid artifacts");

    let mut connection = database();
    let mut backend = SqliteConnectionMigrationBackend::new(&mut connection);

    let report = embedded
        .migrate(&mut backend)
        .expect("the stage initialises the empty database");

    let cold_start = report
        .cold_start
        .expect("an empty database is initialised from the stage");
    assert_eq!(cold_start.stage_scripts, vec!["users.sql".to_owned()]);
    assert_eq!(cold_start.installed_rank, Some(1));
    assert!(report.applied_scripts.is_empty());

    // The applied marker anchors the digest the client backend introspected,
    // and it is the only entry of a run that applied a stage.
    let history = backend.read_history().expect("the history is readable");
    assert_eq!(history.len(), 1);
    let marker = history.first().expect("the cold start records its marker");
    assert!(marker.success);
    assert!(
        marker.checksum.is_some(),
        "the marker anchors a catalog digest"
    );
}

#[test]
fn the_sqlite_migration_protocol_travels_with_the_driver_feature() {
    // The history statement of the engine is reachable through the facade,
    // which is what makes a SQLite connection a migration target for an
    // application that never links the command-line crate.
    let statement = rurqe::sqlite::migrate::select_history_sql();
    assert!(statement.contains("__rurqe_schema_history"), "{statement}");
}

#[cfg(feature = "postgres")]
#[test]
fn the_postgres_migration_protocol_travels_with_the_driver_feature() {
    // The protocol statements of the engine are reachable through the facade,
    // so `embedded.migrate(&mut conn)` over a synchronous PostgreSQL connection
    // needs no adapter crate of its own; applying a collection needs a server,
    // so nothing is dialled here.
    let statement = rurqe::postgres::migrate::select_history_sql();
    assert!(statement.contains("__rurqe_schema_history"), "{statement}");
    assert_ne!(rurqe::postgres::migrate::MIGRATION_LOCK_ID, 0);
}

/// The asynchronous lifecycle stays reachable for a backend that owns its run.
///
/// `migrate_async` runs the very same protocol on a dedicated blocking thread,
/// which is what an asynchronous caller drives instead of blocking its worker.
/// The future is built and dropped without being polled: reaching the method
/// needs only the pairing of the collection with a `Send` backend, while
/// awaiting it needs a runtime this check does not start.
#[cfg(feature = "tokio")]
#[test]
fn the_asynchronous_lifecycle_stays_reachable_for_an_owned_backend() {
    /// Reports whether a value may cross a thread boundary.
    fn is_send<T: Send>(_: &T) {}

    let embedded =
        EmbeddedMigrations::new(&[("V1__users.sql", CREATE_USERS)]).expect("valid migrations");
    let backend = rurqe::sqlite::migrate::SqliteMigrationBackend::new(
        Connection::open_in_memory().expect("the in-memory database opens"),
    );

    let pending = embedded.migrate_async(backend);
    is_send(&pending);
}
