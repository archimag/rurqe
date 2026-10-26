//! Regressions for the exclusive checkpoint section and the writers it arbitrates.

use std::time::Duration;

use rurqe_migrate::checkpoint::Checkpoint;
use rurqe_migrate::{Checksum, DeclarativeStage};

use crate::introspect::DEFAULT_SCHEMA;
use crate::stage::materialise_stage;

use super::super::commit_stage;
use super::super::stage::lock::CheckpointLock;
use super::super::stage::{recover_interrupted_commit, verify_stage};
use super::fixtures::{directory_names, lock_entry, version, workspace};

#[test]
fn a_commit_that_cannot_take_the_section_writes_no_artifact() {
    let dir = workspace("commit-partial");
    let stage_dir = dir.join("schema");
    std::fs::create_dir_all(&stage_dir).expect("create the stage directory");
    let lock_path = dir.join("schema.lock");
    let blocker = dir.join("blocker");
    std::fs::write(&blocker, "not a directory").expect("write the blocking file");
    let checkpoint_path = blocker.join("checkpoint.toml");

    // The section guards every artifact write of the commit — the lock, the
    // boundary, and the marker — so a commit whose checkpoint directory cannot
    // even be prepared for the lock entry fails before the first one: nothing
    // is left behind for a later commit or a verification to disagree with.
    let error = commit_stage(
        version("3.2"),
        ":memory:",
        DEFAULT_SCHEMA,
        &stage_dir,
        &lock_path,
        &checkpoint_path,
    )
    .expect_err("a file cannot hold the checkpoint artifacts");

    assert!(error.to_string().contains("blocker"), "{error}");
    assert!(
        !lock_path.exists(),
        "a commit that never entered its section writes no lock"
    );
    assert!(
        !checkpoint_path.exists(),
        "a commit that never entered its section writes no checkpoint"
    );
    assert!(
        !blocker.join("checkpoint-stage").exists(),
        "a commit that never entered its section writes no boundary snapshot"
    );
}

#[test]
fn the_parked_copies_of_a_live_foreign_writer_are_never_repaired() {
    let dir = workspace("commit-foreign");
    let checkpoint_path = dir.join("checkpoint.toml");

    // The entry records the writer pid 1, so these artifacts belong to a section
    // this process does not hold: the pair that writer parked may still be the only
    // recoverable copy of its state.
    lock_entry(&dir, 1, 2, Duration::ZERO);
    let marker_backup = dir.join(".checkpoint.toml.1.2.previous");
    std::fs::write(
        &marker_backup,
        Checkpoint::new(version("1"), Checksum::of(b"stage")).render(),
    )
    .expect("park the previous marker");
    let boundary_backup = dir.join(".checkpoint-stage.1.2.previous");
    std::fs::create_dir_all(&boundary_backup).expect("park the previous boundary");
    std::fs::write(
        boundary_backup.join("users.sql"),
        "CREATE TABLE users (id INTEGER);",
    )
    .expect("write the parked snapshot");

    let error = recover_interrupted_commit(&checkpoint_path)
        .expect_err("a live foreign writer owns the artifacts");

    let message = error.to_string();
    assert!(message.contains("pid 1"), "got {message}");
    assert!(message.contains("nonce 2"), "got {message}");

    // Nothing is renamed, restored, or discarded while the writer lives: the pair
    // it parked is left exactly where it parked it.
    assert!(marker_backup.exists(), "the foreign marker copy is untouched");
    assert!(
        boundary_backup.exists(),
        "the foreign boundary copy is untouched"
    );
    assert!(!checkpoint_path.exists(), "no marker was restored");
    assert!(
        !dir.join("checkpoint-stage").exists(),
        "no boundary was restored"
    );
}

