//! End-to-end verification of the Stage + Migration Tail lifecycle.
//!
//! Every test drives the public surface the way a real project would: a cold
//! database is initialised straight from the declarative stage, the active tail
//! advances it, the stage is sealed with a checkpoint, the folded history is
//! pruned, and the golden equation between both descriptions of the schema is
//! verified. The outdated-target guard is exercised through the migration runner
//! itself, because that is where a pruned tail must refuse to run.

use std::fs;
use std::path::{Path, PathBuf};

use rurqe_cli::run_cli;
use rurqe_cli::stage::materialise_stage;
use rurqe_cli::SqliteMigrationBackend;
use rurqe_migrate::catalog::SchemaCatalog;
use rurqe_migrate::{
    Checkpoint, Checksum, DeclarativeStage, Error, Migration, Migrator, StageScript, STAGE_SCRIPT,
};

/// The declarative definition of the `users` table shared by most tests.
const USERS: &str = "CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT NOT NULL);";

/// The declarative definition of the `orders` table shared by most tests.
const ORDERS: &str = "CREATE TABLE orders (id INTEGER PRIMARY KEY, user_id INTEGER NOT NULL);";

/// User tables of the scratch database, excluding the history table.
const CATALOG_QUERY: &str = "SELECT name FROM sqlite_master \
     WHERE type = 'table' \
       AND name NOT LIKE 'sqlite_%' \
       AND name <> '__rurqe_schema_history' \
     ORDER BY name";

/// Recorded history rows as `(script, version, kind)`.
const HISTORY_QUERY: &str =
    "SELECT script, version, \"type\" FROM __rurqe_schema_history ORDER BY installed_rank";

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
        "rurqe-stage-it-{label}-{}-{nanos}-{sequence}",
        std::process::id()
    ));
    if dir.exists() {
        let _ = fs::remove_dir_all(&dir);
    }
    fs::create_dir_all(&dir).expect("create the temp workspace");
    dir
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Builds the argv a `cargo rurqe ...` invocation maps onto.
fn argv(args: &[&str]) -> Vec<String> {
    std::iter::once("cargo-rurqe")
        .chain(std::iter::once("rurqe"))
        .chain(args.iter().copied())
        .map(str::to_owned)
        .collect()
}

/// Writes declarative stage scripts or migration scripts into `dir`.
fn write_scripts(dir: &Path, scripts: &[(&str, &str)]) {
    fs::create_dir_all(dir).expect("create the script directory");
    for (name, sql) in scripts {
        fs::write(dir.join(name), sql).expect("write the script");
    }
}

