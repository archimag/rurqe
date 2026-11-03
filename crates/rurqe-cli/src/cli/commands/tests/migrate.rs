//! Regressions for the `migrate` family reading its artifacts under the section.
//!
//! Each command is pinned twice: a held section refuses it and leaves the database
//! untouched, because the refusal precedes every backend interaction, and the same
//! command converges once the holder is gone, which is also what proves the section
//! it took was released again.

use std::path::Path;

use crate::driver::SqliteMigrationBackend;

use super::super::stage::lock::{simulate_section_holder, CheckpointLock};
use super::super::{apply_migrations, repair_history, report_status};
use super::fixtures::{checkpoint_file, scripts, workspace};

/// The process id the planted section holder records.
const HOLDER_PID: u32 = 4242;

/// The nonce the planted section holder records beside its process id.
const HOLDER_NONCE: u128 = 17;

/// Opens a file-backed SQLite backend over `path`.
fn backend(path: &Path) -> SqliteMigrationBackend {
    SqliteMigrationBackend::new(
        rusqlite::Connection::open(path).expect("open the file-backed SQLite database"),
    )
}

/// Whether the database at `path` holds the migration history table.
fn history_table_exists(path: &Path) -> bool {
    let connection = rusqlite::Connection::open(path).expect("open the SQLite database");
    let count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master \
             WHERE type = 'table' AND name = '__rurqe_schema_history'",
            [],
            |row| row.get(0),
        )
        .expect("read the SQLite catalog");

    count > 0
}

/// Asserts that the diagnostic names the writer holding the section.
fn assert_refusal(error: &crate::cli::CliError) {
    let message = error.to_string();

    assert!(
        message.contains(&format!("pid {HOLDER_PID}")),
        "got {message}"
    );
    assert!(
        message.contains(&format!("nonce {HOLDER_NONCE}")),
        "got {message}"
    );

    // The typed class survives the command hop, so an embedder reads the
    // refusal as checkpoint-section contention instead of matching its prose.
    match error.cause() {
        Some(crate::cli::error::CliCause::SectionHeld(held)) => assert!(
            matches!(
                held.holder(),
                crate::cli::error::SectionHolder::Recorded {
                    pid: HOLDER_PID,
                    nonce: HOLDER_NONCE,
                    ..
                }
            ),
            "unexpected holder: {:?}",
            held.holder()
        ),
        other => panic!("expected the section-held cause, got {other:?}"),
    }
}
#[test]
fn apply_migrations_reads_the_tail_inside_the_checkpoint_section() {
    let dir = workspace("migrate-up-section");
    let migrations_dir = dir.join("migrations");
    scripts(&migrations_dir, &["V1__init.sql"]);
    let checkpoint_path = dir.join("checkpoint.toml");
    let database = dir.join("app.db");
    let mut backend = backend(&database);

    let holder = simulate_section_holder(&checkpoint_path, HOLDER_PID, HOLDER_NONCE)
        .expect("a held section");

    let refusal = apply_migrations(&mut backend, &migrations_dir, &checkpoint_path)
        .expect_err("a live writer keeps its section");
    assert_refusal(&refusal);
    assert!(
        !history_table_exists(&database),
        "the refusal precedes every backend interaction"
    );

    drop(holder);

    let output = apply_migrations(&mut backend, &migrations_dir, &checkpoint_path)
        .expect("the released section is acquired");
    assert!(
        output
            .lines()
            .iter()
            .any(|line| line.contains("V1__init.sql")),
        "unexpected report: {:?}",
        output.lines()
    );

    CheckpointLock::acquire(&checkpoint_path).expect("the section is released after the read");
}

