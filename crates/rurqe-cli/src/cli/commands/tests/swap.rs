//! Regressions for the boundary swap and the parked copies it leaves behind.

use std::time::Duration;

use rurqe_migrate::checkpoint::Checkpoint;
use rurqe_migrate::{Checksum, DeclarativeStage, StageScript};

use crate::introspect::DEFAULT_SCHEMA;

use super::super::commit_stage;
use super::super::stage::lock::{CheckpointLock, WriterId};
use super::super::stage::{
    flush_boundary_parent, move_failure, recover_interrupted_commit, write_staged_boundary,
};
use super::fixtures::{directory_names, lock_entry, version, workspace};

#[test]
fn a_failed_staged_write_keeps_the_committed_boundary() {
    let dir = workspace("snapshot-staged-write");
    let boundary = dir.join("checkpoint-stage");
    std::fs::create_dir_all(&boundary).expect("create the committed boundary");
    std::fs::write(boundary.join("users.sql"), "CREATE TABLE users (id INTEGER);")
        .expect("write the committed snapshot");

    // `orders.sql` lands, and `users.sql` collides with a directory, so the
    // staging fails after one script has already been written.
    let staged = dir.join(".checkpoint-stage.staged.tmp");
    std::fs::create_dir_all(staged.join("users.sql")).expect("block the second script");

    let stage = DeclarativeStage::new(vec![
        StageScript::new("orders.sql", "CREATE TABLE orders (id INTEGER);"),
        StageScript::new("users.sql", "CREATE TABLE users (id BIGINT);"),
    ]);

    let error =
        write_staged_boundary(&stage, &staged).expect_err("the blocked script cannot be staged");

    assert!(error.to_string().contains("users.sql"), "{error}");
    assert!(
        staged.join("orders.sql").exists(),
        "the first script landed in the staging directory"
    );
    assert_eq!(
        std::fs::read_to_string(boundary.join("users.sql")).expect("read the committed snapshot"),
        "CREATE TABLE users (id INTEGER);"
    );
}

