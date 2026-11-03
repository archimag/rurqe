//! Server-independent tests of the migration protocol and the catalog queries.
//!
//! A PostgreSQL daemon would answer every question the protocol asks, so the
//! scenarios here exercise the parts that need no server: the advisory lock takes
//! its rounds from a closure, the unlock decision from an explicit answer, the
//! history decoding from a raw row, and the catalog statements from their own
//! text. The suite therefore runs wherever the crate is built with the `migrate`
//! feature, and the server-facing behaviour stays with the scenarios that name a
//! live database.

// A skipped scenario reports itself to the operator running the tests, which is
// the one place the workspace-wide printing ban is lifted for this file.
#![allow(clippy::print_stdout)]
#![cfg(feature = "migrate")]

use std::sync::Mutex;
use std::time::Duration;

use rurqe_postgres::introspect;
use rurqe_postgres::migrate;
use rurqe_postgres::rurqe_migrate;
use rurqe_postgres::rurqe_migrate::error::Error;
use rurqe_postgres::rurqe_migrate::sleeper::{NoopSleeper, Sleeper};

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

#[test]
fn the_advisory_lock_is_taken_on_the_first_attempt() {
    let mut calls = 0u32;

    let attempts = migrate::poll_advisory_lock(
        migrate::MIGRATION_LOCK_ID,
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

    let attempts = migrate::poll_advisory_lock(
        migrate::MIGRATION_LOCK_ID,
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

    let attempts = migrate::poll_advisory_lock(
        migrate::MIGRATION_LOCK_ID,
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
    let lock_id = migrate::MIGRATION_LOCK_ID;

    let error = migrate::poll_advisory_lock(
        lock_id,
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

    assert!(message.contains(&lock_id.to_string()), "{message}");
    assert!(message.contains(&format!("0x{lock_id:016x}")), "{message}");
}

#[test]
fn a_backend_failure_stops_the_polling() {
    let error = migrate::poll_advisory_lock(
        migrate::MIGRATION_LOCK_ID,
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

    migrate::apply_unlock_answer(migrate::MIGRATION_LOCK_ID, &mut held, Ok(true))
        .expect("the confirmed release succeeds");

    assert!(!held, "a freed lock is not retried");
}

#[test]
fn an_unlock_the_session_never_held_is_reported() {
    let lock_id = migrate::MIGRATION_LOCK_ID;
    let mut held = true;

    let error = migrate::apply_unlock_answer(lock_id, &mut held, Ok(false))
        .expect_err("an unheld lock is reported");

    let message = match &error {
        Error::Backend(message) => message.clone(),
        other => panic!("unexpected error: {other:?}"),
    };
    assert!(message.contains(&lock_id.to_string()), "{message}");
    assert!(message.contains(&format!("0x{lock_id:016x}")), "{message}");
    assert!(!held, "the session answered, so `Drop` retries nothing");
}

#[test]
fn an_unanswered_unlock_keeps_the_lock_for_the_drop_retry() {
    let mut held = true;

    let error = migrate::apply_unlock_answer(
        migrate::MIGRATION_LOCK_ID,
        &mut held,
        Err(Error::Backend("connection lost".to_owned())),
    )
    .expect_err("an unanswered statement is reported");

    assert_eq!(error, Error::Backend("connection lost".to_owned()));
    assert!(held, "the lock stays held so `Drop` retries the release");
}

/// The raw row of a versioned migration recorded successfully.
fn versioned_row() -> migrate::RawHistoryRow {
    let checksum = rurqe_migrate::Checksum::of(b"CREATE TABLE orders ();");

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
    assert_eq!(entry.kind, rurqe_migrate::MigrationKind::Versioned);
    assert_eq!(entry.script, "V2_1__add_orders.sql");
    assert_eq!(
        entry.checksum,
        Some(rurqe_migrate::Checksum::of(b"CREATE TABLE orders ();"))
    );
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
    assert_eq!(entry.kind, rurqe_migrate::MigrationKind::Baseline);
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

#[test]
fn the_history_statements_bind_the_recorded_columns() {
    let columns = migrate::HISTORY_COLUMN_COUNT;

    let insert = migrate::insert_entry_sql();
    assert!(insert.contains("$1"), "{insert}");
    assert!(
        insert.contains(&format!("${columns}")),
        "the insert binds every recorded column: {insert}"
    );
    assert!(
        !insert.contains(&format!("${}", columns + 1)),
        "the insert binds no further parameter: {insert}"
    );

    let select = migrate::select_history_sql();
    assert!(select.contains("ORDER BY installed_rank"), "{select}");
    assert!(!select.contains('$'), "the select binds nothing: {select}");
}

#[test]
fn the_delete_and_update_statements_bind_their_parameters() {
    let delete = migrate::delete_entry_sql();
    assert!(delete.contains("installed_rank = $1"), "{delete}");

    let update = migrate::update_checksum_sql();
    assert!(update.contains("checksum = $1"), "{update}");
    assert!(update.contains("script = $2"), "{update}");
}

#[test]
fn the_catalog_statements_filter_the_schema_list() {
    assert!(introspect::POSTGRES_TABLES.contains("= ANY($1)"));
    assert!(introspect::POSTGRES_COLUMNS.contains("= ANY($1)"));
    assert!(introspect::POSTGRES_ENUMS.contains("= ANY($1)"));
    assert!(introspect::POSTGRES_PRIMARY_KEYS.contains("= ANY($1)"));
    assert!(introspect::POSTGRES_FOREIGN_KEYS.contains("= ANY($1)"));
    assert!(introspect::POSTGRES_INDEXES.contains("= ANY($1)"));
    assert!(introspect::POSTGRES_CHECKS.contains("= ANY($1)"));
    assert!(introspect::POSTGRES_VIEWS.contains("= ANY($1)"));
    assert!(introspect::POSTGRES_TRIGGERS.contains("= ANY($1)"));

    assert!(!introspect::POSTGRES_TABLES.contains("= $1"));
}

#[test]
fn the_catalog_statements_project_the_schema_of_every_row() {
    assert!(introspect::POSTGRES_TABLES.contains("SELECT table_schema, table_name"));
    assert!(introspect::POSTGRES_COLUMNS.contains("column_row.table_schema,"));
    assert!(introspect::POSTGRES_COLUMNS.contains("END AS udt_schema"));
    assert!(introspect::POSTGRES_ENUMS.contains("n.nspname AS schema_name"));
    assert!(introspect::POSTGRES_PRIMARY_KEYS.contains("kcu.table_schema, kcu.table_name"));
    assert!(introspect::POSTGRES_FOREIGN_KEYS.contains("namespace.nspname AS table_schema"));
    assert!(introspect::POSTGRES_INDEXES.contains("namespace.nspname AS table_schema"));
    assert!(introspect::POSTGRES_CHECKS.contains("namespace.nspname AS table_schema"));
    assert!(introspect::POSTGRES_VIEWS.contains("SELECT table_schema, table_name AS view_name"));
    assert!(introspect::POSTGRES_TRIGGERS.contains("namespace.nspname AS table_schema"));
}

#[test]
fn the_index_statement_reads_only_the_key_attributes() {
    let statement = introspect::POSTGRES_INDEXES;

    assert!(statement.contains("index_row.indnkeyatts >= position.index"));
    assert!(statement.contains("LEFT JOIN pg_attribute attribute"));
    assert!(statement.contains("access_method.amname"));
    assert!(statement.contains("pg_get_expr(index_row.indpred"));
    assert!(statement.contains("pg_get_expr(index_row.indexprs"));
}

#[test]
fn the_foreign_key_statement_reads_the_referenced_schema() {
    let statement = introspect::POSTGRES_FOREIGN_KEYS;

    assert!(statement.contains("referenced_namespace.nspname AS referenced_schema"));
    assert!(statement.contains("namespace.nspname::text = ANY($1)"));
    assert!(statement.contains("constraint_row.confdeltype"));
    assert!(statement.contains("constraint_row.confupdtype"));
}

#[test]
fn the_trigger_statement_skips_the_internal_triggers() {
    assert!(introspect::POSTGRES_TRIGGERS.contains("NOT trigger_row.tgisinternal"));
    assert!(introspect::POSTGRES_TRIGGERS.contains("NOT trigger_row.tgisinternal"));
}

#[test]
fn the_view_statement_aliases_the_name_information_schema_carries() {
    // `information_schema.views` carries the name of a view in `table_name`
    // and defines no `view_name` column, so a statement selecting one is
    // refused by every server; the alias keeps the label a decoded row
    // carries.
    assert!(introspect::POSTGRES_VIEWS.contains("table_name AS view_name"));
}
#[test]
fn the_schema_neighbour_statement_ignores_a_populated_selection() {
    let statement = introspect::POSTGRES_SCHEMA_NEIGHBOURS;

    assert!(statement.contains("selected.relations = 0"));
    assert!(statement.contains("table_name != '__rurqe_schema_history'"));
    assert!(statement.contains("NOT IN ('pg_catalog', 'information_schema')"));
    assert!(statement.contains("table_row.table_schema::text <> ALL($1)"));
}

#[test]
fn an_undecodable_catalog_column_names_the_query_and_the_column() {
    let error = introspect::column_error("columns", "column_default", "unexpected type");

    let message = error.to_string();
    assert!(message.contains("`column_default`"), "{message}");
    assert!(message.contains("columns query"), "{message}");
    assert!(message.contains("unexpected type"), "{message}");
}

/// The URL of the server the server-bound scenarios run against.
///
/// A scenario that cannot run reports itself and returns, so an environment
/// without PostgreSQL runs the suite to completion instead of failing it.
fn database_url(scenario: &str) -> Option<String> {
    match std::env::var("RURQE_TEST_POSTGRES_URL") {
        Ok(url) if !url.trim().is_empty() => Some(url),
        _ => {
            println!("skipping `{scenario}`: RURQE_TEST_POSTGRES_URL is not set");
            None
        }
    }
}

/// Migration scenarios driven through the client backend.
///
/// Every scenario needs a live server named by `RURQE_TEST_POSTGRES_URL` and
/// reports a skip when it is absent, like the other server-bound scenarios of this
/// suite. Each one owns a schema of its own, because the scenarios of one binary
/// run concurrently and a shared schema would let the history table, the fixtures,
/// and the recorded digest of one scenario observe the work of another.
mod client_backend {
    use postgres::NoTls;

    use rurqe_postgres::migrate::PgConnectionMigrationBackend;
    use rurqe_postgres::rurqe_core::query::TransactionGuard;
    use rurqe_postgres::rurqe_migrate::runner::{MigrationBackend, MigrationTransaction};
    use rurqe_postgres::rurqe_migrate::{Migration, Migrator};
    use rurqe_postgres::PgConnection;

    use super::database_url;

    /// Connects to the scenario server and prepares a schema of its own.
    ///
    /// The schema becomes the `search_path` of the session, so the history table
    /// and the tables of a scenario land inside it, and the backend is told the
    /// same schema through `with_schema`, so the catalog digest it records
    /// describes that schema alone.
    fn fixture(scenario: &str, schema: &str) -> Option<PgConnection> {
        let url = database_url(scenario)?;
        let mut client = postgres::Client::connect(&url, NoTls)
            .expect("the test server accepts the connection");

        client
            .batch_execute(&format!(
                "DROP SCHEMA IF EXISTS {schema} CASCADE; CREATE SCHEMA {schema}"
            ))
            .expect("the fixture schema is created");
        client
            .batch_execute(&format!("SET search_path TO {schema}"))
            .expect("the fixture schema is selected");

        Some(PgConnection::new(client))
    }

    #[test]
    fn a_client_connection_applies_every_script_once() {
        let Some(mut connection) = fixture(
            "PgConnectionMigrationBackend::migrate",
            "rurqe_client_apply",
        ) else {
            return;
        };

        let migrations = vec![
            Migration::parse("V1__users.sql", "CREATE TABLE users (id INTEGER PRIMARY KEY);")
                .expect("a valid script name"),
            Migration::parse("V2__orders.sql", "CREATE TABLE orders (id INTEGER PRIMARY KEY);")
                .expect("a valid script name"),
        ];

        let first = {
            let mut backend = PgConnectionMigrationBackend::new(&mut connection)
                .with_schema("rurqe_client_apply");
            Migrator::new()
                .migrate(&mut backend, &migrations)
                .expect("the client connection applies the tail")
        };

        assert_eq!(first.applied_count, 2);
        assert_eq!(
            first.applied_scripts,
            vec!["V1__users.sql", "V2__orders.sql"]
        );

        let second = {
            let mut backend = PgConnectionMigrationBackend::new(&mut connection)
                .with_schema("rurqe_client_apply");
            Migrator::new()
                .migrate(&mut backend, &migrations)
                .expect("a second run finds nothing pending")
        };

        assert_eq!(second.applied_count, 0, "an applied script never runs twice");
        assert!(second.applied_scripts.is_empty());

        let recorded = connection
            .client_mut()
            .query_one(
                "SELECT COUNT(*) FROM rurqe_client_apply.__rurqe_schema_history",
                &[],
            )
            .expect("the recorded history is readable through the connection")
            .get::<_, i64>(0);

        assert_eq!(recorded, 2, "both scripts left one history row each");
    }

    #[test]
    fn the_client_backend_reports_the_catalog_digest_of_its_schema() {
        let Some(mut connection) = fixture(
            "PgConnectionMigrationBackend::catalog_checksum",
            "rurqe_client_catalog",
        ) else {
            return;
        };

        let migrations = vec![Migration::parse(
            "V1__users.sql",
            "CREATE TABLE users (id INTEGER PRIMARY KEY);",
        )
        .expect("a valid script name")];

        let before = {
            let mut backend = PgConnectionMigrationBackend::new(&mut connection)
                .with_schema("rurqe_client_catalog");
            Migrator::new()
                .migrate(&mut backend, &migrations)
                .expect("the tail applies through the client connection");
            backend
                .catalog_checksum()
                .expect("the selected schema is introspectable")
                .expect("the client backend reports a catalog digest")
        };

        connection
            .client_mut()
            .batch_execute("ALTER TABLE rurqe_client_catalog.users ADD COLUMN email TEXT")
            .expect("the schema moves on");

        let after = {
            let mut backend = PgConnectionMigrationBackend::new(&mut connection)
                .with_schema("rurqe_client_catalog");
            backend
                .catalog_checksum()
                .expect("the selected schema is introspectable")
                .expect("the client backend reports a catalog digest")
        };

        assert_ne!(before, after, "the digest follows the catalog it describes");
    }

    #[test]
    fn a_client_step_nests_inside_the_callers_savepoint() {
        let Some(mut connection) = fixture(
            "PgConnectionMigrationBackend::begin savepoint nesting",
            "rurqe_client_nested",
        ) else {
            return;
        };

        connection
            .client_mut()
            .batch_execute("CREATE TABLE outer_rows (id INTEGER PRIMARY KEY)")
            .expect("the fixture table is created");

        let mut guard = TransactionGuard::begin(&mut connection)
            .expect("the caller opens its own transaction");
        guard
            .client_mut()
            .batch_execute("INSERT INTO outer_rows (id) VALUES (1)")
            .expect("the first row of the caller is written");

        {
            let mut backend = PgConnectionMigrationBackend::new(&mut guard)
                .with_schema("rurqe_client_nested");
            let mut step = backend
                .begin()
                .expect("the step nests inside a savepoint of the caller");
            step.execute("CREATE TABLE nested_rows (id INTEGER PRIMARY KEY);")
                .expect("the nested script applies");
            step.rollback().expect("the nested block rolls back");
        }

        guard
            .client_mut()
            .batch_execute("INSERT INTO outer_rows (id) VALUES (2)")
            .expect("the second row of the caller is written");
        guard.commit().expect("the caller commits its own transaction");

        let nested = connection
            .client_mut()
            .query_one(
                "SELECT to_regclass('rurqe_client_nested.nested_rows') IS NOT NULL",
                &[],
            )
            .expect("the catalog answers")
            .get::<_, bool>(0);
        assert!(!nested, "the nested block rolled back its own script");

        let rows = connection
            .client_mut()
            .query_one("SELECT COUNT(*) FROM rurqe_client_nested.outer_rows", &[])
            .expect("the caller's table is readable")
            .get::<_, i64>(0);
        assert_eq!(rows, 2, "the enclosing transaction kept the caller's work");
    }
}

/// Migration scenarios driven through the backend that owns its session.
///
/// Every scenario needs a live server named by `RURQE_TEST_POSTGRES_URL` and
/// reports a skip when it is absent. The adapter owns the client it drives, so a
/// scenario prepares the fixture schema on that very session, while the result it
/// left behind is read back through a second connection of its own.
mod standalone_backend {
    use postgres::NoTls;

    use rurqe_postgres::migrate::PostgresMigrationBackend;
    use rurqe_postgres::rurqe_migrate::runner::{MigrationBackend, MigrationTransaction};
    use rurqe_postgres::rurqe_migrate::{Migration, Migrator};

    use super::database_url;

    /// Connects a blocking client to the server of the scenario.
    fn connect(url: &str) -> postgres::Client {
        postgres::Client::connect(url, NoTls).expect("the test server accepts the connection")
    }

    /// Builds the adapter over a session whose fixture schema is prepared.
    ///
    /// The schema becomes the `search_path` of the session, so the history table
    /// and the tables of a scenario land inside it, and the backend is told the
    /// same schema through `with_schema`, so the catalog digest it records
    /// describes that schema alone.
    fn fixture(url: &str, schema: &str) -> PostgresMigrationBackend {
        let mut client = connect(url);
        client
            .batch_execute(&format!(
                "DROP SCHEMA IF EXISTS {schema} CASCADE; CREATE SCHEMA {schema}"
            ))
            .expect("the fixture schema is created");
        client
            .batch_execute(&format!("SET search_path TO {schema}"))
            .expect("the fixture schema is selected");

        PostgresMigrationBackend::new(client).with_schema(schema)
    }

    /// Connects a second session that reads what the adapter has written.
    ///
    /// Selecting the fixture schema makes the recorded history table and the
    /// committed tables reachable under the names the migration scripts use.
    fn probe(url: &str, schema: &str) -> postgres::Client {
        let mut client = connect(url);
        client
            .batch_execute(&format!("SET search_path TO {schema}"))
            .expect("the fixture schema is selected");
        client
    }

    #[test]
    fn an_owned_session_applies_every_script_once() {
        let Some(url) = database_url("PostgresMigrationBackend::migrate") else {
            return;
        };
        let schema = "rurqe_standalone_apply";
        let mut backend = fixture(&url, schema);

        let migrations = vec![
            Migration::parse("V1__users.sql", "CREATE TABLE users (id INTEGER PRIMARY KEY);")
                .expect("a valid script name"),
            Migration::parse("V2__orders.sql", "CREATE TABLE orders (id INTEGER PRIMARY KEY);")
                .expect("a valid script name"),
        ];

        let first = Migrator::new()
            .migrate(&mut backend, &migrations)
            .expect("the owned session applies the tail");

        assert_eq!(first.applied_count, 2);
        assert_eq!(
            first.applied_scripts,
            vec!["V1__users.sql", "V2__orders.sql"]
        );

        let second = Migrator::new()
            .migrate(&mut backend, &migrations)
            .expect("a second run finds nothing pending");

        assert_eq!(second.applied_count, 0, "an applied script never runs twice");
        assert!(second.applied_scripts.is_empty());

        let recorded = probe(&url, schema)
            .query_one("SELECT COUNT(*) FROM __rurqe_schema_history", &[])
            .expect("the recorded history is readable")
            .get::<_, i64>(0);

        assert_eq!(recorded, 2, "both scripts left one history row each");
    }

    #[test]
    fn the_owned_session_records_the_catalog_digest_of_its_schema() {
        let Some(url) = database_url("PostgresMigrationBackend::catalog_checksum") else {
            return;
        };
        let schema = "rurqe_standalone_catalog";
        let mut backend = fixture(&url, schema);

        let migrations = vec![Migration::parse(
            "V1__users.sql",
            "CREATE TABLE users (id INTEGER PRIMARY KEY);",
        )
        .expect("a valid script name")];

        Migrator::new()
            .migrate(&mut backend, &migrations)
            .expect("the tail applies through the owned session");

        let before = backend
            .catalog_checksum()
            .expect("the selected schema is introspectable")
            .expect("the backend reports a catalog digest");

        // The catalog moves on through a step of the adapter itself, so the
        // second digest describes a change the adapter committed.
        let mut step = backend.begin().expect("the step opens its transaction");
        step.execute("ALTER TABLE users ADD COLUMN email TEXT;")
            .expect("the script applies inside the step");
        step.commit().expect("the step commits its script");

        let after = backend
            .catalog_checksum()
            .expect("the selected schema is introspectable")
            .expect("the backend reports a catalog digest");

        assert_ne!(before, after, "the digest follows the catalog it describes");
    }

    #[test]
    fn an_abandoned_owned_step_rolls_back() {
        let Some(url) = database_url("PostgresMigrationBackend abandoned step") else {
            return;
        };
        let schema = "rurqe_standalone_abandoned";
        let mut backend = fixture(&url, schema);

        {
            let mut step = backend.begin().expect("the step opens its transaction");
            step.execute("CREATE TABLE abandoned_rows (id INTEGER PRIMARY KEY);")
                .expect("the script applies inside the step");
            // The step is dropped without a terminal call, so the adapter rolls
            // its transaction back instead of leaving it open.
        }

        let survived = probe(&url, schema)
            .query_one("SELECT to_regclass('abandoned_rows') IS NOT NULL", &[])
            .expect("the catalog answers")
            .get::<_, bool>(0);

        assert!(!survived, "the abandoned step rolled its own script back");
    }
}

/// Migration scenarios driven through the blocking adapter over an asynchronous
/// connection.
///
/// Every scenario needs a live server named by `RURQE_TEST_POSTGRES_URL` and
/// reports a skip when it is absent. The connection task of a scenario runs on the
/// worker threads of a multi-threaded runtime while the migration itself blocks
/// the thread of the scenario, which is the arrangement the adapter expects: the
/// runtime stays free to drive the socket the protocol awaits.
#[cfg(feature = "tokio-postgres")]
mod async_backend {
    use rurqe_postgres::migrate::PgAsyncMigrationBackend;
    use rurqe_postgres::rurqe_migrate::error::Error;
    use rurqe_postgres::rurqe_migrate::runner::{MigrationBackend, MigrationTransaction};
    use rurqe_postgres::rurqe_migrate::{Migration, Migrator};
    use rurqe_postgres::PgAsyncConnection;
    use tokio_postgres::NoTls;

    use super::database_url;

    /// The multi-threaded runtime that drives the connection of a scenario.
    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("the scenario runtime builds")
    }

    /// Connects asynchronously and prepares a schema of its own.
    ///
    /// The schema becomes the `search_path` of the session, so the history table
    /// and the tables of a scenario land inside it, and the backend is told the
    /// same schema through `with_schema`, so the catalog digest it records
    /// describes that schema alone.
    async fn connect(url: &str, schema: &str) -> PgAsyncConnection {
        let (client, connection) = tokio_postgres::connect(url, NoTls)
            .await
            .expect("the test server accepts the asynchronous connection");
        // The task drives the socket until the client is dropped, which is what
        // lets the adapter await its statements.
        tokio::spawn(async move {
            let _ = connection.await;
        });

        client
            .batch_execute(&format!(
                "DROP SCHEMA IF EXISTS {schema} CASCADE; CREATE SCHEMA {schema}"
            ))
            .await
            .expect("the fixture schema is created");
        client
            .batch_execute(&format!("SET search_path TO {schema}"))
            .await
            .expect("the fixture schema is selected");

        PgAsyncConnection::new(client)
    }

    /// Connects a blocking client to the fixture of a scenario.
    fn blocking_client(url: &str, schema: &str) -> postgres::Client {
        let mut client = postgres::Client::connect(url, NoTls)
            .expect("the test server accepts the blocking connection");
        client
            .batch_execute(&format!("SET search_path TO {schema}"))
            .expect("the fixture schema is selected");
        client
    }

    #[test]
    fn an_async_connection_applies_every_script_once() {
        let Some(url) = database_url("PgAsyncMigrationBackend::migrate") else {
            return;
        };
        let runtime = runtime();
        let schema = "rurqe_async_apply";
        let connection = runtime.block_on(connect(&url, schema));
        let mut backend = PgAsyncMigrationBackend::new(connection)
            .expect("the migration worker starts")
            .with_schema(schema);

        let migrations = vec![
            Migration::parse("V1__users.sql", "CREATE TABLE users (id INTEGER PRIMARY KEY);")
                .expect("a valid script name"),
            Migration::parse("V2__orders.sql", "CREATE TABLE orders (id INTEGER PRIMARY KEY);")
                .expect("a valid script name"),
        ];

        let first = Migrator::new()
            .migrate(&mut backend, &migrations)
            .expect("the asynchronous connection applies the tail");

        assert_eq!(first.applied_count, 2);
        assert_eq!(
            first.applied_scripts,
            vec!["V1__users.sql", "V2__orders.sql"]
        );

        let second = Migrator::new()
            .migrate(&mut backend, &migrations)
            .expect("a second run finds nothing pending");

        assert_eq!(second.applied_count, 0, "an applied script never runs twice");
        assert!(second.applied_scripts.is_empty());

        // The backend owns the session, so dropping it closes the connection and
        // the history it recorded is read through a client of the scenario.
        drop(backend);

        let recorded = blocking_client(&url, schema)
            .query_one("SELECT COUNT(*) FROM __rurqe_schema_history", &[])
            .expect("the recorded history is readable")
            .get::<_, i64>(0);

        assert_eq!(recorded, 2, "both scripts left one history row each");
    }

    #[test]
    fn the_async_backend_reports_the_catalog_digest_of_its_schema() {
        let Some(url) = database_url("PgAsyncMigrationBackend::catalog_checksum") else {
            return;
        };
        let runtime = runtime();
        let schema = "rurqe_async_catalog";
        let connection = runtime.block_on(connect(&url, schema));
        let mut backend = PgAsyncMigrationBackend::new(connection)
            .expect("the migration worker starts")
            .with_schema(schema);

        let migrations = vec![Migration::parse(
            "V1__users.sql",
            "CREATE TABLE users (id INTEGER PRIMARY KEY);",
        )
        .expect("a valid script name")];

        Migrator::new()
            .migrate(&mut backend, &migrations)
            .expect("the tail applies through the asynchronous connection");

        let before = backend
            .catalog_checksum()
            .expect("the selected schema is introspectable")
            .expect("the asynchronous backend reports a catalog digest");

        blocking_client(&url, schema)
            .batch_execute("ALTER TABLE users ADD COLUMN email TEXT")
            .expect("the schema moves on");

        let after = backend
            .catalog_checksum()
            .expect("the selected schema is introspectable")
            .expect("the asynchronous backend reports a catalog digest");

        assert_ne!(before, after, "the digest follows the catalog it describes");
    }

    #[test]
    fn an_abandoned_async_step_rolls_back() {
        let Some(url) = database_url("PgAsyncMigrationBackend abandoned step") else {
            return;
        };
        let runtime = runtime();
        let schema = "rurqe_async_abandoned";
        let connection = runtime.block_on(connect(&url, schema));
        let mut backend = PgAsyncMigrationBackend::new(connection)
            .expect("the migration worker starts")
            .with_schema(schema);

        {
            let mut step = backend.begin().expect("the step opens its transaction");
            step.execute("CREATE TABLE abandoned_rows (id INTEGER PRIMARY KEY);")
                .expect("the script applies inside the step");
            // The step is dropped without a terminal call, so the adapter rolls
            // its transaction back instead of leaving it open.
        }

        let survived = blocking_client(&url, schema)
            .query_one("SELECT to_regclass('abandoned_rows') IS NOT NULL", &[])
            .expect("the catalog answers")
            .get::<_, bool>(0);

        assert!(!survived, "the abandoned step rolled its own script back");
    }

    /// Whether `error` is the worker refusing a boundary over an open
    /// transaction.
    fn is_transaction_open(error: &Error) -> bool {
        matches!(
            error,
            Error::Backend(message) if message.contains("already holds an open transaction")
        )
    }

    #[test]
    fn a_refused_rollback_keeps_the_session_inside_its_transaction() {
        let Some(url) = database_url("PgAsyncMigrationBackend refused rollback") else {
            return;
        };
        let runtime = runtime();
        let schema = "rurqe_async_refused_rollback";
        let connection = runtime.block_on(connect(&url, schema));
        let mut backend = PgAsyncMigrationBackend::new(connection)
            .expect("the migration worker starts")
            .with_schema(schema);

        let mut step = backend.begin().expect("the step opens its transaction");
        // The statement ends the session, so the rollback the worker issues
        // below reaches a connection that no longer exists. The statement may
        // report the loss of the session it causes, which is why its own outcome
        // is not asserted.
        let _ = step.execute("SELECT pg_terminate_backend(pg_backend_pid())");

        let error = step
            .rollback()
            .expect_err("the dead session never accepts the rollback");
        assert!(
            matches!(&error, Error::Backend(_)),
            "the refused rollback reports the driver failure: {error:?}"
        );

        // The flag survived the unproven rollback, so the worker refuses the
        // next boundary before it touches the dead socket instead of opening a
        // transaction inside the abandoned one.
        let refused = match backend.begin() {
            Ok(_) => panic!("the worker refuses a boundary over an open transaction"),
            Err(error) => error,
        };
        assert!(is_transaction_open(&refused), "got {refused:?}");
    }

    #[test]
    fn a_refused_compensating_rollback_keeps_the_session_inside_its_transaction() {
        let Some(url) = database_url("PgAsyncMigrationBackend refused commit") else {
            return;
        };
        let runtime = runtime();
        let schema = "rurqe_async_refused_commit";
        let connection = runtime.block_on(connect(&url, schema));
        let mut backend = PgAsyncMigrationBackend::new(connection)
            .expect("the migration worker starts")
            .with_schema(schema);

        let mut step = backend.begin().expect("the step opens its transaction");
        // The statement ends the session, so neither the commit nor the rollback
        // the worker issues to compensate for it reaches a live connection.
        let _ = step.execute("SELECT pg_terminate_backend(pg_backend_pid())");

        let error = step
            .commit()
            .expect_err("the dead session never accepts the commit");
        assert!(
            matches!(&error, Error::Backend(_)),
            "the refused commit reaches the caller: {error:?}"
        );

        // Neither terminal statement proved the session idle, so the next
        // boundary is refused instead of persisting both steps' work in one
        // transaction.
        let refused = match backend.begin() {
            Ok(_) => panic!("the worker refuses a boundary over an open transaction"),
            Err(error) => error,
        };
        assert!(is_transaction_open(&refused), "got {refused:?}");
    }

    /// The pooled form checks its connection out of the pool on the worker thread
    /// and holds it for the whole run.
    #[cfg(feature = "deadpool-postgres")]
    #[test]
    fn a_pooled_async_backend_applies_the_tail() {
        use deadpool_postgres::{Manager, ManagerConfig, Pool, Runtime};
        use rurqe_postgres::PgAsyncPool;

        let Some(url) = database_url("PgAsyncMigrationBackend::from_pool") else {
            return;
        };

        let runtime = runtime();
        // The pooled session runs in `public`, so the fixture is reset there under
        // names of its own: another scenario of this binary keeps its own schema.
        let fixture = runtime.block_on(async {
            let (client, connection) = tokio_postgres::connect(&url, NoTls)
                .await
                .expect("the test server accepts the asynchronous connection");
            tokio::spawn(async move {
                let _ = connection.await;
            });
            client
                .batch_execute(
                    "DROP TABLE IF EXISTS rurqe_pool_users; \
                     DROP TABLE IF EXISTS rurqe_pool_orders; \
                     DROP TABLE IF EXISTS __rurqe_schema_history",
                )
                .await
                .expect("the pooled fixture is reset");
            PgAsyncConnection::new(client)
        });
        drop(fixture);

        let config: tokio_postgres::Config = url.parse().expect("the test URL parses");
        let manager = Manager::from_config(config, NoTls, ManagerConfig::default());
        let pool = Pool::builder(manager)
            .max_size(1)
            .runtime(Runtime::Tokio1)
            .build()
            .expect("a one-connection pool builds");

        let mut backend = PgAsyncMigrationBackend::from_pool(PgAsyncPool::new(pool))
            .expect("the migration worker checks a connection out");

        let migrations = vec![
            Migration::parse(
                "V1__users.sql",
                "CREATE TABLE rurqe_pool_users (id INTEGER PRIMARY KEY);",
            )
            .expect("a valid script name"),
            Migration::parse(
                "V2__orders.sql",
                "CREATE TABLE rurqe_pool_orders (id INTEGER PRIMARY KEY);",
            )
            .expect("a valid script name"),
        ];

        let report = Migrator::new()
            .migrate(&mut backend, &migrations)
            .expect("the pooled backend applies the tail");

        assert_eq!(report.applied_count, 2);

        let mut client = postgres::Client::connect(&url, NoTls)
            .expect("the test server accepts the blocking connection");
        let recorded = client
            .query_one("SELECT COUNT(*) FROM __rurqe_schema_history", &[])
            .expect("the recorded history is readable")
            .get::<_, i64>(0);

        assert_eq!(recorded, 2, "both scripts left one history row each");
    }
}