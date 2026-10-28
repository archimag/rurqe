//! Regressions for `initialise_stage` and `commit_stage`.

use rurqe_migrate::checkpoint::Checkpoint;
use rurqe_migrate::runner::MigrationBackend;
use rurqe_migrate::{Checksum, DeclarativeStage, InMemoryBackend, SchemaCatalog, STAGE_SCRIPT};

use crate::driver::SqliteMigrationBackend;
use crate::introspect::DEFAULT_SCHEMA;
use crate::stage::materialise_stage;

use super::super::{commit_stage, initialise_stage};
use super::fixtures::{apply_live_ddl, directory_names, version, workspace};

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
    let catalog =
        materialise_stage(&stage, ":memory:", DEFAULT_SCHEMA).expect("the stage materialises");

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
    let stage_dir = dir.join("schema");
    std::fs::create_dir_all(&stage_dir).expect("create the stage directory");
    let lock_path = dir.join("schema.lock");
    let checkpoint_path = dir.join("checkpoint.toml");

    commit_stage(
        version("3.2"),
        ":memory:",
        DEFAULT_SCHEMA,
        &stage_dir,
        &lock_path,
        &checkpoint_path,
    )
    .expect("an empty database commits cleanly");

    let catalog =
        SchemaCatalog::from_lock(&std::fs::read_to_string(&lock_path).expect("read the lock"))
            .expect("the lock is canonical");
    let checkpoint = Checkpoint::load(&checkpoint_path)
        .expect("the checkpoint is readable")
        .expect("the checkpoint exists");

    assert_eq!(checkpoint.pruned_up_to_version().to_string(), "3.2");
    assert_eq!(checkpoint.schema_hash(), catalog.checksum());
}

