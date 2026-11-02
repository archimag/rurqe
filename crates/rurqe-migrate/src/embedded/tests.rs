//! Unit tests of the embedded lifecycle protocol.

use super::*;
// Only the asynchronous baseline tests name `MigrationKind`, so the import
// carries the same feature gate as the tests that read it.
#[cfg(feature = "tokio")]
use crate::migration::MigrationKind;
use crate::runner::InMemoryBackend;

fn embedded() -> EmbeddedMigrations {
    EmbeddedMigrations::new(&[
        ("V1__users.sql", "CREATE TABLE users (id INTEGER);"),
        ("V2__orders.sql", "CREATE TABLE orders (id INTEGER);"),
    ])
    .expect("valid migrations")
}

#[test]
fn migrate_reports_every_applied_script() {
    let mut backend = InMemoryBackend::new();
    let report = embedded()
        .migrate(&mut backend)
        .expect("the migration run succeeds");

    assert_eq!(report.applied_count, 2);
    assert_eq!(backend.history.len(), 2);
}

#[test]
fn duplicate_script_names_are_rejected_at_construction() {
    let error = EmbeddedMigrations::new(&[
        ("R__view.sql", "CREATE VIEW v AS SELECT 1;"),
        ("R__view.sql", "CREATE VIEW v AS SELECT 2;"),
    ])
    .expect_err("two scripts under one name must be rejected");

    assert_eq!(
        error,
        Error::DuplicateScriptName {
            script: "R__view.sql".to_owned()
        }
    );
}

#[test]
fn from_migrations_rejects_duplicate_script_names() {
    let first = Migration::parse("R__view.sql", "SELECT 1;").expect("a valid script name");
    let second = Migration::parse("R__view.sql", "SELECT 2;").expect("a valid script name");

    let error = EmbeddedMigrations::from_migrations(vec![first, second])
        .expect_err("two scripts under one name must be rejected");

    assert_eq!(
        error,
        Error::DuplicateScriptName {
            script: "R__view.sql".to_owned()
        }
    );
}
/// A cold start closes every versioned migration the stage already reflects, and
/// never a repeatable one: repeatable scripts carry no version, so the marker
/// cannot cover them and they run on top of the stage the cold start produced.
///
/// The re-run is load-bearing. Suppressing it would leave the scripts unrecorded,
/// so the next plain `migrate` would apply them anyway — after the cold start
/// created the very objects they build.
#[test]
fn a_cold_start_closes_only_the_versioned_tail() {
    let catalog = SchemaCatalog::default();
    let snapshot = catalog.to_lock();

    let embedded = EmbeddedMigrations::from_artifacts(EmbeddedArtifacts {
        tail: &[
            ("V1__users.sql", "CREATE TABLE users (id INTEGER);"),
            (
                "R__active_users.sql",
                "CREATE VIEW active_users AS SELECT id FROM users;",
            ),
        ],
        checkpoint: None,
        stage: &[("users.sql", "CREATE TABLE users (id INTEGER, email TEXT);")],
        schema_lock: Some(&snapshot),
    })
    .expect("valid artifacts");

    let mut backend = InMemoryBackend::new();
    backend.catalog_checksum = Some(catalog.checksum());

    let report = embedded
        .migrate(&mut backend)
        .expect("the database is cold started");

    let cold = report.cold_start.as_ref().expect("the cold start runs");
    assert_eq!(cold.stage_scripts, vec!["users.sql"]);

    assert_eq!(report.applied_scripts, vec!["R__active_users.sql"]);
    assert_eq!(report.applied_count, 1);

    assert_eq!(backend.history.len(), 2);
    assert!(backend.history.iter().all(|entry| entry.success));
    assert_eq!(backend.history[1].script, "R__active_users.sql");

    let executed: Vec<&str> = backend.executed_sql.iter().map(String::as_str).collect();
    assert!(
        !executed.contains(&"CREATE TABLE users (id INTEGER);"),
        "the stage marker closes the versioned migration the stage already reflects"
    );
    assert!(
        executed.contains(&"CREATE VIEW active_users AS SELECT id FROM users;"),
        "the repeatable script runs on top of the stage"
    );

    let rerun = embedded
        .migrate(&mut backend)
        .expect("the rerun applies nothing");

    assert_eq!(rerun.applied_count, 0);
    assert!(rerun.cold_start.is_none());
    assert_eq!(backend.history.len(), 2);
}
#[test]
#[cfg(feature = "tokio")]
fn migrate_async_hands_the_backend_back() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("build tokio runtime");

    let (report, backend) = runtime
        .block_on(embedded().migrate_async(InMemoryBackend::new()))
        .expect("the blocking run succeeds");

    assert_eq!(report.applied_count, 2);
    assert_eq!(report.applied_scripts.len(), 2);
    assert_eq!(backend.history.len(), 2);
    assert!(backend.history.iter().all(|entry| entry.success));
}

