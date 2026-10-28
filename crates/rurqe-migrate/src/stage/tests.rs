#[cfg(feature = "tokio")]
use super::cold_start_async;
use super::{
    cold_start, ColdStartReport, DeclarativeStage, StageMarker, StageScript, STAGE_SCRIPT,
};
use crate::catalog::SchemaCatalog;
use crate::checksum::Checksum;
use crate::error::Error;
use crate::history::{create_schema_history_table_sql, SchemaHistoryEntry};
use crate::migration::{Migration, MigrationKind};
use crate::runner::{InMemoryBackend, MigrationBackend};
use crate::version::MigrationVersion;
use std::path::PathBuf;

const ORDERS: &str = "CREATE TABLE orders (id INTEGER);";
const USERS: &str = "CREATE TABLE users (id INTEGER);";

fn version(text: &str) -> MigrationVersion {
    text.parse().expect("a valid version")
}

fn stage() -> DeclarativeStage {
    DeclarativeStage::new(vec![
        StageScript::new("orders.sql", ORDERS),
        StageScript::new("users.sql", USERS),
    ])
}

fn marker() -> StageMarker {
    StageMarker::from_catalog(version("3.2"), &SchemaCatalog::default())
}

fn stage_directory(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("rurqe-stage-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).expect("create the stage directory");
    path
}

#[test]
fn scripts_are_read_from_the_stage_directory() {
    let dir = stage_directory("load");
    std::fs::write(dir.join("users.sql"), USERS).expect("write the script");
    std::fs::write(dir.join("orders.sql"), ORDERS).expect("write the script");
    std::fs::write(dir.join("notes.txt"), "not a script").expect("write the note");

    let stage = DeclarativeStage::load(&dir)
        .expect("the stage is readable")
        .expect("the directory exists");
    let _ = std::fs::remove_dir_all(&dir);

    let names: Vec<&str> = stage.scripts().iter().map(StageScript::name).collect();
    assert_eq!(names, vec!["orders.sql", "users.sql"]);
    assert_eq!(stage.scripts()[0].sql(), ORDERS);
}

#[test]
fn a_missing_stage_directory_is_not_an_error() {
    let dir = stage_directory("absent");
    let _ = std::fs::remove_dir_all(&dir);

    assert_eq!(DeclarativeStage::load(&dir), Ok(None));
}

#[test]
fn scripts_are_applied_in_name_order() {
    let stage = DeclarativeStage::new(vec![
        StageScript::new("users.sql", USERS),
        StageScript::new("orders.sql", ORDERS),
    ]);

    let names: Vec<&str> = stage.scripts().iter().map(StageScript::name).collect();
    assert_eq!(names, vec!["orders.sql", "users.sql"]);
}

#[test]
fn the_stage_digest_follows_names_and_bodies() {
    let stage = stage();
    let reordered = DeclarativeStage::new(vec![
        StageScript::new("users.sql", USERS),
        StageScript::new("orders.sql", ORDERS),
    ]);
    let changed = DeclarativeStage::new(vec![StageScript::new(
        "users.sql",
        "CREATE TABLE users (id BIGINT);",
    )]);

    assert_eq!(reordered.checksum(), stage.checksum());
    assert_ne!(changed.checksum(), stage.checksum());
}

#[test]
fn cold_start_applies_the_stage_in_a_single_transaction() {
    let mut backend = InMemoryBackend::new();
    let catalog = SchemaCatalog::default();

    let report = cold_start(&mut backend, &stage(), &marker()).expect("the database is cold");

    assert_eq!(report.stage_scripts, vec!["orders.sql", "users.sql"]);
    assert_eq!(report.installed_rank, Some(1));

    assert_eq!(backend.executed_sql.len(), 3);
    assert_eq!(backend.executed_sql[0], create_schema_history_table_sql());
    assert_eq!(backend.executed_sql[1], ORDERS);
    assert_eq!(backend.executed_sql[2], USERS);

    assert_eq!(backend.history.len(), 1);
    let entry = &backend.history[0];
    assert_eq!(entry.installed_rank, 1);
    assert_eq!(
        entry.version.as_ref().map(ToString::to_string).as_deref(),
        Some("3.2")
    );
    assert_eq!(entry.description, StageMarker::DESCRIPTION);
    assert_eq!(entry.kind, MigrationKind::Baseline);
    assert_eq!(entry.script, STAGE_SCRIPT);
    assert_eq!(entry.checksum, Some(catalog.checksum()));
    assert!(entry.success);
}

#[test]
fn repeating_the_same_stage_is_a_no_op() {
    let mut backend = InMemoryBackend::new();
    cold_start(&mut backend, &stage(), &marker()).expect("the first run initialises the schema");

    let report =
        cold_start(&mut backend, &stage(), &marker()).expect("the recorded stage is recognised");

    assert_eq!(report, ColdStartReport::default());
    assert_eq!(backend.history.len(), 1);
    let ddl_runs = backend
        .executed_sql
        .iter()
        .filter(|sql| sql.as_str() == USERS || sql.as_str() == ORDERS)
        .count();
    assert_eq!(ddl_runs, 2);
}

#[test]
fn an_initialised_database_is_never_overwritten() {
    let mut backend = InMemoryBackend::new();
    let migration = Migration::parse("V1__init.sql", USERS).expect("a valid script name");
    backend.history.push(SchemaHistoryEntry::applied(
        1,
        &migration,
        "2024-01-01 00:00:00",
        1,
    ));

    let error = cold_start(&mut backend, &stage(), &marker())
        .expect_err("an initialised database is rejected");

    assert!(matches!(error, Error::BaselineAlreadyExists { .. }));
    assert_eq!(backend.history.len(), 1);
}

#[test]
fn a_diverged_stage_digest_is_rejected() {
    let mut backend = InMemoryBackend::new();
    cold_start(&mut backend, &stage(), &marker()).expect("the first run initialises the schema");

    let changed = StageMarker::new(
        version("3.2"),
        StageMarker::DESCRIPTION,
        Checksum::of(b"other"),
    );
    let error = cold_start(&mut backend, &stage(), &changed)
        .expect_err("a diverged stage cannot masquerade as a cold start");

    assert!(matches!(error, Error::InvalidStage { .. }));
}

#[test]
fn a_failing_script_aborts_the_cold_start() {
    let mut backend = InMemoryBackend::new();
    backend.failure_script = Some("BOOM".to_owned());
    let failing = DeclarativeStage::new(vec![
        StageScript::new("orders.sql", ORDERS),
        StageScript::new("users.sql", "BOOM;"),
    ]);

    let error = cold_start(&mut backend, &failing, &marker())
        .expect_err("the failing script aborts the cold start");

    assert!(matches!(error, Error::MigrationExecutionFailed { .. }));
    assert!(backend.history.is_empty());
    assert_eq!(backend.executed_sql.len(), 1);
    assert_eq!(backend.executed_sql[0], create_schema_history_table_sql());
}

#[test]
fn the_marker_carries_the_catalog_digest() {
    let catalog = SchemaCatalog::default();
    let marker = StageMarker::from_catalog(version("3.2"), &catalog);

    assert_eq!(marker.version().to_string(), "3.2");
    assert_eq!(marker.description(), StageMarker::DESCRIPTION);
    assert_eq!(marker.checksum(), catalog.checksum());
}

#[test]
fn cold_start_acquires_and_releases_the_backend_lock() {
    let mut backend = InMemoryBackend::new();
    assert!(!backend.locked);

    let report = cold_start(&mut backend, &stage(), &marker()).expect("cold start succeeds");
    assert_eq!(report.installed_rank, Some(1));
    assert!(!backend.locked);

    backend.acquire_lock().expect("lock acquired");
    let error = cold_start(&mut backend, &stage(), &marker())
        .expect_err("cold start must fail when backend is locked");
    assert!(matches!(error, Error::Backend(_)));
    backend.release_lock().expect("lock released");
    assert!(!backend.locked);

    let mut failing_backend = InMemoryBackend::new();
    failing_backend.failure_script = Some("BOOM".to_owned());
    let failing = DeclarativeStage::new(vec![StageScript::new("users.sql", "BOOM;")]);
    let error = cold_start(&mut failing_backend, &failing, &marker())
        .expect_err("cold start fails on bad script");
    assert!(matches!(error, Error::MigrationExecutionFailed { .. }));
    assert!(!failing_backend.locked);
}

#[test]
fn cold_start_outside_a_tokio_runtime_does_not_panic() {
    let mut backend = InMemoryBackend::new();
    let report = cold_start(&mut backend, &stage(), &marker())
        .expect("a cold start outside a runtime succeeds");

    assert_eq!(report.installed_rank, Some(1));
    assert!(!backend.locked);
}

/// The advisory stays a warning by default: the blocking entry point keeps
/// working inside a runtime, exactly like the migration lifecycle it belongs
/// to. Under `strict-async` the same call is refused instead, which the
/// `strict-async` test below covers.
#[test]
#[cfg(all(feature = "tokio", not(feature = "strict-async")))]
fn cold_start_inside_a_tokio_runtime_still_succeeds() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("build tokio runtime");
    rt.block_on(async {
        let mut backend = InMemoryBackend::new();
        let report = cold_start(&mut backend, &stage(), &marker())
            .expect("the blocking entry point stays callable inside a runtime");

        assert_eq!(report.installed_rank, Some(1));
        assert!(!backend.locked);
    });
}

