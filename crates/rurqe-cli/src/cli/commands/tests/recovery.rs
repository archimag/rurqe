//! Regressions for `verify_stage` and the repair it runs before the equation.

use rurqe_migrate::checkpoint::Checkpoint;
use rurqe_migrate::{Checksum, DeclarativeStage};

use crate::introspect::DEFAULT_SCHEMA;
use crate::stage::materialise_stage;

use super::super::stage::{lock::simulate_section_holder, verify_stage};
use super::fixtures::{directory_names, selection, version, workspace};

#[test]
fn verifying_an_interrupted_commit_restores_the_parked_boundary() {
    let dir = workspace("verify-restore");
    let stage_dir = dir.join("schema");
    std::fs::create_dir_all(&stage_dir).expect("create the stage directory");
    let users = "CREATE TABLE users (id INTEGER);";
    std::fs::write(stage_dir.join("users.sql"), users).expect("write the stage script");
    let migrations_dir = dir.join("migrations");
    std::fs::create_dir_all(&migrations_dir).expect("create the migrations directory");

    // The marker anchors the parked boundary, which a killed swap left beside
    // no live boundary.
    let stage = DeclarativeStage::load(&stage_dir)
        .expect("the stage is readable")
        .expect("the stage directory exists");
    let catalog =
        materialise_stage(&stage, ":memory:", DEFAULT_SCHEMA).expect("the stage materialises");
    let checkpoint_path = dir.join("checkpoint.toml");
    std::fs::write(
        &checkpoint_path,
        Checkpoint::new(version("1"), catalog.checksum()).render(),
    )
    .expect("write the checkpoint");

    let backup = dir.join(".checkpoint-stage.4242.11.previous");
    std::fs::create_dir_all(&backup).expect("park the committed boundary");
    std::fs::write(backup.join("users.sql"), users).expect("write the parked snapshot");

    // The verification repairs the interrupted commit and then finds the
    // boundary and its marker in agreement.
    verify_stage(
        &stage_dir,
        None,
        &migrations_dir,
        &checkpoint_path,
        &selection(),
        ":memory:",
    )
    .expect("the repaired checkpoint verifies");

    let boundary = dir.join("checkpoint-stage");
    assert_eq!(
        std::fs::read_to_string(boundary.join("users.sql")).expect("read the restored snapshot"),
        users
    );
    assert!(
        !backup.exists(),
        "the parked snapshot was renamed into place"
    );
}