#[test]
fn report_status_reads_the_tail_inside_the_checkpoint_section() {
    let dir = workspace("migrate-status-section");
    let migrations_dir = dir.join("migrations");
    scripts(&migrations_dir, &["V1__init.sql", "V2__orders.sql"]);
    let checkpoint_path = dir.join("checkpoint.toml");
    let database = dir.join("app.db");
    let mut backend = backend(&database);

    let holder = simulate_section_holder(&checkpoint_path, HOLDER_PID, HOLDER_NONCE)
        .expect("a held section");

    let refusal = report_status(&mut backend, &migrations_dir, &checkpoint_path)
        .expect_err("a live writer keeps its section");
    assert_refusal(&refusal);
    assert!(
        !history_table_exists(&database),
        "the refusal precedes every backend interaction"
    );

    drop(holder);

    let output = report_status(&mut backend, &migrations_dir, &checkpoint_path)
        .expect("the released section is acquired");
    assert!(
        output
            .lines()
            .iter()
            .any(|line| line.contains("V1__init.sql")),
        "unexpected report: {:?}",
        output.lines()
    );

    CheckpointLock::acquire(&checkpoint_path).expect("the section is released after the read");
}

#[test]
fn repair_history_reads_the_tail_inside_the_checkpoint_section() {
    let dir = workspace("migrate-repair-section");
    let migrations_dir = dir.join("migrations");
    scripts(&migrations_dir, &["V1__init.sql"]);
    let checkpoint_path = dir.join("checkpoint.toml");
    let database = dir.join("app.db");
    let mut backend = backend(&database);

    let holder = simulate_section_holder(&checkpoint_path, HOLDER_PID, HOLDER_NONCE)
        .expect("a held section");

    let refusal = repair_history(&mut backend, &migrations_dir, &checkpoint_path)
        .expect_err("a live writer keeps its section");
    assert_refusal(&refusal);
    assert!(
        !history_table_exists(&database),
        "the refusal precedes every backend interaction"
    );

    drop(holder);

    let output = repair_history(&mut backend, &migrations_dir, &checkpoint_path)
        .expect("the released section is acquired");
    assert!(
        output
            .lines()
            .iter()
            .any(|line| line.contains("Removed 0 failed entr(ies), updated 0 checksum(s)")),
        "unexpected report: {:?}",
        output.lines()
    );

    CheckpointLock::acquire(&checkpoint_path).expect("the section is released after the read");
}

#[test]
fn report_status_reports_the_boundary_the_next_run_would_refuse() {
    let dir = workspace("migrate-status-boundary");
    let migrations_dir = dir.join("migrations");
    scripts(&migrations_dir, &["V1__init.sql"]);
    let checkpoint_path = dir.join("checkpoint.toml");
    let database = dir.join("app.db");
    let mut backend = backend(&database);

    apply_migrations(&mut backend, &migrations_dir, &checkpoint_path)
        .expect("the initial migration applies without a boundary");

    // The active tail, and a boundary the database sits below: the scripts that
    // would carry it across were pruned, so the run can only refuse.
    scripts(&migrations_dir, &["V3__orders.sql"]);
    checkpoint_file(&checkpoint_path, "2");

    let refusal = apply_migrations(&mut backend, &migrations_dir, &checkpoint_path)
        .expect_err("the pruned boundary refuses the tail");
    assert!(
        refusal.to_string().contains("pruned history up to `2`"),
        "got {refusal}"
    );

    let output = report_status(&mut backend, &migrations_dir, &checkpoint_path)
        .expect("the status of the tail is reported");

    assert!(
        output
            .lines()
            .iter()
            .any(|line| line.contains("V3__orders.sql") && line.contains("Pending")),
        "the tail script is listed: {:?}",
        output.lines()
    );
    assert!(
        output.lines().iter().any(|line| line
            .contains("`migrate up` would refuse this database")
            && line.contains("pruned history up to `2`")),
        "the boundary is reported beside the table: {:?}",
        output.lines()
    );

    // Without a marker the project runs the plain sequential lifecycle, so no
    // boundary is reported at all.
    std::fs::remove_file(&checkpoint_path).expect("drop the boundary marker");
    let output = report_status(&mut backend, &migrations_dir, &checkpoint_path)
        .expect("the status of the tail is reported");
    assert!(
        !output
            .lines()
            .iter()
            .any(|line| line.contains("would refuse this database")),
        "no boundary is reported without a marker: {:?}",
        output.lines()
    );
}