#[test]
#[cfg(feature = "tokio")]
fn cold_start_async_initialises_the_database() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("build tokio runtime");

    let (report, backend) = runtime
        .block_on(cold_start_async(
            InMemoryBackend::new(),
            &stage(),
            &marker(),
        ))
        .expect("the blocking cold start succeeds");

    assert_eq!(report.stage_scripts, vec!["orders.sql", "users.sql"]);
    assert_eq!(report.installed_rank, Some(1));
    assert_eq!(backend.history.len(), 1);
    assert!(backend.history[0].success);
    assert_eq!(backend.history[0].script, STAGE_SCRIPT);
    assert!(
        !backend.locked,
        "the lock is released before the task returns"
    );
}

#[test]
#[cfg(feature = "tokio")]
fn cold_start_async_surfaces_a_diverged_stage_digest() {
    let mut backend = InMemoryBackend::new();
    cold_start(&mut backend, &stage(), &marker()).expect("the first run initialises the schema");

    let changed = StageMarker::new(
        version("3.2"),
        StageMarker::DESCRIPTION,
        Checksum::of(b"other"),
    );

    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("build tokio runtime");

    let error = runtime
        .block_on(cold_start_async(backend, &stage(), &changed))
        .expect_err("a diverged stage cannot masquerade as a cold start");

    assert!(matches!(error, Error::InvalidStage { .. }), "got {error:?}");
}

