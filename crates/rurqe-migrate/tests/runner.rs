//! End-to-end integration tests for `Migrator` and transactional lifecycle operations.

use rurqe_migrate::{
    Error, InMemoryBackend, Migration, MigrationKind, MigrationStatusKind, MigrationVersion,
    Migrator,
};

#[test]
fn migrator_executes_versioned_then_repeatable_in_order() {
    let mut backend = InMemoryBackend::new();
    let migrator = Migrator::new();

    let m1 = Migration::parse("V2__add_orders.sql", "CREATE TABLE orders (id INT);").unwrap();
    let m2 = Migration::parse("V1__init.sql", "CREATE TABLE users (id INT);").unwrap();
    let r1 = Migration::parse("R__users_view.sql", "CREATE VIEW users_v AS SELECT * FROM users;").unwrap();

    let report = migrator
        .migrate(&mut backend, &[m1.clone(), r1.clone(), m2.clone()])
        .expect("migration succeeds");

    assert_eq!(report.applied_count, 3);
    assert_eq!(
        report.applied_scripts,
        vec![
            "V1__init.sql",
            "V2__add_orders.sql",
            "R__users_view.sql"
        ]
    );

    let history = &backend.history;
    assert_eq!(history.len(), 3);
    assert_eq!(history[0].installed_rank, 1);
    assert_eq!(history[0].script, "V1__init.sql");
    assert_eq!(history[1].installed_rank, 2);
    assert_eq!(history[1].script, "V2__add_orders.sql");
    assert_eq!(history[2].installed_rank, 3);
    assert_eq!(history[2].script, "R__users_view.sql");
}

#[test]
fn re_running_migrate_skips_already_applied_migrations() {
    let mut backend = InMemoryBackend::new();
    let migrator = Migrator::new();

    let m1 = Migration::parse("V1__init.sql", "CREATE TABLE users (id INT);").unwrap();
    let r1 = Migration::parse("R__users_view.sql", "CREATE VIEW users_v AS SELECT 1;").unwrap();

    migrator
        .migrate(&mut backend, &[m1.clone(), r1.clone()])
        .expect("first run");

    let report = migrator
        .migrate(&mut backend, &[m1.clone(), r1.clone()])
        .expect("second run");

    assert_eq!(report.applied_count, 0);
    assert!(report.applied_scripts.is_empty());
}

#[test]
fn repeatable_migration_re_executes_when_checksum_changes() {
    let mut backend = InMemoryBackend::new();
    let migrator = Migrator::new();

    let r_v1 = Migration::parse("R__view.sql", "CREATE VIEW v AS SELECT 1;").unwrap();
    migrator.migrate(&mut backend, &[r_v1]).expect("first run");

    let r_v2 = Migration::parse("R__view.sql", "CREATE VIEW v AS SELECT 2;").unwrap();
    let report = migrator.migrate(&mut backend, &[r_v2]).expect("second run");

    assert_eq!(report.applied_count, 1);
    assert_eq!(report.applied_scripts, vec!["R__view.sql"]);
    assert_eq!(backend.history.len(), 2);
    assert_eq!(backend.history[1].installed_rank, 2);
}

#[test]
fn retroactive_edits_trigger_checksum_mismatch() {
    let mut backend = InMemoryBackend::new();
    let migrator = Migrator::new();

    let m1 = Migration::parse("V1__init.sql", "CREATE TABLE users (id INT);").unwrap();
    migrator.migrate(&mut backend, &[m1]).expect("first run");

    let m1_tampered = Migration::parse("V1__init.sql", "CREATE TABLE users (id BIGINT);").unwrap();
    let err = migrator
        .migrate(&mut backend, &[m1_tampered])
        .expect_err("tampered script must fail");

    match err {
        Error::ChecksumMismatch { script, .. } => assert_eq!(script, "V1__init.sql"),
        other => panic!("expected ChecksumMismatch, got {other:?}"),
    }
}

#[test]
fn baseline_skips_earlier_versioned_migrations() {
    let mut backend = InMemoryBackend::new();
    let migrator = Migrator::new();

    let v1: MigrationVersion = "1.0.0".parse().unwrap();
    migrator
        .baseline(&mut backend, v1, "legacy baseline")
        .expect("baseline succeeds");

    assert_eq!(backend.history.len(), 1);
    assert_eq!(backend.history[0].kind, MigrationKind::Baseline);

    let m0 = Migration::parse("V0_9__legacy.sql", "CREATE TABLE legacy (id INT);").unwrap();
    let m1 = Migration::parse("V1_0_0__init.sql", "CREATE TABLE users (id INT);").unwrap();
    let m2 = Migration::parse("V2_0__orders.sql", "CREATE TABLE orders (id INT);").unwrap();

    let report = migrator
        .migrate(&mut backend, &[m0, m1, m2])
        .expect("migrate after baseline");

    assert_eq!(report.applied_count, 1);
    assert_eq!(report.applied_scripts, vec!["V2_0__orders.sql"]);
}