#[test]
#[cfg(feature = "tokio")]
fn status_async_reports_the_applied_scripts() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("build tokio runtime");

    let embedded = embedded();
    let (_, backend) = runtime
        .block_on(embedded.migrate_async(InMemoryBackend::new()))
        .expect("the blocking run succeeds");

    let (statuses, backend) = runtime
        .block_on(embedded.status_async(backend))
        .expect("the blocking status read succeeds");

    assert_eq!(statuses.len(), 2);
    assert_eq!(
        statuses[0].status,
        crate::runner::MigrationStatusKind::Applied
    );
    assert_eq!(
        statuses[1].status,
        crate::runner::MigrationStatusKind::Applied
    );
    assert_eq!(backend.history.len(), 2);
}

#[test]
#[cfg(feature = "tokio")]
fn migrate_async_surfaces_a_backend_failure() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("build tokio runtime");

    let mut backend = InMemoryBackend::new();
    backend.failure_script = Some("BOOM".to_owned());
    let embedded =
        EmbeddedMigrations::new(&[("V1__broken.sql", "BOOM;")]).expect("valid migrations");

    let error = runtime
        .block_on(embedded.migrate_async(backend))
        .expect_err("the failing script aborts the run");

    assert!(matches!(error, Error::MigrationExecutionFailed { .. }));
}

#[test]
#[cfg(feature = "tokio")]
fn baseline_async_records_the_baseline_and_hands_the_backend_back() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("build tokio runtime");

    let version: MigrationVersion = "1.0.0".parse().expect("a valid version");

    let (entry, backend) = runtime
        .block_on(embedded().baseline_async(
            InMemoryBackend::new(),
            version.clone(),
            "legacy baseline",
        ))
        .expect("the blocking baseline succeeds");

    assert_eq!(entry.kind, MigrationKind::Baseline);
    assert_eq!(
        entry.version.as_ref().map(ToString::to_string),
        Some(version.to_string())
    );
    assert_eq!(backend.history.len(), 1);
    assert_eq!(backend.history[0].kind, MigrationKind::Baseline);
    assert!(backend.history[0].success);
}

#[test]
#[cfg(feature = "tokio")]
fn repair_async_purges_the_failed_entry_of_a_broken_script() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("build tokio runtime");

    // The failing run is driven on this plain test thread: the `*_async`
    // wrappers hand the backend back on success only, so the connection that
    // carries the failed entry has to come from the blocking path.
    let mut backend = InMemoryBackend::new();
    backend.failure_script = Some("BOOM".to_owned());
    let broken =
        EmbeddedMigrations::new(&[("V1__broken.sql", "BOOM;")]).expect("valid migrations");

    let error = broken
        .migrate(&mut backend)
        .expect_err("the failing script aborts the run");
    assert!(matches!(error, Error::MigrationExecutionFailed { .. }));
    assert_eq!(backend.history.len(), 1);
    assert!(!backend.history[0].success);

    backend.failure_script = None;
    let fixed =
        EmbeddedMigrations::new(&[("V1__broken.sql", "SELECT 1;")]).expect("valid migrations");

    let (report, backend) = runtime
        .block_on(fixed.repair_async(backend))
        .expect("the blocking repair succeeds");

    assert_eq!(report.removed_failed_entries, 1);
    assert!(report.updated_checksums.is_empty());
    assert!(
        backend.history.is_empty(),
        "the purge leaves a clean history behind"
    );
}

/// The `*_async` wrapper runs the guard-free core, so the blocking-pool thread
/// never trips the enforcement the synchronous entry points raise.
#[test]
#[cfg(feature = "strict-async")]
fn migrate_async_runs_the_guard_free_core_inside_a_runtime() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("build tokio runtime");

    let (report, backend) = runtime
        .block_on(embedded().migrate_async(InMemoryBackend::new()))
        .expect("the blocking run succeeds under `strict-async`");

    assert_eq!(report.applied_count, 2);
    assert_eq!(backend.history.len(), 2);
    assert!(backend.history.iter().all(|entry| entry.success));
}

/// The baseline wrapper drives the same guard-free core, so it stays usable
/// from a runtime thread even when the synchronous entry points refuse to run.
#[test]
#[cfg(feature = "strict-async")]
fn baseline_async_runs_the_guard_free_core_inside_a_runtime() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("build tokio runtime");

    let version: MigrationVersion = "1.0.0".parse().expect("a valid version");

    let (entry, backend) = runtime
        .block_on(embedded().baseline_async(InMemoryBackend::new(), version, "legacy baseline"))
        .expect("the blocking baseline succeeds under `strict-async`");

    assert_eq!(entry.kind, MigrationKind::Baseline);
    assert_eq!(backend.history.len(), 1);
    assert!(backend.history[0].success);
}