/// The file names present in `dir`, sorted alphabetically.
fn script_names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .expect("read the script directory")
        .map(|entry| {
            entry
                .expect("a directory entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    names
}

/// Writes a canonical `checkpoint.toml` marker with a placeholder hash.
///
/// Used by the pruning tests, where no database sits behind the marker and the
/// anchored hash never takes part in the verification.
fn write_checkpoint(path: &Path, version: &str) {
    write_checkpoint_with_hash(path, version, Checksum::of(b"stage"));
}

/// Writes a canonical `checkpoint.toml` marker anchoring `schema_hash`.
fn write_checkpoint_with_hash(path: &Path, version: &str, schema_hash: Checksum) {
    let checkpoint = Checkpoint::new(version.parse().expect("a valid version"), schema_hash);
    fs::write(path, checkpoint.render()).expect("write the checkpoint");
}

/// Writes the `rurqe.toml` naming the migration tail and the boundary marker.
///
/// The `migrate` family takes no path option of its own, so a regression that
/// points the runner at a directory of its own names it through the project
/// configuration and hands the file to the invocation with `--config`.
fn write_project_config(workspace: &Path, migrations: &Path, checkpoint: &Path) -> String {
    let config = workspace.join("rurqe.toml");
    fs::write(
        &config,
        format!(
            "[migrations]\ndir = \"{}\"\n\n[stage]\ncheckpoint = \"{}\"\n",
            declared_path(workspace, migrations),
            declared_path(workspace, checkpoint),
        ),
    )
    .expect("write the project configuration");
    path_string(&config)
}

/// The path of `path` as the project configuration declares it.
///
/// A declared path resolves against the directory holding `rurqe.toml`, so it is
/// written relative to the workspace: an absolute value would be refused.
fn declared_path(workspace: &Path, path: &Path) -> String {
    path.strip_prefix(workspace)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

/// The canonical catalog digest of the SQLite database at `db`.
///
/// The digest is read through the very introspection the migration backend
/// reports at the checkpoint boundary, so a marker written from it describes the
/// live schema instead of a placeholder.
fn catalog_checksum(db: &Path) -> Checksum {
    let connection = rusqlite::Connection::open(db).expect("open the sqlite database");
    let backend = SqliteMigrationBackend::new(connection);
    backend
        .introspect_catalog()
        .expect("introspect the live catalog")
        .checksum()
}

/// The user tables of the SQLite database at `db`, sorted by name.
fn tables(db: &Path) -> Vec<String> {
    let connection = rusqlite::Connection::open(db).expect("open the sqlite database");
    let mut statement = connection
        .prepare(CATALOG_QUERY)
        .expect("prepare the catalog query");

    statement
        .query_map([], |row| row.get::<_, String>(0))
        .expect("query the catalog")
        .collect::<Result<Vec<_>, _>>()
        .expect("read the catalog rows")
}

/// The recorded history rows of the SQLite database at `db`.
fn history(db: &Path) -> Vec<(String, Option<String>, String)> {
    let connection = rusqlite::Connection::open(db).expect("open the sqlite database");
    let mut statement = connection
        .prepare(HISTORY_QUERY)
        .expect("prepare the history query");

    statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .expect("query the history")
        .collect::<Result<Vec<_>, _>>()
        .expect("read the history rows")
}

/// The checksum recorded for the synthetic stage marker of the database at `db`.
fn stage_marker_checksum(db: &Path) -> String {
    let connection = rusqlite::Connection::open(db).expect("open the sqlite database");
    connection
        .query_row(
            "SELECT checksum FROM __rurqe_schema_history WHERE script = ?1",
            [STAGE_SCRIPT],
            |row| row.get::<_, String>(0),
        )
        .expect("read the stage marker checksum")
}

#[test]
fn cold_start_initialises_the_database_from_the_declarative_stage() {
    let workspace = temp_workspace("init");
    let db = workspace.join("app.db");
    let stage_dir = workspace.join("schema");
    write_scripts(&stage_dir, &[("orders.sql", ORDERS), ("users.sql", USERS)]);

    let stage = path_string(&stage_dir);
    let database = path_string(&db);
    let init = argv(&[
        "stage",
        "init",
        "--version",
        "3.2",
        "--stage-dir",
        &stage,
        "--description",
        "initial stage",
        "--database-url",
        &database,
    ]);

    run_cli(init.clone()).expect("the cold start applies the declarative stage");

    assert_eq!(tables(&db), vec!["orders".to_owned(), "users".to_owned()]);
    assert_eq!(
        history(&db),
        vec![(
            STAGE_SCRIPT.to_owned(),
            Some("3.2".to_owned()),
            "BASELINE".to_owned(),
        )]
    );

    run_cli(init).expect("a recorded stage leaves the database untouched");
    assert_eq!(history(&db).len(), 1);
    assert_eq!(tables(&db).len(), 2);
}

#[test]
fn the_active_tail_advances_the_cold_started_database() {
    let workspace = temp_workspace("tail");
    let db = workspace.join("app.db");
    let stage_dir = workspace.join("schema");
    let migrations = workspace.join("migrations");
    write_scripts(&stage_dir, &[("users.sql", USERS)]);
    write_scripts(
        &migrations,
        &[("V1__init.sql", USERS), ("V4__orders.sql", ORDERS)],
    );

    let stage = path_string(&stage_dir);
    let database = path_string(&db);
    let config = write_project_config(&workspace, &migrations, &workspace.join("checkpoint.toml"));

    run_cli(argv(&[
        "stage",
        "init",
        "--version",
        "3.2",
        "--stage-dir",
        &stage,
        "--database-url",
        &database,
    ]))
    .expect("the cold start applies the declarative stage");

    run_cli(argv(&[
        "migrate",
        "up",
        "--config",
        &config,
        "--database-url",
        &database,
    ]))
    .expect("the tail above the stage marker is applied");

    assert_eq!(tables(&db), vec!["orders".to_owned(), "users".to_owned()]);

    let scripts: Vec<String> = history(&db)
        .into_iter()
        .map(|(script, _, _)| script)
        .collect();
    assert_eq!(
        scripts,
        vec![STAGE_SCRIPT.to_owned(), "V4__orders.sql".to_owned()]
    );

    run_cli(argv(&[
        "migrate",
        "up",
        "--config",
        &config,
        "--database-url",
        &database,
    ]))
    .expect("a repeated run applies nothing");
    assert_eq!(history(&db).len(), 2);
}

#[test]
fn committing_the_stage_writes_the_lock_and_the_checkpoint() {
    let workspace = temp_workspace("commit");
    let db = workspace.join("app.db");
    {
        let connection = rusqlite::Connection::open(&db).expect("open the sqlite database");
        connection.execute_batch(USERS).expect("create the schema");
    }

    let stage_dir = workspace.join("schema");
    write_scripts(&stage_dir, &[("users.sql", USERS)]);
    let stage = path_string(&stage_dir);
    let lock = workspace.join("schema.lock");
    let checkpoint_path = workspace.join("checkpoint.toml");
    let database = path_string(&db);
    let lock_path = path_string(&lock);
    let checkpoint = path_string(&checkpoint_path);

    run_cli(argv(&[
        "stage",
        "commit",
        "--version",
        "4",
        "--stage-dir",
        &stage,
        "--database-url",
        &database,
        "--lock-path",
        &lock_path,
        "--checkpoint-path",
        &checkpoint,
    ]))
    .expect("the live database is sealed as the new stage");

    let snapshot = fs::read_to_string(&lock).expect("read the snapshot");
    let catalog = SchemaCatalog::from_lock(&snapshot).expect("the snapshot is canonical");
    let marker = Checkpoint::load(&checkpoint_path)
        .expect("the checkpoint is readable")
        .expect("the checkpoint exists");

    assert_eq!(marker.pruned_up_to_version().to_string(), "4");
    assert_eq!(marker.schema_hash(), catalog.checksum());
    assert_eq!(catalog.tables.len(), 1);
    assert_eq!(catalog.tables[0].name, "users");

    // The boundary snapshot lands next to the marker and carries the DDL of
    // the stage verbatim, so the tail can be verified without a manual archive.
    assert_eq!(
        fs::read_to_string(workspace.join("checkpoint-stage").join("users.sql"))
            .expect("read the boundary snapshot"),
        USERS
    );

    let error = run_cli(argv(&[
        "stage",
        "commit",
        "--version",
        "3",
        "--stage-dir",
        &stage,
        "--database-url",
        &database,
        "--lock-path",
        &lock_path,
        "--checkpoint-path",
        &checkpoint,
    ]))
    .expect_err("the committed boundary cannot move backwards");

    assert!(error.to_string().contains("re-open"), "got {error}");
}

#[test]
fn pruning_removes_only_the_scripts_the_checkpoint_folded_in() {
    let workspace = temp_workspace("prune");
    let migrations = workspace.join("migrations");
    write_scripts(
        &migrations,
        &[
            ("V1__init.sql", USERS),
            ("V2__orders.sql", ORDERS),
            (
                "V9__tail.sql",
                "CREATE TABLE audit (id INTEGER PRIMARY KEY);",
            ),
            (
                "R__view.sql",
                "CREATE VIEW active_users AS SELECT id FROM users;",
            ),
        ],
    );
    let checkpoint_path = workspace.join("checkpoint.toml");
    write_checkpoint(&checkpoint_path, "9");

    let dir = path_string(&migrations);
    let checkpoint = path_string(&checkpoint_path);

    run_cli(argv(&[
        "prune",
        "--before",
        "3",
        "--migrations-dir",
        &dir,
        "--checkpoint-path",
        &checkpoint,
        "--dry-run",
    ]))
    .expect("the dry run reports without touching the directory");
    assert_eq!(script_names(&migrations).len(), 4);

    run_cli(argv(&[
        "prune",
        "--before",
        "3",
        "--migrations-dir",
        &dir,
        "--checkpoint-path",
        &checkpoint,
    ]))
    .expect("the prune drops the folded scripts");

    assert_eq!(
        script_names(&migrations),
        vec!["R__view.sql".to_owned(), "V9__tail.sql".to_owned()]
    );
}

#[test]
fn pruning_refuses_to_drop_scripts_the_tail_still_needs() {
    let workspace = temp_workspace("prune-refused");
    let migrations = workspace.join("migrations");
    write_scripts(&migrations, &[("V2__orders.sql", ORDERS)]);
    let checkpoint_path = workspace.join("checkpoint.toml");
    write_checkpoint(&checkpoint_path, "1");

    let dir = path_string(&migrations);
    let checkpoint = path_string(&checkpoint_path);

    let error = run_cli(argv(&[
        "prune",
        "--before",
        "3",
        "--migrations-dir",
        &dir,
        "--checkpoint-path",
        &checkpoint,
    ]))
    .expect_err("the checkpoint sits below the requested boundary");

    assert!(error.to_string().contains("still needs"), "got {error}");
    assert_eq!(script_names(&migrations), vec!["V2__orders.sql".to_owned()]);

    let missing = path_string(&workspace.join("missing-checkpoint.toml"));
    let error = run_cli(argv(&[
        "prune",
        "--before",
        "3",
        "--migrations-dir",
        &dir,
        "--checkpoint-path",
        &missing,
    ]))
    .expect_err("pruning without a committed checkpoint is rejected");

    assert!(
        error.to_string().contains("commit the stage"),
        "got {error}"
    );
}

#[test]
fn an_outdated_database_is_blocked_by_the_checkpoint() {
    let workspace = temp_workspace("outdated");
    let db = workspace.join("legacy.db");
    let init = Migration::parse("V1__init.sql", USERS).expect("a valid script name");

    {
        let connection = rusqlite::Connection::open(&db).expect("open the sqlite database");
        let mut backend = SqliteMigrationBackend::new(connection);
        Migrator::new()
            .migrate(&mut backend, &[init])
            .expect("the initial migration runs");
    }

    let tail = Migration::parse("V3__orders.sql", ORDERS).expect("a valid script name");
    let stale = Checkpoint::new(
        "2".parse().expect("a valid version"),
        Checksum::of(b"stage"),
    );

    let error = {
        let connection = rusqlite::Connection::open(&db).expect("reopen the sqlite database");
        let mut backend = SqliteMigrationBackend::new(connection);
        Migrator::with_checkpoint(stale)
            .migrate(&mut backend, std::slice::from_ref(&tail))
            .expect_err("the pruned tail cannot advance an outdated database")
    };

    assert!(
        matches!(error, Error::OutdatedCheckpoint { .. }),
        "got {error:?}"
    );
    assert!(error.to_string().contains("schema/"), "got {error}");
    assert_eq!(tables(&db), vec!["users".to_owned()]);

    // The boundary marker anchors the live catalog, so the tail is gated by the
    // real schema hash instead of a placeholder.
    let boundary = Checkpoint::new("1".parse().expect("a valid version"), catalog_checksum(&db));
    let report = {
        let connection = rusqlite::Connection::open(&db).expect("reopen the sqlite database");
        let mut backend = SqliteMigrationBackend::new(connection);
        Migrator::with_checkpoint(boundary)
            .migrate(&mut backend, &[tail])
            .expect("a database at the boundary is advanced by the tail")
    };

    assert_eq!(report.applied_count, 1);
    assert_eq!(report.applied_scripts, vec!["V3__orders.sql".to_owned()]);
    assert_eq!(tables(&db), vec!["orders".to_owned(), "users".to_owned()]);
}

#[test]
fn migrate_up_honours_the_committed_checkpoint() {
    let workspace = temp_workspace("cli-checkpoint");
    let db = workspace.join("app.db");
    let migrations = workspace.join("migrations");
    write_scripts(&migrations, &[("V1__init.sql", USERS)]);

    let config = write_project_config(&workspace, &migrations, &workspace.join("checkpoint.toml"));
    let database = path_string(&db);

    run_cli(argv(&[
        "migrate",
        "up",
        "--config",
        &config,
        "--database-url",
        &database,
    ]))
    .expect("the initial migration is applied without a checkpoint");

    write_scripts(&migrations, &[("V3__orders.sql", ORDERS)]);
    let checkpoint_path = workspace.join("checkpoint.toml");
    write_checkpoint(&checkpoint_path, "2");

    let error = run_cli(argv(&[
        "migrate",
        "up",
        "--config",
        &config,
        "--database-url",
        &database,
    ]))
    .expect_err("the database sits below the pruned boundary");

    assert!(error.to_string().contains("schema/"), "got {error}");
    assert_eq!(tables(&db), vec!["users".to_owned()]);

    let stage_hash = catalog_checksum(&db);
    write_checkpoint_with_hash(&checkpoint_path, "1", stage_hash);
    run_cli(argv(&[
        "migrate",
        "up",
        "--config",
        &config,
        "--database-url",
        &database,
    ]))
    .expect("a database at the boundary is advanced by the tail");

    assert_eq!(tables(&db), vec!["orders".to_owned(), "users".to_owned()]);
    assert_eq!(
        history(&db)
            .into_iter()
            .map(|(script, _, _)| script)
            .collect::<Vec<_>>(),
        vec!["V1__init.sql".to_owned(), "V3__orders.sql".to_owned()]
    );
}

#[test]
fn the_golden_equation_holds_for_a_consistent_stage_and_tail() {
    let workspace = temp_workspace("golden");
    let stage_dir = workspace.join("schema");
    let migrations = workspace.join("migrations");
    write_scripts(&stage_dir, &[("users.sql", USERS)]);
    write_scripts(&migrations, &[("V1__users.sql", USERS)]);

    let stage = path_string(&stage_dir);
    let dir = path_string(&migrations);
    // `stage verify` takes the exclusive section guarding the artifacts beside
    // the marker, so each test names a marker inside its own workspace: a shared
    // default would serialise the parallel tests on one lock.
    let checkpoint = path_string(&workspace.join("checkpoint.toml"));

    run_cli(argv(&[
        "stage",
        "verify",
        "--stage-dir",
        &stage,
        "--migrations-dir",
        &dir,
        "--checkpoint-path",
        &checkpoint,
        "--scratch-url",
        ":memory:",
    ]))
    .expect("both descriptions of the schema converge");
}

#[test]
fn the_golden_equation_reports_a_tail_that_lost_a_column() {
    let workspace = temp_workspace("golden-diverged");
    let stage_dir = workspace.join("schema");
    let migrations = workspace.join("migrations");
    write_scripts(&stage_dir, &[("users.sql", USERS)]);
    write_scripts(
        &migrations,
        &[(
            "V1__users.sql",
            "CREATE TABLE users (id INTEGER PRIMARY KEY);",
        )],
    );

    let stage = path_string(&stage_dir);
    let dir = path_string(&migrations);
    // The marker sits inside the workspace of the test, so the section the
    // verification takes never overlaps that of a test running in parallel.
    let checkpoint = path_string(&workspace.join("checkpoint.toml"));

    let error = run_cli(argv(&[
        "stage",
        "verify",
        "--stage-dir",
        &stage,
        "--migrations-dir",
        &dir,
        "--checkpoint-path",
        &checkpoint,
        "--scratch-url",
        ":memory:",
    ]))
    .expect_err("the tail never creates the email column");

    let message = error.to_string();
    assert!(message.contains("golden equation"), "got {message}");
    assert!(message.contains("users"), "got {message}");
    assert!(message.contains("email"), "got {message}");
}

#[test]
fn the_base_stage_anchors_an_incremental_tail() {
    let workspace = temp_workspace("golden-base");
    let base_dir = workspace.join("base");
    write_scripts(
        &base_dir,
        &[("users.sql", "CREATE TABLE users (id INTEGER PRIMARY KEY);")],
    );
    let migrations = workspace.join("migrations");
    write_scripts(
        &migrations,
        &[(
            "V2__email.sql",
            "ALTER TABLE users ADD COLUMN email TEXT NOT NULL DEFAULT '';",
        )],
    );
    let stage_dir = workspace.join("schema");
    write_scripts(
        &stage_dir,
        &[(
            "users.sql",
            "CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT NOT NULL DEFAULT '');",
        )],
    );

    let stage = path_string(&stage_dir);
    let base = path_string(&base_dir);
    let dir = path_string(&migrations);
    // Both verifications of the test share one marker inside its own workspace,
    // so they never contend with a test running in parallel.
    let checkpoint = path_string(&workspace.join("checkpoint.toml"));

    run_cli(argv(&[
        "stage",
        "verify",
        "--stage-dir",
        &stage,
        "--base-stage-dir",
        &base,
        "--migrations-dir",
        &dir,
        "--checkpoint-path",
        &checkpoint,
        "--scratch-url",
        ":memory:",
    ]))
    .expect("the base stage plus the tail reproduces the stage");

    let error = run_cli(argv(&[
        "stage",
        "verify",
        "--stage-dir",
        &stage,
        "--migrations-dir",
        &dir,
        "--checkpoint-path",
        &checkpoint,
        "--scratch-url",
        ":memory:",
    ]))
    .expect_err("the tail alters a table nothing created");

    assert!(error.to_string().contains("V2__email.sql"), "got {error}");
}

#[test]
fn cold_start_records_catalog_checksum_matching_stage_commit() {
    let workspace = temp_workspace("catalog-checksum");
    let db = workspace.join("app.db");
    let stage_dir = workspace.join("schema");
    let migrations = workspace.join("migrations");
    write_scripts(&stage_dir, &[("users.sql", USERS)]);
    write_scripts(&migrations, &[("V4__orders.sql", ORDERS)]);

    let stage = path_string(&stage_dir);
    let database = path_string(&db);
    let config = write_project_config(&workspace, &migrations, &workspace.join("checkpoint.toml"));

    run_cli(argv(&[
        "stage",
        "init",
        "--version",
        "3.2",
        "--stage-dir",
        &stage,
        "--database-url",
        &database,
    ]))
    .expect("the cold start applies the declarative stage");

    let recorded = stage_marker_checksum(&db);

    let lock = workspace.join("schema.lock");
    let checkpoint_path = workspace.join("checkpoint.toml");
    let lock_path = path_string(&lock);
    let checkpoint = path_string(&checkpoint_path);

    run_cli(argv(&[
        "stage",
        "commit",
        "--version",
        "3.2",
        "--stage-dir",
        &stage,
        "--database-url",
        &database,
        "--lock-path",
        &lock_path,
        "--checkpoint-path",
        &checkpoint,
    ]))
    .expect("the live database is sealed as the new stage");

    let marker = Checkpoint::load(&checkpoint_path)
        .expect("the checkpoint is readable")
        .expect("the checkpoint exists");

    // The cold start anchored the canonical catalog digest, so sealing the very
    // same database records the identical hash instead of a raw DDL digest.
    assert_eq!(marker.schema_hash().to_hex(), recorded);

    run_cli(argv(&[
        "migrate",
        "up",
        "--config",
        &config,
        "--database-url",
        &database,
    ]))
    .expect("the active tail advances the cold-started database");

    assert_eq!(tables(&db), vec!["orders".to_owned(), "users".to_owned()]);
    assert_eq!(
        history(&db)
            .into_iter()
            .map(|(script, _, _)| script)
            .collect::<Vec<_>>(),
        vec![STAGE_SCRIPT.to_owned(), "V4__orders.sql".to_owned()]
    );
}

#[test]
fn migrate_up_rejects_cold_started_database_with_mismatched_checkpoint_schema_hash() {
    let workspace = temp_workspace("checkpoint-hash-mismatch");
    let db = workspace.join("app.db");
    let stage_dir = workspace.join("schema");
    write_scripts(&stage_dir, &[("users.sql", USERS)]);

    let stage = path_string(&stage_dir);
    let database = path_string(&db);

    run_cli(argv(&[
        "stage",
        "init",
        "--version",
        "3.2",
        "--stage-dir",
        &stage,
        "--database-url",
        &database,
    ]))
    .expect("the cold start applies the declarative stage");

    let lock = workspace.join("schema.lock");
    let checkpoint_path = workspace.join("checkpoint.toml");
    let lock_path = path_string(&lock);
    let checkpoint = path_string(&checkpoint_path);

    run_cli(argv(&[
        "stage",
        "commit",
        "--version",
        "3.2",
        "--stage-dir",
        &stage,
        "--database-url",
        &database,
        "--lock-path",
        &lock_path,
        "--checkpoint-path",
        &checkpoint,
    ]))
    .expect("the live database is sealed as the new stage");

    // Tamper with the recorded stage hash without touching the database.
    write_checkpoint(&checkpoint_path, "3.2");

    let migrations = workspace.join("migrations");
    write_scripts(&migrations, &[("V4__orders.sql", ORDERS)]);
    let config = write_project_config(&workspace, &migrations, &workspace.join("checkpoint.toml"));

    let error = run_cli(argv(&[
        "migrate",
        "up",
        "--config",
        &config,
        "--database-url",
        &database,
    ]))
    .expect_err("the diverged stage hash blocks the tail");

    assert!(
        error
            .to_string()
            .contains("has diverged from the checkpoint"),
        "got {error}"
    );
    assert_eq!(tables(&db), vec!["users".to_owned()]);
}

#[test]
fn stages_verify_filters_the_unpruned_tail_at_the_checkpoint() {
    let workspace = temp_workspace("prune-verify");
    let base_dir = workspace.join("base");
    let stage_dir = workspace.join("schema");
    let migrations = workspace.join("migrations");

    // The declarative stage the checkpoint was cut at: the `users` table
    // without the `email` column the tail later adds.
    let base_users = "CREATE TABLE users (id INTEGER PRIMARY KEY);";
    write_scripts(&base_dir, &[("users.sql", base_users)]);

    // Unpruned history: the folded `V1` still sits at the boundary while `V2`
    // is the active delta.
    write_scripts(
        &migrations,
        &[
            ("V1__users.sql", base_users),
            (
                "V2__email.sql",
                "ALTER TABLE users ADD COLUMN email TEXT NOT NULL DEFAULT '';",
            ),
        ],
    );

    // The current stage the base plus the active tail must reproduce.
    write_scripts(
        &stage_dir,
        &[(
            "users.sql",
            "CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT NOT NULL DEFAULT '');",
        )],
    );

    // The checkpoint hash must anchor the base stage catalog.
    let base_stage = DeclarativeStage::new(vec![StageScript::new("users.sql", base_users)]);
    let base_catalog = materialise_stage(
        &base_stage,
        ":memory:",
        rurqe_cli::introspect::DEFAULT_SCHEMA,
    )
    .expect("the base stage materialises");
    let checkpoint_path = workspace.join("checkpoint.toml");
    write_checkpoint_with_hash(&checkpoint_path, "1", base_catalog.checksum());

    let stage = path_string(&stage_dir);
    let base = path_string(&base_dir);
    let dir = path_string(&migrations);
    let checkpoint = path_string(&checkpoint_path);

    run_cli(argv(&[
        "stage",
        "verify",
        "--stage-dir",
        &stage,
        "--base-stage-dir",
        &base,
        "--migrations-dir",
        &dir,
        "--checkpoint-path",
        &checkpoint,
        "--scratch-url",
        ":memory:",
    ]))
    .expect("the pruned tail above the checkpoint converges with the stage");
}

#[test]
fn the_committed_boundary_snapshot_anchors_the_tail_without_manual_directories() {
    let workspace = temp_workspace("commit-verify");
    let db = workspace.join("app.db");
    let stage_dir = workspace.join("schema");
    let migrations = workspace.join("migrations");

    // The boundary: the declarative stage the checkpoint is cut at.
    let boundary = "CREATE TABLE users (id INTEGER PRIMARY KEY);";
    write_scripts(&stage_dir, &[("users.sql", boundary)]);

    let stage = path_string(&stage_dir);
    let database = path_string(&db);

    run_cli(argv(&[
        "stage",
        "init",
        "--version",
        "1",
        "--stage-dir",
        &stage,
        "--database-url",
        &database,
    ]))
    .expect("the cold start applies the boundary stage");

    let lock = workspace.join("schema.lock");
    let checkpoint_path = workspace.join("checkpoint.toml");
    let lock_path = path_string(&lock);
    let checkpoint = path_string(&checkpoint_path);

    run_cli(argv(&[
        "stage",
        "commit",
        "--version",
        "1",
        "--stage-dir",
        &stage,
        "--database-url",
        &database,
        "--lock-path",
        &lock_path,
        "--checkpoint-path",
        &checkpoint,
    ]))
    .expect("the boundary is sealed together with its snapshot");

    // Both descriptions advance past the committed boundary. The stage
    // repeats the default the tail migration adds, so the boundary snapshot
    // plus the filtered tail reproduces it exactly.
    let advanced = "CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT NOT NULL DEFAULT '');";
    write_scripts(&stage_dir, &[("users.sql", advanced)]);
    write_scripts(
        &migrations,
        &[(
            "V2__email.sql",
            "ALTER TABLE users ADD COLUMN email TEXT NOT NULL DEFAULT '';",
        )],
    );
    let dir = path_string(&migrations);

    // No `--base-stage-dir` is named: the snapshot the commit recorded is the
    // base the filtered tail replays over.
    run_cli(argv(&[
        "stage",
        "verify",
        "--stage-dir",
        &stage,
        "--migrations-dir",
        &dir,
        "--checkpoint-path",
        &checkpoint,
        "--scratch-url",
        ":memory:",
    ]))
    .expect("the committed snapshot anchors the tail without a manual directory");

    // Without the snapshot the checkpoint cannot be reproduced from nothing,
    // and the verification says so instead of reporting a diverged project.
    fs::remove_dir_all(workspace.join("checkpoint-stage")).expect("drop the snapshot");

    let error = run_cli(argv(&[
        "stage",
        "verify",
        "--stage-dir",
        &stage,
        "--migrations-dir",
        &dir,
        "--checkpoint-path",
        &checkpoint,
        "--scratch-url",
        ":memory:",
    ]))
    .expect_err("the checkpoint has no base stage to replay");

    assert!(error.to_string().contains("base stage"), "got {error}");
}

#[test]
fn the_golden_equation_rejects_a_base_stage_diverging_from_the_checkpoint() {
    let workspace = temp_workspace("verify-base-mismatch");
    let base_dir = workspace.join("base");
    let stage_dir = workspace.join("schema");
    let migrations = workspace.join("migrations");

    write_scripts(
        &base_dir,
        &[("users.sql", "CREATE TABLE users (id INTEGER PRIMARY KEY);")],
    );
    write_scripts(
        &migrations,
        &[(
            "V2__email.sql",
            "ALTER TABLE users ADD COLUMN email TEXT NOT NULL DEFAULT '';",
        )],
    );
    write_scripts(
        &stage_dir,
        &[(
            "users.sql",
            "CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT NOT NULL DEFAULT '');",
        )],
    );

    // The marker anchors a placeholder hash the base stage cannot reproduce.
    let checkpoint_path = workspace.join("checkpoint.toml");
    write_checkpoint(&checkpoint_path, "1");

    let stage = path_string(&stage_dir);
    let base = path_string(&base_dir);
    let dir = path_string(&migrations);
    let checkpoint = path_string(&checkpoint_path);

    let error = run_cli(argv(&[
        "stage",
        "verify",
        "--stage-dir",
        &stage,
        "--base-stage-dir",
        &base,
        "--migrations-dir",
        &dir,
        "--checkpoint-path",
        &checkpoint,
        "--scratch-url",
        ":memory:",
    ]))
    .expect_err("the base stage catalog diverges from the checkpoint hash");

    assert!(
        error
            .to_string()
            .contains("base stage catalog hash mismatch"),
        "got {error}"
    );
}

#[test]
fn a_sequential_database_matching_the_checkpoint_is_advanced_by_the_tail() {
    let workspace = temp_workspace("sequential-match");
    let db = workspace.join("app.db");
    let migrations = workspace.join("migrations");
    write_scripts(&migrations, &[("V1__users.sql", USERS)]);

    let config = write_project_config(&workspace, &migrations, &workspace.join("checkpoint.toml"));
    let database = path_string(&db);

    // A classic sequential lifecycle initialises the database; no stage marker
    // is ever recorded, so the history stops at the boundary version.
    run_cli(argv(&[
        "migrate",
        "up",
        "--config",
        &config,
        "--database-url",
        &database,
    ]))
    .expect("the initial migration is applied without a cold start");

    // Anchor the checkpoint to the live catalog of the sequential database.
    let checkpoint_path = workspace.join("checkpoint.toml");
    write_checkpoint_with_hash(&checkpoint_path, "1", catalog_checksum(&db));

    write_scripts(&migrations, &[("V2__orders.sql", ORDERS)]);
    run_cli(argv(&[
        "migrate",
        "up",
        "--config",
        &config,
        "--database-url",
        &database,
    ]))
    .expect("the matching catalog is verified and the tail applies");

    assert_eq!(tables(&db), vec!["orders".to_owned(), "users".to_owned()]);
    assert_eq!(
        history(&db)
            .into_iter()
            .map(|(script, _, _)| script)
            .collect::<Vec<_>>(),
        vec!["V1__users.sql".to_owned(), "V2__orders.sql".to_owned()]
    );
}

#[test]
fn a_drifted_sequential_database_is_rejected_by_the_checkpoint() {
    let workspace = temp_workspace("sequential-drift");
    let db = workspace.join("app.db");
    let migrations = workspace.join("migrations");
    write_scripts(&migrations, &[("V1__users.sql", USERS)]);

    let config = write_project_config(&workspace, &migrations, &workspace.join("checkpoint.toml"));
    let database = path_string(&db);

    run_cli(argv(&[
        "migrate",
        "up",
        "--config",
        &config,
        "--database-url",
        &database,
    ]))
    .expect("the initial migration is applied without a cold start");

    let checkpoint_path = workspace.join("checkpoint.toml");
    write_checkpoint_with_hash(&checkpoint_path, "1", catalog_checksum(&db));

    // Drift the live schema without touching the recorded history, so only the
    // catalog digest catches the divergence.
    {
        let connection = rusqlite::Connection::open(&db).expect("open the sqlite database");
        connection
            .execute_batch("ALTER TABLE users ADD COLUMN drifted TEXT;")
            .expect("the live schema drifts from the checkpoint");
    }

    write_scripts(&migrations, &[("V2__orders.sql", ORDERS)]);
    let error = run_cli(argv(&[
        "migrate",
        "up",
        "--config",
        &config,
        "--database-url",
        &database,
    ]))
    .expect_err("the drifted catalog diverges from the checkpoint hash");

    assert!(
        error
            .to_string()
            .contains("has diverged from the checkpoint"),
        "got {error}"
    );
    assert_eq!(tables(&db), vec!["users".to_owned()]);
}