/// Under `strict-async` the synchronous entry point refuses to block a
/// Tokio-managed thread, while the `*_async` wrapper runs the same work on a
/// blocking-pool thread and therefore stays usable.
#[test]
#[cfg(feature = "strict-async")]
fn cold_start_is_refused_inside_a_runtime_under_strict_async() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("build tokio runtime");

    runtime.block_on(async {
        let mut backend = InMemoryBackend::new();
        let error = cold_start(&mut backend, &stage(), &marker())
            .expect_err("`strict-async` refuses to block the runtime");

        assert!(matches!(error, Error::Backend(_)), "got {error:?}");
        assert!(backend.history.is_empty(), "the backend stays untouched");
        assert!(
            backend.executed_sql.is_empty(),
            "nothing reached the backend"
        );
        assert!(!backend.locked);

        let (report, backend) = cold_start_async(InMemoryBackend::new(), &stage(), &marker())
            .await
            .expect("the sanctioned runtime path initialises the database");

        assert_eq!(report.stage_scripts, vec!["orders.sql", "users.sql"]);
        assert_eq!(report.installed_rank, Some(1));
        assert_eq!(backend.history.len(), 1);
        assert!(backend.history[0].success);
        assert!(!backend.locked);
    });
}

#[test]
fn a_stage_script_keeps_its_declared_transactionality() {
    assert!(StageScript::new("users.sql", USERS).is_transactional());
    assert!(!StageScript::new("users.notx.sql", USERS).is_transactional());
    assert!(!StageScript::new(
        "users.sql",
        "-- executeInTransaction=false\nCREATE TABLE users (id INTEGER);"
    )
    .is_transactional());
    assert!(!StageScript::new("users.sql", USERS)
        .with_transactional(false)
        .is_transactional());
}