#[test]
fn an_interrupted_commit_restores_the_parked_pair() {
    let dir = workspace("commit-restore");
    let checkpoint_path = dir.join("checkpoint.toml");

    // The process was killed after parking the previous marker and boundary but
    // before installing the new boundary: both are parked under the scope of the
    // writer that parked them, nothing is live.
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

    // The repair runs under the section its caller holds, whose acquisition
    // recorded the very scope it reports.
    let section = CheckpointLock::acquire(&checkpoint_path).expect("the free section is taken");

    let notice = recover_interrupted_commit(&checkpoint_path, section.writer())
        .expect("the interrupted commit is repaired")
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
fn a_failed_rollback_names_the_parked_boundary_it_strands() {
    let dir = workspace("snapshot-stranded");
    let boundary = dir.join("checkpoint-stage");
    let staged = dir.join(".checkpoint-stage.4242.4.tmp");
    // The parked boundary is gone by the time the rollback runs, so putting it
    // back fails exactly like the installation did.
    let parked = dir.join(".checkpoint-stage.previous");
    let failure = std::io::Error::from(std::io::ErrorKind::PermissionDenied);

    let message = move_failure(&boundary, &staged, &failure, Some(&parked)).to_string();

    assert!(
        message.contains(&staged.display().to_string()),
        "the diagnostic names the snapshot that could not be installed, got {message}"
    );
    assert!(
        message.contains(&parked.display().to_string()),
        "the diagnostic names the copy that stays parked, got {message}"
    );
    assert!(
        message.contains("also failed"),
        "the diagnostic reports the failed rollback, got {message}"
    );
    assert!(
        message.contains("mv "),
        "the diagnostic names the rename that restores the boundary, got {message}"
    );
}

#[test]
fn a_failed_installation_that_restores_the_boundary_reports_no_stranding() {
    let dir = workspace("install-rollback");
    let boundary = dir.join("checkpoint-stage");
    let staged = dir.join(".checkpoint-stage.4242.5.tmp");
    let parked = dir.join(".checkpoint-stage.previous");

    // The parked boundary is a real directory, so the rollback inside
    // `move_failure` restores it successfully and reports no stranding.
    std::fs::create_dir_all(&parked).expect("park the committed boundary");
    std::fs::write(
        parked.join("users.sql"),
        "CREATE TABLE users (id INTEGER);",
    )
    .expect("write the parked snapshot");

    let failure = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
    let message = move_failure(&boundary, &staged, &failure, Some(&parked)).to_string();

    assert!(message.contains(&staged.display().to_string()), "{message}");
    assert!(
        !message.contains("also failed"),
        "the rollback succeeded, so the diagnostic reports no stranding, got {message}"
    );
    assert_eq!(
        std::fs::read_to_string(boundary.join("users.sql")).expect("read the restored boundary"),
        "CREATE TABLE users (id INTEGER);"
    );
    assert!(!parked.exists(), "the parked boundary was renamed back");
}

#[test]
fn a_cycle_over_one_marker_leaves_the_parked_pair_of_another_untouched() {
    let dir = workspace("foreign-parked");
    let stage_dir = dir.join("schema");
    std::fs::create_dir_all(&stage_dir).expect("create the stage directory");
    let checkpoint_path = dir.join("checkpoint.toml");
    let lock_path = dir.join("schema.lock");

    // The pair belongs to `prod.toml` and carries the scope of the writer that
    // parked it. A commit over `checkpoint.toml` scans for the copies of its own
    // marker and of its own boundary, whose prefixes these names never match.
    let marker_backup = dir.join(".prod.toml.4242.17.previous");
    std::fs::write(
        &marker_backup,
        Checkpoint::new(version("1"), Checksum::of(b"stage")).render(),
    )
    .expect("park the foreign marker");
    let boundary_backup = dir.join(".prod-stage.4242.17.previous");
    std::fs::create_dir_all(&boundary_backup).expect("park the foreign boundary");
    std::fs::write(
        boundary_backup.join("users.sql"),
        "CREATE TABLE users (id INTEGER);",
    )
    .expect("write the parked snapshot");

    commit_stage(
        version("3.2"),
        ":memory:",
        DEFAULT_SCHEMA,
        &stage_dir,
        &lock_path,
        &checkpoint_path,
    )
    .expect("the commit succeeds");

    // The recovery of the commit runs under the section of `checkpoint.toml`, so
    // the pair parked beside `prod.toml` is never classified, restored, or
    // discarded: it stays the only copy of the other marker's previous state.
    let section = CheckpointLock::acquire(&checkpoint_path).expect("the committed section is free");
    let notice = recover_interrupted_commit(&checkpoint_path, section.writer())
        .expect("the repair runs under its section");
    assert!(
        notice.is_none(),
        "no copy of this marker was interrupted, got {notice:?}"
    );

    assert!(marker_backup.exists(), "the foreign marker copy is untouched");
    assert!(
        boundary_backup.exists(),
        "the foreign boundary copy is untouched"
    );
    assert!(
        !dir.join("prod.toml").exists(),
        "no foreign marker was restored"
    );
    assert!(
        !dir.join("prod-stage").exists(),
        "no foreign boundary was restored"
    );
}

#[test]
fn the_parked_pair_of_another_marker_is_repaired_under_its_own_section() {
    let dir = workspace("foreign-repair");
    let prod_path = dir.join("prod.toml");

    let section = CheckpointLock::acquire(&prod_path).expect("the free section is taken");
    let scope = section.writer();

    // The pair is parked under the very scope the section reports, which its own
    // record echoes back, so the repair of `prod.toml` restores it while the pair
    // parked beside `checkpoint.toml` is left exactly where it sits.
    let marker_backup = dir.join(format!(".prod.toml.{}.{}.previous", scope.pid, scope.nonce));
    std::fs::write(
        &marker_backup,
        Checkpoint::new(version("2"), Checksum::of(b"prod")).render(),
    )
    .expect("park the marker");
    let boundary_backup = dir.join(format!(".prod-stage.{}.{}.previous", scope.pid, scope.nonce));
    std::fs::create_dir_all(&boundary_backup).expect("park the boundary");
    std::fs::write(
        boundary_backup.join("orders.sql"),
        "CREATE TABLE orders (id INTEGER);",
    )
    .expect("write the parked snapshot");
    let foreign_marker = dir.join(".checkpoint.toml.4242.17.previous");
    std::fs::write(
        &foreign_marker,
        Checkpoint::new(version("1"), Checksum::of(b"stage")).render(),
    )
    .expect("park another marker");

    let notice = recover_interrupted_commit(&prod_path, scope)
        .expect("the parked pair is repaired")
        .expect("the repair is reported");

    assert!(notice.contains("Restored"), "got {notice}");
    assert!(prod_path.exists(), "the parked marker is restored");
    assert_eq!(
        std::fs::read_to_string(dir.join("prod-stage").join("orders.sql"))
            .expect("read the restored snapshot"),
        "CREATE TABLE orders (id INTEGER);"
    );
    assert!(!marker_backup.exists(), "the parked marker is gone");
    assert!(!boundary_backup.exists(), "the parked boundary is gone");
    assert!(
        foreign_marker.exists(),
        "the copy of another marker stays untouched"
    );
}

#[test]
fn an_interrupted_commit_discards_the_boundary_it_installed_without_a_marker() {
    let dir = workspace("commit-installed");
    let checkpoint_path = dir.join("checkpoint.toml");

    // The process was killed after installing the new boundary but before
    // writing its marker: the previous marker and boundary are parked while the
    // installed boundary is live without a marker.
    let boundary = dir.join("checkpoint-stage");
    std::fs::create_dir_all(&boundary).expect("install the uncommitted boundary");
    std::fs::write(boundary.join("orders.sql"), "CREATE TABLE orders (id INTEGER);")
        .expect("write the installed snapshot");
    let marker_backup = dir.join(".checkpoint.toml.4242.8.previous");
    std::fs::write(
        &marker_backup,
        Checkpoint::new(version("1"), Checksum::of(b"stage")).render(),
    )
    .expect("park the previous marker");
    let boundary_backup = dir.join(".checkpoint-stage.4242.8.previous");
    std::fs::create_dir_all(&boundary_backup).expect("park the previous boundary");
    std::fs::write(
        boundary_backup.join("users.sql"),
        "CREATE TABLE users (id INTEGER);",
    )
    .expect("write the parked snapshot");

    // The repair runs under the section its caller holds, whose acquisition
    // recorded the very scope it reports.
    let section = CheckpointLock::acquire(&checkpoint_path).expect("the free section is taken");

    recover_interrupted_commit(&checkpoint_path, section.writer())
        .expect("the interrupted commit is repaired");

    assert!(checkpoint_path.exists(), "the parked marker is restored");
    assert_eq!(
        directory_names(&boundary),
        vec!["users.sql".to_owned()],
        "the boundary installed without a marker is replaced by the parked one"
    );
    assert!(!marker_backup.exists(), "the parked marker is gone");
    assert!(!boundary_backup.exists(), "the parked boundary is gone");
}

#[cfg(unix)]
#[test]
fn a_boundary_whose_directory_cannot_be_flushed_reports_the_gap() {
    use std::os::unix::fs::PermissionsExt;

    use super::fixtures::permissions_are_enforced;

    let dir = workspace("flush-gap");
    let boundary = dir.join("checkpoint-stage");
    std::fs::create_dir_all(&boundary).expect("create the boundary");

    if !permissions_are_enforced(&dir) {
        // A superuser reads through the withdrawn permission, so the flush
        // cannot be made to fail and the swap stays durable either way.
        return;
    }

    // Flushing the boundary's directory needs it open for reading.
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o300))
        .expect("withdraw the read permission of the directory");

    let notice = flush_boundary_parent(&boundary);

    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
        .expect("restore the permissions of the directory");

    let notice = notice.expect("the flush cannot be completed");
    assert!(notice.contains("could not be flushed"), "got {notice}");
}