#[test]
fn a_concurrent_command_never_preempts_a_live_writer() {
    let dir = workspace("concurrent-live-writer");
    let stage_dir = dir.join("schema");
    std::fs::create_dir_all(&stage_dir).expect("create the stage directory");
    let users = "CREATE TABLE users (id INTEGER);";
    std::fs::write(stage_dir.join("users.sql"), users).expect("write the stage script");
    let migrations_dir = dir.join("migrations");
    std::fs::create_dir_all(&migrations_dir).expect("create the migrations directory");

    // The marker anchors the boundary the database already stands on, so a
    // verification that reaches the equation finds the pair consistent.
    let stage = DeclarativeStage::load(&stage_dir)
        .expect("the stage is readable")
        .expect("the stage directory exists");
    let catalog =
        materialise_stage(&stage, ":memory:", DEFAULT_SCHEMA).expect("the stage materialises");
    let checkpoint_path = dir.join("checkpoint.toml");
    let marker = Checkpoint::new(version("1"), catalog.checksum()).render();
    std::fs::write(&checkpoint_path, &marker).expect("write the checkpoint");
    let boundary = dir.join("checkpoint-stage");
    std::fs::create_dir_all(&boundary).expect("install the boundary");
    std::fs::write(boundary.join("users.sql"), users).expect("write the boundary snapshot");

    // The writer is between the steps of its swap: its previous marker and
    // boundary are parked under its own scope, and the kernel keeps the section
    // for it alone until its description closes.
    let marker_backup = dir.join(".checkpoint.toml.1.2.previous");
    std::fs::write(
        &marker_backup,
        Checkpoint::new(version("1"), Checksum::of(b"stage")).render(),
    )
    .expect("park the previous marker");
    let boundary_backup = dir.join(".checkpoint-stage.1.2.previous");
    std::fs::create_dir_all(&boundary_backup).expect("park the previous boundary");
    std::fs::write(
        boundary_backup.join("older.sql"),
        "CREATE TABLE older (id INTEGER);",
    )
    .expect("write the parked snapshot");

    let holder = simulate_section_holder(&checkpoint_path, 1, 2).expect("a held section");

    // Negative half: the verification refuses, names the holder, and touches
    // nothing — neither the pair the writer parked nor the live pair beside it.
    let error = verify_stage(
        &stage_dir,
        None,
        &migrations_dir,
        &checkpoint_path,
        &selection(),
        ":memory:",
    )
    .expect_err("a live writer keeps its section");

    let message = error.to_string();
    assert!(message.contains("pid 1"), "got {message}");
    assert!(message.contains("nonce 2"), "got {message}");
    assert!(
        message.contains("never preempts"),
        "the refusal names the kernel as the arbiter, got {message}"
    );
    assert!(
        !message.contains("repaired an interrupted commit"),
        "the refusal runs no repair, got {message}"
    );

    // The in-flight writer's parked pair is the only copy of the state its
    // section owns, and no verification touches it while that writer lives.
    assert!(marker_backup.exists(), "the parked marker is untouched");
    assert!(boundary_backup.exists(), "the parked boundary is untouched");
    assert_eq!(
        std::fs::read_to_string(&checkpoint_path).expect("read the live marker"),
        marker,
        "the live marker is untouched"
    );
    assert_eq!(
        directory_names(&boundary),
        vec!["users.sql".to_owned()],
        "the live boundary is untouched"
    );

    // Positive half: the kernel releases the section the moment the writer's
    // description closes, so the same verification converges and discards the
    // parked pair as the debris of a writer that is provably gone.
    drop(holder);

    verify_stage(
        &stage_dir,
        None,
        &migrations_dir,
        &checkpoint_path,
        &selection(),
        ":memory:",
    )
    .expect("the released section verifies");

    assert!(!marker_backup.exists(), "the parked marker is discarded");
    assert!(
        !boundary_backup.exists(),
        "the parked boundary is discarded"
    );
    assert_eq!(
        directory_names(&boundary),
        vec!["users.sql".to_owned()],
        "the live boundary survives the repair"
    );
    assert_eq!(
        std::fs::read_to_string(boundary.join("users.sql")).expect("read the live snapshot"),
        users,
        "the live snapshot keeps its declarative script"
    );
}

#[test]
fn a_failed_verification_reports_the_interrupted_commit_it_repaired() {
    let dir = workspace("verify-recovery-failure");
    let stage_dir = dir.join("schema");
    std::fs::create_dir_all(&stage_dir).expect("create the stage directory");
    std::fs::write(
        stage_dir.join("users.sql"),
        "CREATE TABLE users (id INTEGER);",
    )
    .expect("write the stage script");
    let migrations_dir = dir.join("migrations");
    std::fs::create_dir_all(&migrations_dir).expect("create the migrations directory");
    let checkpoint_path = dir.join("checkpoint.toml");

    // The parked boundary replays a different relation than the declarative
    // stage, so the repaired checkpoint cannot satisfy the golden equation and
    // the failure has to say that the workspace already changed underneath it.
    let marker_backup = dir.join(".checkpoint.toml.4242.7.previous");
    std::fs::write(
        &marker_backup,
        Checkpoint::new(version("1"), Checksum::of(b"stage")).render(),
    )
    .expect("park the previous marker");
    let boundary_backup = dir.join(".checkpoint-stage.4242.7.previous");
    std::fs::create_dir_all(&boundary_backup).expect("park the previous boundary");
    std::fs::write(
        boundary_backup.join("orders.sql"),
        "CREATE TABLE orders (id INTEGER);",
    )
    .expect("write the parked snapshot");

    let error = verify_stage(
        &stage_dir,
        None,
        &migrations_dir,
        &checkpoint_path,
        &selection(),
        ":memory:",
    )
    .expect_err("the diverging stage cannot satisfy the golden equation");

    let message = error.to_string();
    assert!(
        message.contains("Restored the checkpoint marker"),
        "the failure names the repair that already moved artifacts, got {message}"
    );
    assert!(
        checkpoint_path.exists(),
        "the repair really restored the marker the failure reports"
    );
}