#[test]
fn repair_fixes_checksums_and_removes_failed_entries() {
    let mut backend = InMemoryBackend::new();
    let migrator = Migrator::new();

    let m1 = Migration::parse("V1__init.sql", "CREATE TABLE users (id INT);").unwrap();
    migrator.migrate(&mut backend, &[m1]).expect("init succeeds");

    // Manually inject a failed entry
    let m2 = Migration::parse("V2__broken.sql", "BROKEN SQL").unwrap();
    backend.history.push(rurqe_migrate::SchemaHistoryEntry::failed(
        2,
        &m2,
        "now",
        10,
    ));

    // Tamper m1
    let m1_new = Migration::parse("V1__init.sql", "CREATE TABLE users (id INT, email TEXT);").unwrap();

    let repair_report = migrator
        .repair(&mut backend, std::slice::from_ref(&m1_new))
        .expect("repair succeeds");

    assert_eq!(repair_report.removed_failed_entries, 1);
    assert_eq!(repair_report.updated_checksums, vec!["V1__init.sql"]);

    // After repair, validation succeeds
    assert_eq!(backend.history.len(), 1);
    assert_eq!(backend.history[0].checksum, Some(m1_new.checksum));
}

#[test]
fn status_reports_pending_applied_and_modified_states() {
    let mut backend = InMemoryBackend::new();
    let migrator = Migrator::new();

    let m1 = Migration::parse("V1__init.sql", "CREATE TABLE users (id INT);").unwrap();
    migrator.migrate(&mut backend, &[m1]).expect("first migration");

    let m1_tampered = Migration::parse("V1__init.sql", "CREATE TABLE users (id BIGINT);").unwrap();
    let m2 = Migration::parse("V2__orders.sql", "CREATE TABLE orders (id INT);").unwrap();

    let statuses = migrator
        .status(&mut backend, &[m1_tampered, m2])
        .expect("status check succeeds");

    assert_eq!(statuses.len(), 2);
    assert_eq!(statuses[0].script, "V1__init.sql");
    assert_eq!(statuses[0].status, MigrationStatusKind::Modified);
    assert_eq!(statuses[1].script, "V2__orders.sql");
    assert_eq!(statuses[1].status, MigrationStatusKind::Pending);
}

#[test]
fn failed_migration_triggers_rollback_and_prevents_subsequent_runs() {
    let mut backend = InMemoryBackend::new();
    backend.failure_script = Some("FAIL".to_owned());

    let migrator = Migrator::new();
    let m1 = Migration::parse("V1__ok.sql", "SELECT 1;").unwrap();
    let m2 = Migration::parse("V2__fail.sql", "SELECT FAIL;").unwrap();

    let err = migrator
        .migrate(&mut backend, &[m1.clone(), m2.clone()])
        .expect_err("execution must fail");

    match err {
        Error::MigrationExecutionFailed { script, .. } => assert_eq!(script, "V2__fail.sql"),
        other => panic!("expected MigrationExecutionFailed, got {other:?}"),
    }

    // Lock must be released even after migration failure
    assert!(!backend.locked);

    // m1 was committed, m2 was rolled back and recorded as failed in history
    assert_eq!(backend.history.len(), 2);
    assert_eq!(backend.history[0].script, "V1__ok.sql");
    assert!(backend.history[0].success);
    assert_eq!(backend.history[1].script, "V2__fail.sql");
    assert!(!backend.history[1].success);
    assert_eq!(backend.history[1].installed_rank, 2);

    // Subsequent migrate must be rejected due to the failed migration in history
    let re_err = migrator
        .migrate(&mut backend, &[m1.clone(), m2.clone()])
        .expect_err("subsequent migrate must be blocked by failed migration");

    match re_err {
        Error::FailedMigrationAlreadyApplied {
            script,
            installed_rank,
        } => {
            assert_eq!(script, "V2__fail.sql");
            assert_eq!(installed_rank, 2);
        }
        other => panic!("expected FailedMigrationAlreadyApplied, got {other:?}"),
    }

    // Repair removes the failed entry
    let repair_report = migrator
        .repair(&mut backend, std::slice::from_ref(&m1))
        .expect("repair succeeds");
    assert_eq!(repair_report.removed_failed_entries, 1);
    assert_eq!(backend.history.len(), 1);
    assert_eq!(backend.history[0].script, "V1__ok.sql");

    // Now migrate can succeed with a fixed migration
    backend.failure_script = None;
    let m2_fixed = Migration::parse("V2__fail.sql", "SELECT 2;").unwrap();
    let report = migrator
        .migrate(&mut backend, &[m1, m2_fixed])
        .expect("migrate after repair succeeds");
    assert_eq!(report.applied_count, 1);
    assert_eq!(report.applied_scripts, vec!["V2__fail.sql"]);
    assert_eq!(backend.history.len(), 2);
    assert!(backend.history[1].success);
}