#[test]
fn a_parked_pair_beside_a_committed_pair_is_discarded() {
    let dir = workspace("commit-discard");
    let checkpoint_path = dir.join("checkpoint.toml");

    // The commit reached its commit point: the live pair is committed and the
    // parked pair is the previous state the swap replaced.
    std::fs::write(
        &checkpoint_path,
        Checkpoint::new(version("2"), Checksum::of(b"new")).render(),
    )
    .expect("write the committed marker");
    let boundary = dir.join("checkpoint-stage");
    std::fs::create_dir_all(&boundary).expect("install the committed boundary");
    std::fs::write(boundary.join("orders.sql"), "CREATE TABLE orders (id INTEGER);")
        .expect("write the committed snapshot");

    let marker_backup = dir.join(".checkpoint.toml.4242.9.previous");
    std::fs::write(
        &marker_backup,
        Checkpoint::new(version("1"), Checksum::of(b"old")).render(),
    )
    .expect("park the previous marker");
    let boundary_backup = dir.join(".checkpoint-stage.4242.9.previous");
    std::fs::create_dir_all(&boundary_backup).expect("park the previous boundary");
    std::fs::write(
        boundary_backup.join("users.sql"),
        "CREATE TABLE users (id INTEGER);",
    )
    .expect("write the parked snapshot");

    // The repair runs under the section its caller holds, whose acquisition
    // recorded the very scope it reports.
    let section = CheckpointLock::acquire(&checkpoint_path).expect("the free section is taken");

    let notice = recover_interrupted_commit(&checkpoint_path, section.writer())
        .expect("the debris is discarded")
        .expect("the discard is reported");

    assert!(notice.contains("Discarded"), "got {notice}");
    assert!(!marker_backup.exists(), "the parked marker is discarded");
    assert!(!boundary_backup.exists(), "the parked boundary is discarded");
    assert_eq!(
        directory_names(&boundary),
        vec!["orders.sql".to_owned()],
        "the committed boundary is untouched"
    );
}