#[test]
fn committing_the_stage_snapshots_the_boundary_verbatim() {
    let dir = workspace("stage-snapshot");
    let stage_dir = dir.join("schema");
    std::fs::create_dir_all(&stage_dir).expect("create the stage directory");
    let users = "CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT NOT NULL);";
    std::fs::write(stage_dir.join("users.sql"), users).expect("write the stage script");

    // The live database holds exactly what the declarative stage declares, so the
    // parity gate lets the commit seal an agreeing pair of descriptions.
    let database = dir.join("app.db");
    apply_live_ddl(&database, users);

    let lock_path = dir.join("schema.lock");
    let checkpoint_path = dir.join("checkpoint.toml");

    commit_stage(
        version("3.2"),
        &database.to_string_lossy(),
        DEFAULT_SCHEMA,
        &stage_dir,
        &lock_path,
        &checkpoint_path,
    )
    .expect("the boundary is committed together with its snapshot");

    let snapshot = dir.join("checkpoint-stage");
    assert_eq!(
        std::fs::read_to_string(snapshot.join("users.sql")).expect("read the snapshot"),
        users
    );

    // A later commit replaces the snapshot, so a script the new boundary
    // dropped never lingers next to the marker. The live database follows the
    // stage, which keeps the parity gate satisfied.
    std::fs::remove_file(stage_dir.join("users.sql")).expect("drop the script");
    std::fs::write(
        stage_dir.join("orders.sql"),
        "CREATE TABLE orders (id INTEGER);",
    )
    .expect("write the new script");
    apply_live_ddl(
        &database,
        "DROP TABLE users; CREATE TABLE orders (id INTEGER);",
    );

    commit_stage(
        version("4"),
        &database.to_string_lossy(),
        DEFAULT_SCHEMA,
        &stage_dir,
        &lock_path,
        &checkpoint_path,
    )
    .expect("the boundary is re-committed");

    let mut names: Vec<String> = std::fs::read_dir(&snapshot)
        .expect("read the snapshot directory")
        .map(|entry| {
            entry
                .expect("a directory entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();

    assert_eq!(names, vec!["orders.sql".to_owned()]);

    // The staging directory and the backup of the replaced boundary are gone
    // again, so the swap leaves nothing but the committed snapshot behind.
    let leftovers: Vec<String> = std::fs::read_dir(&dir)
        .expect("read the workspace")
        .map(|entry| {
            entry
                .expect("a directory entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .filter(|name| name.starts_with('.'))
        .collect();

    assert!(leftovers.is_empty(), "leftover artifacts: {leftovers:?}");
}

#[test]
fn the_committed_boundary_is_never_rewound() {
    let dir = workspace("stage-rewind");
    let stage_dir = dir.join("schema");
    std::fs::create_dir_all(&stage_dir).expect("create the stage directory");
    let lock_path = dir.join("schema.lock");
    let checkpoint_path = dir.join("checkpoint.toml");
    commit_stage(
        version("5"),
        ":memory:",
        DEFAULT_SCHEMA,
        &stage_dir,
        &lock_path,
        &checkpoint_path,
    )
    .expect("the first commit succeeds");

    let error = commit_stage(
        version("3"),
        ":memory:",
        DEFAULT_SCHEMA,
        &stage_dir,
        &lock_path,
        &checkpoint_path,
    )
    .expect_err("the boundary cannot move backwards");

    assert!(error.to_string().contains("re-open"));

    // The refusal happens before the swap, so the staged snapshot the preparation
    // assembled is discarded instead of lingering beside the boundary it never
    // became.
    let leftovers: Vec<String> = directory_names(&dir)
        .into_iter()
        .filter(|name| name.starts_with('.'))
        .collect();
    assert!(leftovers.is_empty(), "leftover artifacts: {leftovers:?}");
}

#[test]
fn a_stage_that_drifted_from_the_database_is_never_committed() {
    let dir = workspace("stage-drift");
    let stage_dir = dir.join("schema");
    std::fs::create_dir_all(&stage_dir).expect("create the stage directory");
    // The declarative stage declares a column the live database never received,
    // which is exactly the drift the stage lifecycle exists to catch.
    std::fs::write(
        stage_dir.join("users.sql"),
        "CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT NOT NULL);",
    )
    .expect("write the stage script");

    let database = dir.join("app.db");
    apply_live_ddl(&database, "CREATE TABLE users (id INTEGER PRIMARY KEY);");

    let lock_path = dir.join("schema.lock");
    let checkpoint_path = dir.join("checkpoint.toml");

    let error = commit_stage(
        version("3.2"),
        &database.to_string_lossy(),
        DEFAULT_SCHEMA,
        &stage_dir,
        &lock_path,
        &checkpoint_path,
    )
    .expect_err("the drifted stage cannot be sealed into a checkpoint");

    let message = error.to_string();
    assert!(
        message.contains("does not describe the database"),
        "the diagnostic names the violated parity, got {message}"
    );
    assert!(
        message.contains("email"),
        "the diagnostic names the drifted column, got {message}"
    );
    assert!(
        message.contains("re-run `stage commit`"),
        "the diagnostic names the remedy, got {message}"
    );

    // The gate stands in front of every artifact, so a rejected commit leaves no
    // half-described triple behind: no marker, no snapshot, and no lock. The gate
    // also runs before the checkpoint section is taken, so it creates no entry a
    // later command would have to wait behind.
    assert!(
        !checkpoint_path.exists(),
        "a rejected commit writes no checkpoint"
    );
    assert!(
        !dir.join("checkpoint-stage").exists(),
        "a rejected commit writes no boundary snapshot"
    );
    assert!(!lock_path.exists(), "a rejected commit writes no lock");
    assert!(
        !dir.join("checkpoint.lock").exists(),
        "a rejected commit enters no section and leaves no lock entry"
    );
}

#[test]
fn two_sequential_commits_converge_to_the_last_one() {
    let dir = workspace("commit-sequence");
    let stage_dir = dir.join("schema");
    std::fs::create_dir_all(&stage_dir).expect("create the stage directory");
    let users = "CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT NOT NULL);";
    std::fs::write(stage_dir.join("users.sql"), users).expect("write the stage script");

    // The live database holds exactly what the declarative stage declares, so
    // both commits pass the parity gate that stands in front of the artifacts.
    let database = dir.join("app.db");
    apply_live_ddl(&database, users);

    let lock_path = dir.join("schema.lock");
    let checkpoint_path = dir.join("checkpoint.toml");

    commit_stage(
        version("3"),
        &database.to_string_lossy(),
        DEFAULT_SCHEMA,
        &stage_dir,
        &lock_path,
        &checkpoint_path,
    )
    .expect("the first commit succeeds");
    commit_stage(
        version("4"),
        &database.to_string_lossy(),
        DEFAULT_SCHEMA,
        &stage_dir,
        &lock_path,
        &checkpoint_path,
    )
    .expect("the second commit succeeds");

    // The second commit holds the section after the first one released it, so
    // it runs over the pair the first one left live: the marker it parks is
    // the first commit's own, and the workspace converges on the second one.
    let checkpoint = Checkpoint::load(&checkpoint_path)
        .expect("the checkpoint is readable")
        .expect("the checkpoint exists");
    assert_eq!(checkpoint.pruned_up_to_version().to_string(), "4");
    assert_eq!(
        std::fs::read_to_string(dir.join("checkpoint-stage").join("users.sql"))
            .expect("read the committed boundary"),
        users
    );

    // No recoverable copy of a previous pair is left behind: the first commit's
    // pair was parked by the second and discarded once the second marker was
    // durable, and the second commit has no previous pair of its own.
    let leftovers: Vec<String> = directory_names(&dir)
        .into_iter()
        .filter(|name| name.starts_with('.'))
        .collect();
    assert!(leftovers.is_empty(), "leftover artifacts: {leftovers:?}");
}

#[test]
fn a_stage_rejected_by_the_parity_gate_leaves_an_interrupted_commit_parked() {
    let dir = workspace("commit-recovery-failure");
    let stage_dir = dir.join("schema");
    std::fs::create_dir_all(&stage_dir).expect("create the stage directory");
    // The declarative stage declares a column the live database never received,
    // so the parity gate rejects the commit during its preparation, before the
    // section is taken and before any repair could run.
    std::fs::write(
        stage_dir.join("users.sql"),
        "CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT NOT NULL);",
    )
    .expect("write the stage script");
    let database = dir.join("app.db");
    apply_live_ddl(&database, "CREATE TABLE users (id INTEGER PRIMARY KEY);");

    let lock_path = dir.join("schema.lock");
    let checkpoint_path = dir.join("checkpoint.toml");

    // A killed commit left the previous marker and boundary parked. The gate runs
    // outside the section, so the commit neither restores nor discards the pair:
    // the only copy of the previous state stays exactly where it was parked.
    let marker_backup = dir.join(".checkpoint.toml.4242.7.previous");
    std::fs::write(
        &marker_backup,
        Checkpoint::new(version("1"), Checksum::of(b"stage")).render(),
    )
    .expect("park the previous marker");
    let boundary_backup = dir.join(".checkpoint-stage.4242.7.previous");
    std::fs::create_dir_all(&boundary_backup).expect("park the previous boundary");
    std::fs::write(
        boundary_backup.join("users.sql"),
        "CREATE TABLE users (id INTEGER);",
    )
    .expect("write the parked snapshot");

    let error = commit_stage(
        version("3.2"),
        &database.to_string_lossy(),
        DEFAULT_SCHEMA,
        &stage_dir,
        &lock_path,
        &checkpoint_path,
    )
    .expect_err("the drifted stage cannot be sealed into a checkpoint");

    let message = error.to_string();
    assert!(
        message.contains("does not describe the database"),
        "the diagnostic names the violated parity, got {message}"
    );
    assert!(
        message.contains("email"),
        "the diagnostic names the drifted column, got {message}"
    );
    assert!(
        !message.contains("repaired an interrupted commit"),
        "the gate runs before any repair, so the diagnostic carries no repair note, got {message}"
    );

    // The gate wrote nothing and repaired nothing: the parked pair is the only
    // copy of the previous state and stays parked, and no artifact of the commit
    // stands beside it.
    assert!(marker_backup.exists(), "the parked marker is untouched");
    assert!(boundary_backup.exists(), "the parked boundary is untouched");
    assert!(!checkpoint_path.exists(), "no marker was written");
    assert!(
        !dir.join("checkpoint-stage").exists(),
        "no boundary snapshot was written"
    );
    assert!(!lock_path.exists(), "no lock was written");
    assert!(
        !dir.join("checkpoint.lock").exists(),
        "the commit never entered its section"
    );
}
