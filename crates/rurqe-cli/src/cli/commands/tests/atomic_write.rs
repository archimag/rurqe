//! Regressions for the durable artifact writes and the sweeps of staged debris.

use std::time::Duration;

use crate::introspect::DEFAULT_SCHEMA;

use super::super::{commit_stage, sweep_stale_temporaries, write_file, STALE_TEMPORARY_AGE};
use super::fixtures::{backdate, directory_names, version, workspace};

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

#[cfg(unix)]
#[test]
fn a_failed_flush_after_the_rename_fails_the_write() {
    use std::os::unix::fs::PermissionsExt;

    use super::fixtures::permissions_are_enforced;

    let dir = workspace("atomic-flush");
    let target = dir.join("checkpoint.toml");
    write_file(&target, "previous").expect("the first artifact is written");

    if !permissions_are_enforced(&dir) {
        // A superuser reads through the withdrawn permission, so the flush
        // cannot be made to fail and the write stays durable either way.
        return;
    }

    // Renaming the staged bytes needs write and search permission on the
    // directory, while flushing the renamed entry needs the directory open
    // for reading, so the mode lets the rename through and fails the flush
    // alone.
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o300))
        .expect("withdraw the read permission of the directory");

    let result = write_file(&target, "replacement");

    // The permissions are restored before the assertions, so a failure among
    // them can never mask the artifact directory from the checks below.
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
        .expect("restore the permissions of the directory");

    let message = result.expect_err("the flush cannot be completed").to_string();
    assert!(message.contains("failed to flush"), "{message}");
    assert!(message.contains("checkpoint.toml"), "{message}");

    // The bytes are in place and the staging sibling is gone: the failure
    // reports the durability gap of the rename, not a missing artifact.
    assert_eq!(
        std::fs::read_to_string(&target).expect("read the artifact"),
        "replacement"
    );
    assert_eq!(directory_names(&dir), vec!["checkpoint.toml".to_owned()]);
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
fn a_sweep_discards_the_staged_artifacts_a_killed_process_left_behind() {
    let dir = workspace("sweep");
    let lock_path = dir.join("schema.lock");
    let boundary = dir.join("checkpoint-stage");

    // A kill between the write and the rename strands the hidden sibling, and
    // the next run derives a fresh sequence instead of touching it again.
    let stale_lock = dir.join(".schema.lock.4242.7.tmp");
    std::fs::write(&stale_lock, "snapshot").expect("plant a staged lock");
    let stale_boundary = dir.join(".checkpoint-stage.4242.9.tmp");
    std::fs::create_dir_all(stale_boundary.join("users.sql")).expect("plant a staged boundary");
    let owned_lock = dir.join(format!(".schema.lock.{}.3.tmp", std::process::id()));
    std::fs::write(&owned_lock, "in flight").expect("plant a staged lock of this process");

    let removed = sweep_stale_temporaries(&lock_path, Duration::ZERO)
        .expect("the sweep scans the artifact directory");

    assert_eq!(removed, vec![stale_lock.clone()]);
    assert!(!stale_lock.exists(), "the abandoned staged lock is gone");
    assert!(owned_lock.exists(), "a staged path this process owns is kept");

    let removed = sweep_stale_temporaries(&boundary, Duration::ZERO)
        .expect("the sweep scans the boundary directory");

    assert_eq!(removed, vec![stale_boundary.clone()]);
    assert!(
        !stale_boundary.exists(),
        "the abandoned staged boundary is gone"
    );
}

#[test]
fn a_sweep_keeps_a_staged_artifact_younger_than_the_threshold() {
    let dir = workspace("sweep-age");
    let lock_path = dir.join("schema.lock");
    let young = dir.join(".schema.lock.4242.1.tmp");
    std::fs::write(&young, "snapshot").expect("plant a staged lock");

    let removed = sweep_stale_temporaries(&lock_path, STALE_TEMPORARY_AGE)
        .expect("the sweep scans the artifact directory");

    assert!(removed.is_empty());
    assert!(young.exists(), "a young staged lock may still be in flight");
}

#[test]
fn committing_the_stage_sweeps_the_debris_of_an_interrupted_commit() {
    let dir = workspace("commit-sweep");
    let stage_dir = dir.join("schema");
    std::fs::create_dir_all(&stage_dir).expect("create the stage directory");
    let lock_path = dir.join("schema.lock");
    let checkpoint_path = dir.join("checkpoint.toml");

    let abandoned = Duration::from_secs(48 * 60 * 60);
    let stale_lock = dir.join(".schema.lock.4242.1.tmp");
    std::fs::write(&stale_lock, "snapshot").expect("plant a staged lock");
    backdate(&stale_lock, abandoned);

    let stale_marker = dir.join(".checkpoint.toml.4242.2.tmp");
    std::fs::write(&stale_marker, "marker").expect("plant a staged marker");
    backdate(&stale_marker, abandoned);

    let stale_boundary = dir.join(".checkpoint-stage.4242.3.tmp");
    std::fs::create_dir_all(stale_boundary.join("users.sql")).expect("plant a staged boundary");
    backdate(&stale_boundary, abandoned);

    commit_stage(
        version("3.2"),
        ":memory:",
        DEFAULT_SCHEMA,
        &stage_dir,
        &lock_path,
        &checkpoint_path,
    )
    .expect("the commit succeeds");

    assert!(
        !stale_lock.exists(),
        "the staged lock of the killed commit is swept"
    );
    assert!(
        !stale_marker.exists(),
        "the staged marker of the killed commit is swept"
    );
    assert!(
        !stale_boundary.exists(),
        "the staged boundary of the killed commit is swept"
    );
    assert!(
        dir.join("checkpoint-stage").exists(),
        "the committed boundary is in place"
    );

    let leftovers: Vec<String> = directory_names(&dir)
        .into_iter()
        .filter(|name| name.starts_with('.'))
        .collect();
    assert!(leftovers.is_empty(), "leftover artifacts: {leftovers:?}");
}
