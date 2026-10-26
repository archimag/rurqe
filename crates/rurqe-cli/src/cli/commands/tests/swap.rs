//! Regressions for the boundary swap and the parked copies it leaves behind.

use rurqe_migrate::checkpoint::Checkpoint;
use rurqe_migrate::{Checksum, DeclarativeStage, StageScript};

use super::super::stage::{
    flush_boundary_parent, move_failure, recover_interrupted_commit, write_staged_boundary,
};
use super::fixtures::{directory_names, version, workspace};

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

    let notice = recover_interrupted_commit(&checkpoint_path)
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

    recover_interrupted_commit(&checkpoint_path).expect("the interrupted commit is repaired");

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

    let notice = recover_interrupted_commit(&checkpoint_path)
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