#[test]
fn a_non_transactional_stage_script_runs_outside_a_transaction() {
    let mut backend = InMemoryBackend::new();
    let stage = DeclarativeStage::new(vec![
        StageScript::new("orders.sql", ORDERS),
        StageScript::new(
            "users.sql",
            "-- executeInTransaction=false\nCREATE INDEX CONCURRENTLY users_id ON users (id);",
        ),
    ]);

    let report = cold_start(&mut backend, &stage, &marker()).expect("the stage applies");

    assert_eq!(report.stage_scripts, vec!["orders.sql", "users.sql"]);
    assert_eq!(backend.non_transactional_sql.len(), 1);
    assert_eq!(backend.history.len(), 1);
    assert!(backend.history[0].success);
}

#[test]
fn a_failing_non_transactional_stage_script_leaves_the_interrupted_marker() {
    let mut backend = InMemoryBackend::new();
    backend.failure_script = Some("BOOM".to_owned());
    let stage = DeclarativeStage::new(vec![StageScript::new("users.notx.sql", "BOOM;")]);

    let error = cold_start(&mut backend, &stage, &marker())
        .expect_err("the failing script aborts the cold start");

    assert!(matches!(error, Error::MigrationExecutionFailed { .. }));
    assert_eq!(backend.history.len(), 1);
    assert_eq!(backend.history[0].script, STAGE_SCRIPT);
    assert!(
        !backend.history[0].success,
        "the in-progress marker records the interrupted initialisation"
    );
    assert!(!backend.locked);

    backend.failure_script = None;
    let retry = cold_start(&mut backend, &stage, &marker())
        .expect_err("the interrupted marker stops a second cold start");

    assert!(matches!(retry, Error::InvalidStage { .. }), "got {retry:?}");
    assert!(retry.to_string().contains("interrupted"), "got {retry}");
    assert_eq!(backend.history.len(), 1);
}

#[test]
fn the_in_progress_marker_of_a_non_transactional_stage_precedes_the_scripts() {
    let mut backend = InMemoryBackend::new();
    let stage = DeclarativeStage::new(vec![
        StageScript::new("orders.sql", ORDERS),
        StageScript::new("users.notx.sql", USERS),
    ]);

    let report = cold_start(&mut backend, &stage, &marker()).expect("the stage applies");

    assert_eq!(report.installed_rank, Some(1));
    assert_eq!(
        backend.history_len_at_execution,
        vec![1],
        "the in-progress marker exists before the non-transactional script runs"
    );
    assert_eq!(backend.history.len(), 1);
    assert!(backend.history[0].success);
    assert_eq!(backend.history[0].script, STAGE_SCRIPT);
}

#[test]
fn a_cold_start_that_cannot_record_the_applied_marker_stays_visible() {
    let mut backend = InMemoryBackend::new();
    backend.fail_success_commit = true;
    let stage = DeclarativeStage::new(vec![StageScript::new("users.notx.sql", USERS)]);

    let error = cold_start(&mut backend, &stage, &marker())
        .expect_err("the applied marker cannot be committed");

    assert!(matches!(error, Error::Backend(_)), "got {error:?}");
    assert_eq!(backend.history.len(), 1);
    assert!(
        !backend.history[0].success,
        "the in-progress marker survives the failed commit"
    );
    assert!(!backend.locked);

    backend.fail_success_commit = false;
    let retry = cold_start(&mut backend, &stage, &marker())
        .expect_err("the interrupted marker stops a second cold start");

    assert!(matches!(retry, Error::InvalidStage { .. }), "got {retry:?}");
}