#[test]
fn a_recycled_pid_is_refused_until_the_section_is_taken() {
    let dir = workspace("commit-recycled-pid");
    let checkpoint_path = dir.join("checkpoint.toml");

    // A crashed writer left its record behind, and its pid recycled into this
    // process: the record names the current pid with the nonce of a section this
    // process never held, while the pair it parked sits under that very scope.
    lock_entry(&dir, std::process::id(), 4242, Duration::ZERO);
    let marker_backup = dir.join(format!(
        ".checkpoint.toml.{}.4242.previous",
        std::process::id()
    ));
    std::fs::write(
        &marker_backup,
        Checkpoint::new(version("1"), Checksum::of(b"stage")).render(),
    )
    .expect("park the previous marker");

    // Negative half: the caller reports the scope of a section the record does
    // not echo — the nonce alone disagrees — so the repair refuses instead of
    // restoring a pair that may still be the only recoverable copy of a foreign
    // section's state.
    let scope = WriterId {
        pid: std::process::id(),
        nonce: 4243,
    };
    let error = recover_interrupted_commit(&checkpoint_path, scope)
        .expect_err("a record that does not echo the reported scope is refused");

    let message = error.to_string();
    assert!(
        message.contains("nonce 4242"),
        "the diagnostic names the recorded writer, got {message}"
    );
    assert!(
        message.contains("nonce 4243"),
        "the diagnostic names the reported scope, got {message}"
    );
    assert!(
        message.contains("take the section"),
        "the diagnostic names the remedy, got {message}"
    );
    assert!(marker_backup.exists(), "the parked marker is untouched");
    assert!(
        !checkpoint_path.exists(),
        "no marker was restored over the parked pair"
    );

    // Positive half: the writer that recorded the stale entry is gone, so the
    // section is free and acquiring it rewrites the record with this writer's
    // own scope. The repair then runs under a section this process provably
    // holds and restores the debris of the dead writer.
    let section = CheckpointLock::acquire(&checkpoint_path).expect("the free section is taken");

    let notice = recover_interrupted_commit(&checkpoint_path, section.writer())
        .expect("the provably held section repairs the debris")
        .expect("the repair is reported");

    drop(section);

    assert!(notice.contains("Restored"), "got {notice}");
    assert!(checkpoint_path.exists(), "the parked marker is restored");
    assert!(!marker_backup.exists(), "the parked marker is consumed");
}