#[test]
fn embedded_migrations_macro_and_lifecycle_execution() {
    use rurqe_migrate::{embed_migrations, EmbeddedMigrations};

    let embedded: EmbeddedMigrations = embed_migrations!(
        "V2__add_items.sql" => "CREATE TABLE items (id INT);",
        "V1__init.sql" => "CREATE TABLE users (id INT);",
        "R__users_view.sql" => "CREATE VIEW users_v AS SELECT * FROM users;",
    )
    .expect("valid embedded migrations");

    assert_eq!(embedded.len(), 3);
    assert!(!embedded.is_empty());
    // Validates that migrations are automatically ordered (versioned ascending, then repeatable)
    assert_eq!(embedded[0].script, "V1__init.sql");
    assert_eq!(embedded[1].script, "V2__add_items.sql");
    assert_eq!(embedded[2].script, "R__users_view.sql");

    let mut backend = InMemoryBackend::new();
    let report = embedded.migrate(&mut backend).expect("migration succeeds");

    assert_eq!(report.applied_count, 3);
    assert_eq!(
        report.applied_scripts,
        vec![
            "V1__init.sql",
            "V2__add_items.sql",
            "R__users_view.sql"
        ]
    );

    let statuses = embedded.status(&mut backend).expect("status succeeds");
    assert_eq!(statuses.len(), 3);
    assert!(statuses.iter().all(|s| s.status == MigrationStatusKind::Applied));
}

#[test]
fn embedded_migrations_tuple_syntax_and_deref_coercion() {
    use rurqe_migrate::embed_migrations;

    let embedded = embed_migrations!(
        ("V1__init.sql", "CREATE TABLE users (id INT);"),
        ("R__view.sql", "CREATE VIEW v AS SELECT 1;"),
    )
    .expect("valid migrations");

    let mut backend = InMemoryBackend::new();
    let migrator = Migrator::new();

    // Deref coercion into &[Migration]
    let report = migrator.migrate(&mut backend, &embedded).expect("migrate via deref");
    assert_eq!(report.applied_count, 2);
}

#[test]
fn embedded_migrations_rejects_invalid_script_names() {
    use rurqe_migrate::embed_migrations;

    let err = embed_migrations!(
        "invalid_name.sql" => "SELECT 1;",
    )
    .expect_err("must reject invalid script name");

    match err {
        Error::InvalidScriptName { script } => assert_eq!(script, "invalid_name.sql"),
        other => panic!("expected InvalidScriptName, got {other:?}"),
    }
}

#[test]
fn duplicate_versions_are_rejected_before_any_migration_runs() {
    let mut backend = InMemoryBackend::new();
    let migrator = Migrator::new();

    let a = Migration::parse("V1__a.sql", "SELECT 1;").unwrap();
    let b = Migration::parse("V1__b.sql", "SELECT 2;").unwrap();

    let err = migrator
        .migrate(&mut backend, &[a, b])
        .expect_err("duplicate versions must be rejected");

    match err {
        Error::DuplicateMigrationVersion {
            version,
            first,
            second,
        } => {
            assert_eq!(version, "1");
            assert_eq!(first, "V1__a.sql");
            assert_eq!(second, "V1__b.sql");
        }
        other => panic!("expected DuplicateMigrationVersion, got {other:?}"),
    }

    assert!(backend.history.is_empty());
}

#[test]
fn baseline_rejects_non_empty_history() {
    let mut backend = InMemoryBackend::new();
    let migrator = Migrator::new();

    let m1 = Migration::parse("V1__init.sql", "CREATE TABLE users (id INT);").unwrap();
    migrator.migrate(&mut backend, &[m1]).expect("first migration");

    let err = migrator
        .baseline(&mut backend, "2.0.0".parse().unwrap(), "too late")
        .expect_err("baseline on non-empty history must fail");

    match err {
        Error::BaselineAlreadyExists { version } => assert_eq!(version, "1"),
        other => panic!("expected BaselineAlreadyExists, got {other:?}"),
    }
}
