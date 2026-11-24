//! Live PostgreSQL regressions for the destructive schema reset.

#![cfg(feature = "migrate")]
#![allow(clippy::print_stdout)]

use std::sync::atomic::{AtomicU64, Ordering};

use postgres::NoTls;
use rurqe_postgres::migrate::{PgConnectionMigrationBackend, PostgresMigrationBackend, MIGRATION_LOCK_ID};
use rurqe_postgres::rurqe_migrate::introspect::SchemaSelection;
use rurqe_postgres::rurqe_migrate::runner::MigrationBackend;
use rurqe_postgres::rurqe_migrate::{Checkpoint, Checksum, Migration, Migrator};
use rurqe_postgres::PgConnection;

#[cfg(feature = "tokio-postgres")]
use rurqe_postgres::{migrate::PgAsyncMigrationBackend, PgAsyncConnection};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn database_url(label: &str) -> Option<String> {
    match std::env::var("RURQE_TEST_POSTGRES_URL") {
        Ok(url) if !url.trim().is_empty() => Some(url),
        _ => {
            println!("skipping `{label}`: RURQE_TEST_POSTGRES_URL is not set");
            None
        }
    }
}

fn quote(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

struct Fixture {
    admin: postgres::Client,
    names: Vec<String>,
}

impl Fixture {
    fn new(label: &str) -> Option<Self> {
        let url = database_url(label)?;
        let admin = postgres::Client::connect(&url, NoTls).expect("test server accepts connections");
        Some(Self { admin, names: Vec::new() })
    }

    fn name(&mut self, suffix: &str) -> String {
        let name = format!("rurqe_clean_{}_{}_{}_{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed), suffix, "test");
        self.names.push(name.clone());
        name
    }

    fn create(&mut self, name: &str) {
        self.admin.batch_execute(&format!("CREATE SCHEMA {}", quote(name))).expect("create isolated schema");
    }

    fn exists(&mut self, name: &str) -> bool {
        self.admin.query_one("SELECT EXISTS (SELECT 1 FROM pg_namespace WHERE nspname = $1)", &[&name]).expect("read schemas").get(0)
    }

    fn relation(&mut self, schema: &str, table: &str) -> bool {
        self.admin.query_one("SELECT to_regclass($1) IS NOT NULL", &[&format!("{}.{}", quote(schema), quote(table))]).expect("read relation").get(0)
    }

    fn unlocked(&mut self) {
        let held: bool = self.admin.query_one("SELECT pg_try_advisory_lock($1)", &[&MIGRATION_LOCK_ID]).expect("probe migration lock").get(0);
        assert!(held, "migration lock must be released");
        let released: bool = self.admin.query_one("SELECT pg_advisory_unlock($1)", &[&MIGRATION_LOCK_ID]).expect("release probe lock").get(0);
        assert!(released);
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        for name in &self.names {
            self.admin.batch_execute(&format!("DROP SCHEMA IF EXISTS {} CASCADE", quote(name))).expect("remove test schema");
        }
    }
}

#[test]
fn owned_backend_reapplies_full_tail_after_clean() {
    let Some(mut f) = Fixture::new("owned clean cycle") else { return };
    let selected = f.name("selected");
    let untouched = f.name("untouched");
    f.create(&selected);
    f.create(&untouched);
    f.admin.batch_execute(&format!("CREATE TABLE {}.kept (id int)", quote(&untouched))).expect("create unrelated table");
    let url = database_url("owned clean cycle").expect("configured server");
    let mut client = postgres::Client::connect(&url, NoTls).expect("connect");
    client.batch_execute(&format!("SET search_path TO {}", quote(&selected))).expect("select schema");
    let mut backend = PostgresMigrationBackend::new(client).with_schema(&selected);
    let scripts = [Migration::parse("V1__users.sql", "CREATE TABLE users (id int)").expect("valid script"), Migration::parse("V2__orders.sql", "CREATE TABLE orders (id int)").expect("valid script")];
    assert_eq!(Migrator::new().migrate(&mut backend, &scripts).expect("initial migrate").applied_count, 2);
    assert_eq!(backend.clean_manifest().expect("preview"), vec![selected.clone()]);
    assert!(f.relation(&selected, "users"), "preview does not drop tables");
    assert!(f.relation(&selected, "__rurqe_schema_history"), "preview retains history");
    assert_eq!(Migrator::new().clean(&mut backend).expect("clean"), vec![selected.clone()]);
    assert!(f.exists(&selected));
    assert!(!f.relation(&selected, "users"));
    assert!(!f.relation(&selected, "__rurqe_schema_history"));
    assert!(f.relation(&untouched, "kept"));
    assert_eq!(Migrator::new().migrate(&mut backend, &scripts).expect("reapply tail").applied_count, 2);
    assert!(f.relation(&selected, "orders"));
    drop(backend);
    f.unlocked();
}

#[test]
fn client_backend_cleans_external_history_multiple_schemas_and_cascade() {
    let Some(mut f) = Fixture::new("client clean") else { return };
    let primary = f.name("primary");
    let secondary = f.name("secondary");
    let history = f.name("history");
    let unrelated = f.name("unrelated");
    for name in [&primary, &secondary, &history, &unrelated] { f.create(name); }
    let url = database_url("client clean").expect("configured server");
    let mut client = postgres::Client::connect(&url, NoTls).expect("connect");
    client.batch_execute(&format!("SET search_path TO {}, {}", quote(&history), quote(&primary))).expect("set history search path");
    let mut connection = PgConnection::new(client);
    let selection = SchemaSelection::new(&primary, vec![secondary.clone()]);
    let mut backend = PgConnectionMigrationBackend::new(&mut connection).with_schemas(selection);
    let scripts = [Migration::parse("V1__tables.sql", &format!("CREATE TABLE {}.users (id int PRIMARY KEY); CREATE TABLE {}.orders (id int)", quote(&primary), quote(&secondary))).expect("valid script")];
    Migrator::new().migrate(&mut backend, &scripts).expect("migrate");
    f.admin.batch_execute(&format!("CREATE VIEW {}.dependent AS SELECT id FROM {}.users; CREATE TABLE {}.kept (id int)", quote(&unrelated), quote(&primary), quote(&unrelated))).expect("create cascade dependency");
    let manifest = backend.clean_manifest().expect("preview");
    assert_eq!(manifest.len(), 3);
    assert!(manifest.contains(&primary) && manifest.contains(&secondary));
    assert!(manifest.contains(&format!("{}.{}", quote(&history), quote("__rurqe_schema_history"))));
    assert!(f.relation(&history, "__rurqe_schema_history"));
    assert_eq!(Migrator::new().clean(&mut backend).expect("clean"), manifest);
    assert!(!f.relation(&history, "__rurqe_schema_history"));
    assert!(!f.relation(&unrelated, "dependent"), "CASCADE removes external dependents");
    assert!(f.relation(&unrelated, "kept"));
    assert!(f.exists(&primary) && f.exists(&secondary));
    assert_eq!(Migrator::new().migrate(&mut backend, &scripts).expect("reapply").applied_count, 1);
    drop(backend);
    f.unlocked();
}

#[test]
fn missing_and_quoted_schemas_are_reset_safely() {
    let Some(mut f) = Fixture::new("quoted clean") else { return };
    let quoted = format!("{}_\"x", f.name("quoted"));
    f.names.push(quoted.clone());
    let missing = f.name("missing");
    f.create(&quoted);
    f.admin.batch_execute(&format!("CREATE TABLE {}.old (id int)", quote(&quoted))).expect("create table");
    let url = database_url("quoted clean").expect("configured server");
    let client = postgres::Client::connect(&url, NoTls).expect("connect");
    let mut backend = PostgresMigrationBackend::new(client).with_schemas(SchemaSelection::new(&quoted, vec![missing.clone()]));
    assert_eq!(Migrator::new().clean(&mut backend).expect("clean"), vec![quoted.clone(), missing.clone()]);
    assert!(f.exists(&quoted) && f.exists(&missing));
    assert!(!f.relation(&quoted, "old"));
}

#[test]
fn failed_clean_rolls_back_all_schemas_and_releases_lock() {
    let Some(mut f) = Fixture::new("atomic clean") else { return };
    let first = f.name("first");
    let second = f.name("second");
    f.create(&first);
    f.create(&second);
    f.admin.batch_execute(&format!("CREATE TABLE {}.old (id int); CREATE TABLE {}.old (id int)", quote(&first), quote(&second))).expect("create fixture tables");
    // A separate session holds a lock on the second schema; lock_timeout forces
    // failure after the first schema has already been dropped in the transaction.
    let url = database_url("atomic clean").expect("configured server");
    let mut blocker = postgres::Client::connect(&url, NoTls).expect("connect blocker");
    blocker.batch_execute("BEGIN").expect("begin blocker");
    blocker.batch_execute(&format!("SELECT * FROM {}.old", quote(&second))).expect("hold a read lock on the second schema's table");
    let mut client = postgres::Client::connect(&url, NoTls).expect("connect cleaner");
    client.batch_execute("SET lock_timeout = '100ms'").expect("bound lock wait");
    let mut backend = PostgresMigrationBackend::new(client).with_schemas(SchemaSelection::new(&first, vec![second.clone()]));
    assert!(Migrator::new().clean(&mut backend).is_err());
    blocker.batch_execute("ROLLBACK").expect("release blocker");
    assert!(f.relation(&first, "old") && f.relation(&second, "old"), "the first drop was rolled back");
    f.unlocked();
    assert!(Migrator::new().clean(&mut backend).is_ok(), "the failed run released its lock");
}

#[test]
fn a_pruned_checkpoint_still_refuses_the_empty_history_after_clean() {
    let Some(mut f) = Fixture::new("checkpoint clean") else { return };
    let schema = f.name("checkpoint");
    f.create(&schema);
    let url = database_url("checkpoint clean").expect("configured server");
    let mut client = postgres::Client::connect(&url, NoTls).expect("connect");
    client.batch_execute(&format!("SET search_path TO {}", quote(&schema))).expect("select schema");
    let mut backend = PostgresMigrationBackend::new(client).with_schema(schema);
    let tail = [Migration::parse("V2__orders.sql", "CREATE TABLE orders (id int)").expect("valid tail")];
    Migrator::new().clean(&mut backend).expect("clean");
    let guarded = Migrator::with_checkpoint(Checkpoint::new("1".parse().expect("version"), Checksum::of(b"boundary")));
    assert!(matches!(guarded.migrate(&mut backend, &tail), Err(rurqe_postgres::rurqe_migrate::Error::OutdatedCheckpoint { .. })));
    assert!(!f.relation(backend.schema(), "orders"));
}

#[cfg(feature = "tokio-postgres")]
#[test]
fn async_backend_resets_and_reapplies_the_tail() {
    let Some(mut f) = Fixture::new("async clean") else { return };
    let schema = f.name("async");
    f.create(&schema);
    let url = database_url("async clean").expect("configured server");
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("runtime");
    let connection = runtime.block_on(async {
        let (client, task) = tokio_postgres::connect(&url, tokio_postgres::NoTls).await.expect("connect");
        tokio::spawn(async move { let _ = task.await; });
        client.batch_execute(&format!("SET search_path TO {}", quote(&schema))).await.expect("select schema");
        PgAsyncConnection::new(client)
    });
    let mut backend = PgAsyncMigrationBackend::new(connection).expect("start worker").with_schema(&schema);
    let scripts = [Migration::parse("V1__users.sql", "CREATE TABLE users (id int)").expect("valid script")];
    assert_eq!(Migrator::new().migrate(&mut backend, &scripts).expect("migrate").applied_count, 1);
    assert_eq!(Migrator::new().clean(&mut backend).expect("clean"), vec![schema.clone()]);
    assert!(!f.relation(&schema, "__rurqe_schema_history"));
    assert_eq!(Migrator::new().migrate(&mut backend, &scripts).expect("reapply").applied_count, 1);
    drop(backend);
    f.unlocked();
}
