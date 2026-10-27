//! Regressions for the durable artifact writes and the sweeps of staged debris.

use std::time::Duration;

#[cfg(unix)]
use crate::introspect::DEFAULT_SCHEMA;

use super::super::{sweep_stale_temporaries, write_file, STALE_TEMPORARY_AGE};
use super::fixtures::{DEAD_PID, backdate, futuredate, workspace};

// The discard regressions and the commit regression are Unix alone — only
// Unix exposes the liveness probe a sweep discards under — so the imports
// those tests alone reach are gated with them, and a test build on another
// platform compiles without an unused import.
#[cfg(unix)]
use super::super::commit_stage;
#[cfg(unix)]
use super::fixtures::{directory_names, version};

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

#[cfg(unix)]
#[test]
fn a_sweep_discards_the_staged_artifacts_a_killed_process_left_behind() {
    let dir = workspace("sweep");
    let lock_path = dir.join("schema.lock");
    let boundary = dir.join("checkpoint-stage");

    // A kill between the write and the rename strands the hidden sibling, and
    // the next run derives a fresh sequence instead of touching it again. The
    // owner provably names no process, so the liveness gate of the sweep cannot
    // keep the debris on a machine that happens to run a writer under the
    // number.
    let stale_lock = dir.join(format!(".schema.lock.{DEAD_PID}.7.tmp"));
    std::fs::write(&stale_lock, "snapshot").expect("plant a staged lock");
    let stale_boundary = dir.join(format!(".checkpoint-stage.{DEAD_PID}.9.tmp"));
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
    let young = dir.join(format!(".schema.lock.{DEAD_PID}.1.tmp"));
    std::fs::write(&young, "snapshot").expect("plant a staged lock");

    let removed = sweep_stale_temporaries(&lock_path, STALE_TEMPORARY_AGE)
        .expect("the sweep scans the artifact directory");

    assert!(removed.is_empty());
    assert!(young.exists(), "a young staged lock may still be in flight");
}

#[test]
fn a_sweep_keeps_a_staged_artifact_whose_writer_still_lives() {
    let dir = workspace("sweep-liveness");
    let lock_path = dir.join("schema.lock");

    // Pid 1 is `init`, so the owner provably lives on every Unix, and a
    // platform without the probe counts every foreign writer as living.
    let in_flight = dir.join(".schema.lock.1.7.tmp");
    std::fs::write(&in_flight, "snapshot").expect("plant a staged lock");

    // The modification time looks twice as old as the sweep threshold — the
    // shape a forward clock correction (an NTP step, a VM resume) leaves on a
    // sibling a live writer staged moments ago.
    backdate(&in_flight, STALE_TEMPORARY_AGE * 2);

    let removed = sweep_stale_temporaries(&lock_path, STALE_TEMPORARY_AGE)
        .expect("the sweep scans the artifact directory");

    assert!(removed.is_empty());
    assert!(
        in_flight.exists(),
        "the staging path of a live writer is kept"
    );
}

#[test]
fn a_sweep_keeps_a_staged_artifact_dated_ahead_of_the_clock() {
    let dir = workspace("sweep-future");
    let lock_path = dir.join("schema.lock");

    // A backward clock correction leaves a modification time ahead of the
    // clock, and the sweep measures age forwards only: a negative age is not
    // one, so the sibling is kept until the wall clock passes it.
    let ahead = dir.join(format!(".schema.lock.{DEAD_PID}.7.tmp"));
    std::fs::write(&ahead, "snapshot").expect("plant a staged lock");
    futuredate(&ahead, STALE_TEMPORARY_AGE);

    let removed = sweep_stale_temporaries(&lock_path, Duration::ZERO)
        .expect("the sweep scans the artifact directory");

    assert!(removed.is_empty());
    assert!(
        ahead.exists(),
        "a staged path dated ahead of the clock is kept"
    );
}

#[cfg(unix)]
#[test]
fn committing_the_stage_sweeps_the_debris_of_an_interrupted_commit() {
    let dir = workspace("commit-sweep");
    let stage_dir = dir.join("schema");
    std::fs::create_dir_all(&stage_dir).expect("create the stage directory");
    let lock_path = dir.join("schema.lock");
    let checkpoint_path = dir.join("checkpoint.toml");

    // The debris belongs to a writer that provably no longer exists, so the
    // liveness gate of the sweep cannot keep it on a machine that happens to
    // run a process under the recycled number.
    let abandoned = Duration::from_secs(48 * 60 * 60);
    let stale_lock = dir.join(format!(".schema.lock.{DEAD_PID}.1.tmp"));
    std::fs::write(&stale_lock, "snapshot").expect("plant a staged lock");
    backdate(&stale_lock, abandoned);

    let stale_marker = dir.join(format!(".checkpoint.toml.{DEAD_PID}.2.tmp"));
    std::fs::write(&stale_marker, "marker").expect("plant a staged marker");
    backdate(&stale_marker, abandoned);

    let stale_boundary = dir.join(format!(".checkpoint-stage.{DEAD_PID}.3.tmp"));
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
