//! Regressions for `prune_migrations`.

use std::time::Duration;

use super::super::prune_migrations;
use super::super::stage::lock::CheckpointLock;
use super::fixtures::{checkpoint_file, lock_entry, scripts, version, workspace};

#[test]
fn prune_removes_only_the_scripts_below_the_boundary() {
    let dir = workspace("prune");
    let migrations_dir = dir.join("migrations");
    scripts(
        &migrations_dir,
        &[
            "V1__init.sql",
            "V2__orders.sql",
            "V9__tail.sql",
            "R__view.sql",
        ],
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

#[test]
fn a_pruned_report_reaches_the_operator_through_the_shared_channel() {
    let dir = workspace("prune-channel");
    let migrations_dir = dir.join("migrations");
    scripts(&migrations_dir, &["V1__init.sql", "V2__orders.sql"]);
    let checkpoint_path = dir.join("checkpoint.toml");
    checkpoint_file(&checkpoint_path, "3");

    let output = prune_migrations(version("3"), &migrations_dir, &checkpoint_path, false)
        .expect("the prune succeeds");

    // The command reports one line per pruned script plus the summary, so the
    // routing of a report is observed on a report that really carries several
    // lines: none of them is dropped on the way to the operator.
    assert_eq!(
        output.lines().len(),
        3,
        "unexpected report: {:?}",
        output.lines()
    );
    assert_eq!(
        output.lines().last().map(String::as_str),
        Some("Pruned 2 migration(s) below version 3")
    );

    let mut routed = Vec::new();
    output
        .write_into(&mut routed)
        .expect("writing to a buffer cannot fail");

    let mut expected = Vec::new();
    for line in output.lines() {
        crate::write_message(&mut expected, crate::MessageClass::Report, line)
            .expect("writing to a buffer cannot fail");
    }

    assert_eq!(
        routed, expected,
        "the report leaves the crate through one write path"
    );
}

#[test]
fn a_prune_takes_over_the_section_of_a_crashed_writer() {
    let dir = workspace("prune-takeover");
    let migrations_dir = dir.join("migrations");
    // `V4__orders.sql` sits above the prune boundary, so the prune drops the
    // version-1 script alone and the tail script stays in place.
    scripts(&migrations_dir, &["V1__init.sql", "V4__orders.sql"]);
    let checkpoint_path = dir.join("checkpoint.toml");
    checkpoint_file(&checkpoint_path, "9");

    // The prune holds the same exclusive section as the commit and the
    // verification, so it acquires over the entry a crashed writer left behind: the
    // kernel released that writer's advisory lock when its process exited.
    lock_entry(&dir, 1, 2, Duration::from_secs(120));

    prune_migrations(version("3"), &migrations_dir, &checkpoint_path, false)
        .expect("the section of the crashed writer is acquired");

    // The entry persists and the prune released the section, so the next writer
    // acquires it again.
    CheckpointLock::acquire(&checkpoint_path).expect("the pruned section is free again");
    assert!(
        !migrations_dir.join("V1__init.sql").exists(),
        "a prune over a crashed writer's section still drops the scripts below the boundary"
    );
    assert!(
        migrations_dir.join("V4__orders.sql").exists(),
        "the prune keeps the scripts above the boundary"
    );
}