#[test]
fn the_parked_copies_of_a_crashed_writer_are_repaired() {
    let dir = workspace("commit-dead-owner");
    let checkpoint_path = dir.join("checkpoint.toml");

    // The entry names this process, which acquired the section after the writer that
    // parked this pair died: the kernel released that writer's advisory lock when its
    // process exited, so the copies it left behind carry a foreign scope and are
    // debris this run repairs.
    lock_entry(&dir, std::process::id(), 1, Duration::ZERO);
    let marker_backup = dir.join(".checkpoint.toml.1.2.previous");
    std::fs::write(
        &marker_backup,
        Checkpoint::new(version("1"), Checksum::of(b"stage")).render(),
    )
    .expect("park the previous marker");
    let boundary_backup = dir.join(".checkpoint-stage.1.2.previous");
    std::fs::create_dir_all(&boundary_backup).expect("park the previous boundary");
    std::fs::write(
        boundary_backup.join("users.sql"),
        "CREATE TABLE users (id INTEGER);",
    )
    .expect("write the parked snapshot");

    let notice = recover_interrupted_commit(&checkpoint_path)
        .expect("the crashed writer's pair is repaired")
        .expect("the repair is reported");

    assert!(notice.contains("Restored"), "got {notice}");
    assert!(checkpoint_path.exists(), "the parked marker is restored");
    assert_eq!(
        std::fs::read_to_string(dir.join("checkpoint-stage").join("users.sql"))
            .expect("read the restored snapshot"),
        "CREATE TABLE users (id INTEGER);"
    );
    assert!(!marker_backup.exists(), "the parked marker is gone");
    assert!(!boundary_backup.exists(), "the parked boundary is gone");
}

#[test]
fn a_commit_takes_over_the_section_of_a_crashed_writer() {
    let dir = workspace("commit-takeover");
    let stage_dir = dir.join("schema");
    std::fs::create_dir_all(&stage_dir).expect("create the stage directory");
    let lock_path = dir.join("schema.lock");
    let checkpoint_path = dir.join("checkpoint.toml");

    // The entry names a writer that died holding the section. Its record is old
    // because that writer entered the section long before it crashed, and the age
    // keeps it dead: the kernel released the writer's advisory lock when its process
    // exited, so the commit acquires the section at once instead of waiting for an
    // owner that no longer exists.
    lock_entry(&dir, 1, 2, Duration::from_secs(120));

    commit_stage(
        version("3.2"),
        ":memory:",
        DEFAULT_SCHEMA,
        &stage_dir,
        &lock_path,
        &checkpoint_path,
    )
    .expect("the section of the crashed writer is acquired");

    // The entry persists and the commit released the section, so the next writer
    // acquires it again instead of waiting behind a command that already left.
    CheckpointLock::acquire(&checkpoint_path).expect("the committed section is free again");
    assert!(
        checkpoint_path.exists(),
        "a commit over a crashed writer's section still writes its marker"
    );

    let leftovers: Vec<String> = directory_names(&dir)
        .into_iter()
        .filter(|name| name.starts_with('.'))
        .collect();
    assert!(leftovers.is_empty(), "leftover artifacts: {leftovers:?}");
}

#[test]
fn a_verification_takes_over_the_section_of_a_crashed_writer() {
    let dir = workspace("verify-takeover");
    let stage_dir = dir.join("schema");
    std::fs::create_dir_all(&stage_dir).expect("create the stage directory");
    let users = "CREATE TABLE users (id INTEGER);";
    std::fs::write(stage_dir.join("users.sql"), users).expect("write the stage script");
    let migrations_dir = dir.join("migrations");
    std::fs::create_dir_all(&migrations_dir).expect("create the migrations directory");

    // The boundary is live beside the marker and no copy is parked, so the
    // repair the verification runs first finds nothing to classify and the
    // equation replays exactly the pair both artifacts describe.
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
    let boundary = dir.join("checkpoint-stage");
    std::fs::create_dir_all(&boundary).expect("install the boundary");
    std::fs::write(boundary.join("users.sql"), users).expect("write the boundary snapshot");

    lock_entry(&dir, 1, 2, Duration::from_secs(120));

    verify_stage(
        &stage_dir,
        None,
        &migrations_dir,
        &checkpoint_path,
        DEFAULT_SCHEMA,
        ":memory:",
    )
    .expect("the section of the crashed writer is acquired");

    // The entry persists and the verification released the section together with
    // the reads it guarded, so the next writer acquires it again.
    CheckpointLock::acquire(&checkpoint_path).expect("the verified section is free again");
}
