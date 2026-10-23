use std::path::{Path, PathBuf};

use rurqe_migrate::checkpoint::Checkpoint;
use rurqe_migrate::runner::MigrationBackend;
use rurqe_migrate::version::MigrationVersion;
use rurqe_migrate::{Checksum, DeclarativeStage, InMemoryBackend, SchemaCatalog, STAGE_SCRIPT};

use crate::driver::SqliteMigrationBackend;
use crate::stage::materialise_stage;

use super::{commit_stage, initialise_stage, prune_migrations, write_file};

fn workspace(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("rurqe-commands-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).expect("create the workspace");
    path
}

fn version(text: &str) -> MigrationVersion {
    text.parse().expect("a valid version")
}

fn scripts(dir: &Path, names: &[&str]) {
    std::fs::create_dir_all(dir).expect("create the migration directory");
    for name in names {
        std::fs::write(dir.join(name), "SELECT 1;").expect("write the script");
    }
}

fn checkpoint_file(path: &Path, version: &str) {
    std::fs::write(
        path,
        Checkpoint::new(version.parse().expect("a valid version"), Checksum::of(b"stage")).render(),
    )
    .expect("write the checkpoint");
}

#[test]
fn the_stage_is_applied_from_the_declarative_directory() {
    let dir = workspace("stage-init");
    let stage_dir = dir.join("schema");
    std::fs::create_dir_all(&stage_dir).expect("create the stage directory");
    std::fs::write(
        stage_dir.join("users.sql"),
        "CREATE TABLE users (id INTEGER);",
    )
    .expect("write the stage script");

    let mut backend = InMemoryBackend::new();
    initialise_stage(&mut backend, &stage_dir, version("3.2"), "stage")
        .expect("the cold start succeeds");

    assert_eq!(backend.history.len(), 1);
    assert_eq!(backend.history[0].script, STAGE_SCRIPT);
    assert_eq!(backend.executed_sql.len(), 2);
}

#[test]
fn the_stage_marker_carries_the_canonical_catalog_hash() {
    let dir = workspace("stage-hash");
    let stage_dir = dir.join("schema");
    std::fs::create_dir_all(&stage_dir).expect("create the stage directory");
    std::fs::write(
        stage_dir.join("users.sql"),
        "CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT NOT NULL);",
    )
    .expect("write the stage script");

    // The marker is written by the backend that really applies the stage, so its
    // digest is read from the catalog that backend introspects.
    let connection = rusqlite::Connection::open_in_memory().expect("open the in-memory database");
    let mut backend = SqliteMigrationBackend::new(connection);
    initialise_stage(&mut backend, &stage_dir, version("3.2"), "stage")
        .expect("the cold start succeeds");

    let stage = DeclarativeStage::load(&stage_dir)
        .expect("the stage is readable")
        .expect("the stage directory exists");
    let catalog = materialise_stage(&stage, ":memory:").expect("the stage materialises");

    // The marker anchors the canonical catalog digest the checkpoint compares
    // against, not the raw DDL digest of the stage directory.
    assert_ne!(catalog.checksum(), stage.checksum());
    let history = backend.read_history().expect("the history is readable");
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].checksum, Some(catalog.checksum()));
}

#[test]
fn committing_the_stage_anchors_the_checkpoint() {
    let dir = workspace("stage-commit");
    let lock_path = dir.join("schema.lock");
    let checkpoint_path = dir.join("checkpoint.toml");

    commit_stage(version("3.2"), ":memory:", &lock_path, &checkpoint_path)
        .expect("an empty database commits cleanly");

    let catalog = SchemaCatalog::from_lock(
        &std::fs::read_to_string(&lock_path).expect("read the lock"),
    )
    .expect("the lock is canonical");
    let checkpoint = Checkpoint::load(&checkpoint_path)
        .expect("the checkpoint is readable")
        .expect("the checkpoint exists");

    assert_eq!(checkpoint.pruned_up_to_version().to_string(), "3.2");
    assert_eq!(checkpoint.schema_hash(), catalog.checksum());
}

#[test]
fn the_committed_boundary_is_never_rewound() {
    let dir = workspace("stage-rewind");
    let lock_path = dir.join("schema.lock");
    let checkpoint_path = dir.join("checkpoint.toml");
    commit_stage(version("5"), ":memory:", &lock_path, &checkpoint_path)
        .expect("the first commit succeeds");

    let error = commit_stage(version("3"), ":memory:", &lock_path, &checkpoint_path)
        .expect_err("the boundary cannot move backwards");

    assert!(error.to_string().contains("re-open"));
}

#[test]
fn prune_removes_only_the_scripts_below_the_boundary() {
    let dir = workspace("prune");
    let migrations_dir = dir.join("migrations");
    scripts(
        &migrations_dir,
        &["V1__init.sql", "V2__orders.sql", "V9__tail.sql", "R__view.sql"],
    );
    let checkpoint_path = dir.join("checkpoint.toml");
    checkpoint_file(&checkpoint_path, "9");

    prune_migrations(version("3"), &migrations_dir, &checkpoint_path, false)
        .expect("the prune succeeds");

    let mut remaining: Vec<String> = std::fs::read_dir(&migrations_dir)
        .expect("read the migration directory")
        .map(|entry| {
            entry
                .expect("a directory entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    remaining.sort();

    assert_eq!(remaining, vec!["R__view.sql", "V9__tail.sql"]);
}

#[test]
fn prune_refuses_to_drop_scripts_the_tail_still_needs() {
    let dir = workspace("prune-boundary");
    let migrations_dir = dir.join("migrations");
    scripts(&migrations_dir, &["V2__orders.sql"]);
    let checkpoint_path = dir.join("checkpoint.toml");
    checkpoint_file(&checkpoint_path, "1");

    let error = prune_migrations(version("3"), &migrations_dir, &checkpoint_path, false)
        .expect_err("the checkpoint sits below the requested boundary");

    assert!(error.to_string().contains("still needs"));
    assert!(migrations_dir.join("V2__orders.sql").exists());
}

#[test]
fn a_written_artifact_leaves_no_staged_sibling_behind() {
    let dir = workspace("atomic-write");
    let target = dir.join("schema.lock");

    write_file(&target, "snapshot").expect("the artifact is written");

    let contents = std::fs::read_to_string(&target).expect("read the artifact");
    assert_eq!(contents, "snapshot");

    let mut entries: Vec<String> = std::fs::read_dir(&dir)
        .expect("read the workspace")
        .map(|entry| {
            entry
                .expect("a directory entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    entries.sort();

    assert_eq!(entries, vec!["schema.lock".to_owned()]);
}

#[test]
fn a_failed_write_reports_the_directory_it_could_not_prepare() {
    let dir = workspace("atomic-failure");
    let blocker = dir.join("blocker");
    std::fs::write(&blocker, "not a directory").expect("write the blocking file");

    let error = write_file(&blocker.join("schema.lock"), "snapshot")
        .expect_err("a file cannot hold the artifact");

    assert!(error.to_string().contains("blocker"), "{error}");
}

#[test]
fn a_failed_checkpoint_write_keeps_the_committed_lock_readable() {
    let dir = workspace("commit-partial");
    let lock_path = dir.join("schema.lock");
    let blocker = dir.join("blocker");
    std::fs::write(&blocker, "not a directory").expect("write the blocking file");
    let checkpoint_path = blocker.join("checkpoint.toml");

    // The snapshot lands before the marker that anchors it, so a marker that
    // cannot be written never leaves a checkpoint pointing at a lock file that
    // describes a different database.
    let error = commit_stage(version("3.2"), ":memory:", &lock_path, &checkpoint_path)
        .expect_err("a file cannot hold the checkpoint");

    assert!(error.to_string().contains("blocker"), "{error}");

    let lock = std::fs::read_to_string(&lock_path).expect("read the lock");
    SchemaCatalog::from_lock(&lock).expect("the committed lock stays canonical");
}

#[test]
fn a_dry_run_keeps_every_script_in_place() {
    let dir = workspace("prune-dry-run");
    let migrations_dir = dir.join("migrations");
    scripts(&migrations_dir, &["V1__init.sql"]);
    let checkpoint_path = dir.join("checkpoint.toml");
    checkpoint_file(&checkpoint_path, "3");

    prune_migrations(version("3"), &migrations_dir, &checkpoint_path, true)
        .expect("the dry run succeeds");

    assert!(migrations_dir.join("V1__init.sql").exists());
}
