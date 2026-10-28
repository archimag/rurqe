//! Regression tests of the Golden Equation verification.

use super::*;
use rurqe_migrate::checksum::Checksum;
use rurqe_migrate::stage::StageScript;

use crate::drift::TableDiff;

const USERS: &str = "CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT NOT NULL);";

fn stage(scripts: &[(&str, &str)]) -> DeclarativeStage {
    DeclarativeStage::new(
        scripts
            .iter()
            .map(|(name, sql)| StageScript::new(*name, *sql))
            .collect(),
    )
}

fn tail(scripts: &[(&str, &str)]) -> Vec<Migration> {
    scripts
        .iter()
        .map(|(name, sql)| Migration::parse(name, sql).expect("a valid script name"))
        .collect()
}

#[test]
fn materialise_stage_introspects_catalog() {
    let stage = stage(&[("users.sql", USERS)]);
    let catalog = materialise_stage(&stage, ":memory:", crate::introspect::DEFAULT_SCHEMA)
        .expect("stage materialises in SQLite scratch");

    assert_eq!(catalog.tables.len(), 1);
    assert_eq!(catalog.tables[0].name, "users");
}

#[test]
fn matching_stage_and_tail_are_equivalent() {
    let stage = stage(&[("users.sql", USERS)]);
    let tail = tail(&[("V1__users.sql", USERS)]);

    let report = GoldenEquation::new(&stage, &tail)
        .verify(":memory:")
        .expect("the stage and the tail describe the same schema");

    assert!(report.is_equivalent());
    assert_eq!(report.stage_hash(), report.tail_hash());
    assert!(
        report.schema_notices.is_empty(),
        "a SQLite scratch holds one unnamed catalog, so no selection can be vacuous: {:?}",
        report.schema_notices
    );
}

#[test]
fn the_tail_must_create_every_stage_table() {
    let stage = stage(&[
        (
            "orders.sql",
            "CREATE TABLE orders (id INTEGER PRIMARY KEY);",
        ),
        ("users.sql", USERS),
    ]);
    let tail = tail(&[("V1__users.sql", USERS)]);

    let error = GoldenEquation::new(&stage, &tail)
        .verify(":memory:")
        .expect_err("`orders` is never created by the tail");

    match &error {
        StageError::Diverged(report) => {
            assert_eq!(report.diff.missing_tables, vec!["orders".to_owned()]);
        }
        other => panic!("expected Diverged, got {other:?}"),
    }
    assert!(error.to_string().contains("orders"));
    assert!(error.to_string().contains("golden equation"));
}

#[test]
fn the_tail_must_declare_every_stage_column() {
    let stage = stage(&[("users.sql", USERS)]);
    let tail = tail(&[(
        "V1__users.sql",
        "CREATE TABLE users (id INTEGER PRIMARY KEY);",
    )]);

    match GoldenEquation::new(&stage, &tail).verify(":memory:") {
        Err(StageError::Diverged(report)) => {
            assert_eq!(
                report.diff.table_diffs["users"].missing_columns,
                vec!["email".to_owned()]
            );
        }
        other => panic!("expected a diverged report, got {other:?}"),
    }
}

#[test]
fn differing_nullability_is_reported_as_an_altered_column() {
    let stage = stage(&[("users.sql", USERS)]);
    let tail = tail(&[(
        "V1__users.sql",
        "CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT);",
    )]);

    match GoldenEquation::new(&stage, &tail).verify(":memory:") {
        Err(StageError::Diverged(report)) => {
            assert_eq!(
                report.diff.table_diffs["users"].altered_columns,
                vec!["email".to_owned()]
            );
        }
        other => panic!("expected a diverged report, got {other:?}"),
    }
}

#[test]
fn the_base_stage_anchors_the_replayed_tail() {
    let base = stage(&[("users.sql", "CREATE TABLE users (id INTEGER PRIMARY KEY);")]);
    let tail = tail(&[(
        "V2__email.sql",
        "ALTER TABLE users ADD COLUMN email TEXT NOT NULL DEFAULT '';",
    )]);
    let stage = stage(&[(
        "users.sql",
        "CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT NOT NULL DEFAULT '');",
    )]);

    let report = GoldenEquation::new(&stage, &tail)
        .with_base(&base)
        .verify(":memory:")
        .expect("the base plus the tail reproduces the stage");

    assert!(report.is_equivalent());
    assert_eq!(report.stage_hash(), report.tail_hash());
}

#[test]
fn checkpoint_filters_outdated_tail_migrations() {
    let base = stage(&[("users.sql", "CREATE TABLE users (id INTEGER PRIMARY KEY);")]);
    let base_catalog = materialise_stage(&base, ":memory:", crate::introspect::DEFAULT_SCHEMA)
        .expect("base catalog");
    let cp = Checkpoint::new(
        "1.0".parse().expect("valid version"),
        base_catalog.checksum(),
    );

    // V1 would fail if applied on top of base because table users already exists
    let tail = tail(&[
        (
            "V1__users.sql",
            "CREATE TABLE users (id INTEGER PRIMARY KEY);",
        ),
        (
            "V2__email.sql",
            "ALTER TABLE users ADD COLUMN email TEXT NOT NULL DEFAULT '';",
        ),
        (
            "R__active_users.sql",
            "CREATE VIEW active_users AS SELECT id FROM users;",
        ),
    ]);
    // The repeatable script lives in the tail, so the stage has to declare
    // the view it creates as well.
    let stage = stage(&[
        (
            "users.sql",
            "CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT NOT NULL DEFAULT '');",
        ),
        (
            "views.sql",
            "CREATE VIEW active_users AS SELECT id FROM users;",
        ),
    ]);

    let report = GoldenEquation::new(&stage, &tail)
        .with_base(&base)
        .with_checkpoint(&cp)
        .verify(":memory:")
        .expect("checkpoint filters V1 and matches base hash");

    assert!(report.is_equivalent());
}

#[test]
fn base_stage_hash_mismatch_is_rejected() {
    let base = stage(&[("users.sql", "CREATE TABLE users (id INTEGER PRIMARY KEY);")]);
    let cp = Checkpoint::new(
        "1.0".parse().expect("valid version"),
        Checksum::of(b"wrong_hash"),
    );

    let tail = tail(&[(
        "V2__email.sql",
        "ALTER TABLE users ADD COLUMN email TEXT NOT NULL DEFAULT '';",
    )]);
    let stage = stage(&[(
        "users.sql",
        "CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT NOT NULL DEFAULT '');",
    )]);

    let error = GoldenEquation::new(&stage, &tail)
        .with_base(&base)
        .with_checkpoint(&cp)
        .verify(":memory:")
        .expect_err("mismatched base stage hash must be rejected");

    match &error {
        StageError::BaseStageHashMismatch { expected, actual } => {
            assert_eq!(expected, &Checksum::of(b"wrong_hash").to_hex());
            assert_ne!(expected, actual);
        }
        other => panic!("expected BaseStageHashMismatch, got {other:?}"),
    }
    assert!(error
        .to_string()
        .contains("base stage catalog hash mismatch"));
}

#[test]
fn the_boundary_is_replayed_once_into_the_tail_sandbox() {
    // The boundary DDL is guarded by nothing, so applying it a second time to
    // the same sandbox would fail: the anchored replay has to hand the very
    // sandbox that holds the boundary straight to the tail.
    let base = stage(&[("users.sql", "CREATE TABLE users (id INTEGER PRIMARY KEY);")]);
    let base_catalog = materialise_stage(&base, ":memory:", crate::introspect::DEFAULT_SCHEMA)
        .expect("the base stage materialises");
    let checkpoint = Checkpoint::new(
        "1.0".parse().expect("a valid version"),
        base_catalog.checksum(),
    );

    let tail = tail(&[(
        "V2__email.sql",
        "ALTER TABLE users ADD COLUMN email TEXT NOT NULL DEFAULT '';",
    )]);
    let stage = stage(&[(
        "users.sql",
        "CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT NOT NULL DEFAULT '');",
    )]);

    let report = GoldenEquation::new(&stage, &tail)
        .with_base(&base)
        .with_checkpoint(&checkpoint)
        .verify(":memory:")
        .expect("the boundary is replayed once and the tail continues in the same sandbox");

    assert!(report.is_equivalent());
}

#[test]
fn a_checkpoint_without_a_base_stage_is_rejected() {
    let base = stage(&[("users.sql", "CREATE TABLE users (id INTEGER PRIMARY KEY);")]);
    let base_catalog = materialise_stage(&base, ":memory:", crate::introspect::DEFAULT_SCHEMA)
        .expect("the base stage materialises");
    let checkpoint = Checkpoint::new(
        "1".parse().expect("a valid version"),
        base_catalog.checksum(),
    );

    let stage = stage(&[(
        "users.sql",
        "CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT NOT NULL);",
    )]);
    let tail = tail(&[(
        "V2__email.sql",
        "ALTER TABLE users ADD COLUMN email TEXT NOT NULL DEFAULT '';",
    )]);

    let error = GoldenEquation::new(&stage, &tail)
        .with_checkpoint(&checkpoint)
        .verify(":memory:")
        .expect_err("the boundary is not reproduced by an empty scratch database");

    assert!(
        matches!(error, StageError::BaseStageMissing { .. }),
        "got {error:?}"
    );
    assert!(error.to_string().contains("base stage"), "got {error}");
}

#[test]
fn an_empty_boundary_needs_no_base_stage() {
    // An empty scratch database reproduces an empty boundary on its own, so
    // requiring a snapshot for it would reject a valid empty cold start.
    let checkpoint = Checkpoint::new(
        "0".parse().expect("a valid version"),
        SchemaCatalog::default().checksum(),
    );

    let stage = stage(&[("users.sql", USERS)]);
    let tail = tail(&[("V1__users.sql", USERS)]);

    let report = GoldenEquation::new(&stage, &tail)
        .with_checkpoint(&checkpoint)
        .verify(":memory:")
        .expect("an empty boundary is reproduced by the empty scratch database");

    assert!(report.is_equivalent());
}

#[test]
fn a_tail_without_its_base_cannot_be_replayed() {
    let stage = stage(&[("users.sql", USERS)]);
    let tail = tail(&[(
        "V2__email.sql",
        "ALTER TABLE users ADD COLUMN email TEXT NOT NULL DEFAULT '';",
    )]);

    let error = GoldenEquation::new(&stage, &tail)
        .verify(":memory:")
        .expect_err("the tail alters a table nothing created");

    assert!(matches!(error, StageError::Script { .. }), "got {error:?}");
    assert!(error.to_string().contains("V2__email.sql"));
}

#[test]
fn a_repeatable_script_belongs_to_the_catalog_it_builds() {
    // The bindings avoid the names of the `stage` and `tail` helpers, because a
    // shadowing `let stage = stage(...)` would resolve the call to the value
    // already bound instead of the helper.
    let initial = stage(&[("users.sql", USERS)]);
    let tail = tail(&[
        ("V1__users.sql", USERS),
        (
            "R__active_users.sql",
            "CREATE VIEW active_users AS SELECT id FROM users;",
        ),
    ]);

    let error = GoldenEquation::new(&initial, &tail)
        .verify(":memory:")
        .expect_err("the stage never declares the view the tail creates");

    match &error {
        StageError::Diverged(report) => {
            assert_eq!(report.diff.extra_views, vec!["active_users".to_owned()]);
        }
        other => panic!("expected Diverged, got {other:?}"),
    }

    let declared = stage(&[
        ("users.sql", USERS),
        (
            "views.sql",
            "CREATE VIEW active_users AS SELECT id FROM users;",
        ),
    ]);

    let report = GoldenEquation::new(&declared, &tail)
        .verify(":memory:")
        .expect("the stage declares the view the tail creates");

    assert!(report.is_equivalent());
}

#[test]
fn a_failing_script_is_reported_with_its_name() {
    let stage = stage(&[("users.sql", "CREATE TABLE users (;")]);

    let error = GoldenEquation::new(&stage, &[])
        .verify(":memory:")
        .expect_err("the stage DDL is malformed");

    assert!(matches!(error, StageError::Script { .. }), "got {error:?}");
    assert!(error.to_string().contains("users.sql"));
}

#[test]
fn a_failed_replay_leaves_nothing_in_the_scratch_database() {
    let path = scratch_path("partial");
    let url = path.to_string_lossy().into_owned();

    // The first statement creates a table and the second one fails, so the
    // replay trips over DDL it has already committed to the scratch.
    let failing = stage(&[(
        "users.sql",
        "CREATE TABLE users (id INTEGER PRIMARY KEY);\n\
         CREATE TABLE users (id INTEGER PRIMARY KEY);",
    )]);

    let error = GoldenEquation::new(&failing, &[])
        .verify(&url)
        .expect_err("the stage cannot create the same table twice");

    assert!(matches!(error, StageError::Script { .. }), "got {error:?}");

    // The scratch database is empty again, so a sound stage still converges.
    let sound = stage(&[("users.sql", USERS)]);
    let tail = tail(&[("V1__users.sql", USERS)]);

    GoldenEquation::new(&sound, &tail)
        .verify(&url)
        .expect("the failed replay left no table behind");

    let _ = std::fs::remove_file(&path);
}

#[test]
fn index_divergences_are_reported_by_name() {
    let catalog = SchemaCatalog::new(Vec::new());
    let mut diff = SchemaDiff::default();
    diff.table_diffs.insert(
        "users".to_owned(),
        TableDiff {
            missing_indexes: vec!["users_email_idx".to_owned()],
            extra_indexes: vec!["users_legacy_idx".to_owned()],
            altered_indexes: vec!["users_name_idx".to_owned()],
            ..TableDiff::default()
        },
    );

    let report = GoldenReport {
        stage_catalog: catalog.clone(),
        tail_catalog: catalog,
        diff,
        schema_notices: Vec::new(),
    };

    assert!(!report.is_equivalent());

    let message = StageError::Diverged(Box::new(report)).to_string();
    assert!(message.contains("index"), "unexpected message: {message}");
    assert!(message.contains("users_email_idx"));
    assert!(message.contains("users_legacy_idx"));
    assert!(message.contains("users_name_idx"));
}

/// A unique, not-yet-created file scratch database for one test.
fn scratch_path(label: &str) -> std::path::PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let sequence = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    let mut path = std::env::temp_dir();
    path.push(format!(
        "rurqe-scratch-{label}-{}-{sequence}.db",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    path
}

#[test]
fn a_file_scratch_database_is_recreated_between_the_sides() {
    let path = scratch_path("reuse");
    let url = path.to_string_lossy().into_owned();
    let stage = stage(&[("users.sql", USERS)]);
    let tail = tail(&[("V1__users.sql", USERS)]);

    GoldenEquation::new(&stage, &tail)
        .verify(&url)
        .expect("both sides converge in the file scratch");

    GoldenEquation::new(&stage, &tail)
        .verify(&url)
        .expect("the deleted scratch is recreated by the next verification");

    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_file_scratch_database_keeps_no_state_between_the_sides() {
    let path = scratch_path("state");
    let url = path.to_string_lossy().into_owned();

    // `PRAGMA user_version` lives in the database header, so it outlives every
    // `DROP TABLE`, and the trigger disappears only together with its table. A
    // cleanup of `sqlite_master` would therefore carry both into the next side.
    let ddl = "PRAGMA user_version = 42;\n\
               CREATE TABLE users (id INTEGER PRIMARY KEY);\n\
               CREATE TRIGGER users_touch AFTER INSERT ON users BEGIN SELECT 1; END;";
    let stage = stage(&[("schema.sql", ddl)]);
    let tail = tail(&[("V1__schema.sql", ddl)]);

    GoldenEquation::new(&stage, &tail)
        .verify(&url)
        .expect("each side starts from a scratch that holds nothing");

    assert!(
        !path.exists(),
        "the file-backed scratch database is deleted after the verification"
    );
}

#[test]
fn a_file_scratch_database_resets_with_foreign_keys_enabled() {
    let path = scratch_path("fk");
    let url = path.to_string_lossy().into_owned();
    let ddl = "PRAGMA foreign_keys = ON;\n\
               CREATE TABLE customers (id INTEGER PRIMARY KEY);\n\
               CREATE TABLE orders (id INTEGER PRIMARY KEY, customer_id INTEGER REFERENCES customers(id));";
    let stage = stage(&[("schema.sql", ddl)]);
    let tail = tail(&[("V1__schema.sql", ddl)]);

    GoldenEquation::new(&stage, &tail)
        .verify(&url)
        .expect("both sides converge with foreign keys enabled");

    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_dirty_scratch_database_is_rejected() {
    let path = scratch_path("dirty");
    {
        let connection = rusqlite::Connection::open(&path).expect("open the scratch");
        connection
            .execute_batch("CREATE TABLE leftover (id INTEGER PRIMARY KEY);")
            .expect("seed the scratch");
    }

    let url = path.to_string_lossy().into_owned();
    let stage = stage(&[("users.sql", USERS)]);
    let tail: Vec<Migration> = Vec::new();

    let error = GoldenEquation::new(&stage, &tail)
        .verify(&url)
        .expect_err("the scratch database is not empty");

    assert!(matches!(error, StageError::Sandbox { .. }), "got {error:?}");
    assert!(error.to_string().contains("leftover"), "got {error}");

    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_non_transactional_stage_script_is_materialised() {
    let stage = DeclarativeStage::new(vec![
        StageScript::new("users.sql", USERS).with_transactional(false)
    ]);

    let catalog = materialise_stage(&stage, ":memory:", crate::introspect::DEFAULT_SCHEMA)
        .expect("the stage materialises in a scratch");

    assert_eq!(catalog.tables.len(), 1);
    assert_eq!(catalog.tables[0].name, "users");
}

/// SQL whose first statement succeeds and whose second one always fails.
#[cfg(unix)]
const HALF_BROKEN: &str = "CREATE TABLE users (id INTEGER PRIMARY KEY);\n\
                           CREATE TABLE users (id INTEGER PRIMARY KEY);";

/// A file-backed scratch database the operating system refuses to remove.
///
/// Only a platform with Unix directory permissions can deny the removal of a file
/// that stays readable and writable itself, so the scratch lives in a directory
/// that lost the write permission its entries are removed through. The replay of
/// [`HALF_BROKEN`] fails on its own, and the discard then cannot unlink the file
/// that replay left behind: exactly the pair of failures a stranded scratch has to
/// report.
///
/// A process that writes through a withdrawn permission — a superuser, and
/// therefore a container running as root — cannot be made to hit the second
/// failure, so [`new`](Self::new) reports it and the caller stands down instead of
/// asserting on an unreachable branch.
#[cfg(unix)]
struct LockedScratch {
    /// The directory holding the scratch database.
    directory: std::path::PathBuf,
    /// The database file the verification is pointed at.
    path: std::path::PathBuf,
}

#[cfg(unix)]
impl LockedScratch {
    /// Creates the empty scratch database, or reports that it cannot be locked.
    fn new(label: &str) -> Option<Self> {
        use std::os::unix::fs::PermissionsExt;

        let mut directory = std::env::temp_dir();
        directory.push(format!("rurqe-locked-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir(&directory).expect("create the scratch directory");

        // Opening the file is what creates it, and a zero-length database is a
        // valid empty one, so the verification still passes its emptiness gate.
        let path = directory.join("scratch.db");
        let created = rusqlite::Connection::open(&path).expect("create the empty scratch database");
        drop(created);

        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o500))
            .expect("withdraw the write permission of the scratch directory");

        let probe = directory.join(".permission-probe");
        if std::fs::write(&probe, "probe").is_ok() {
            let _ = std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700));
            let _ = std::fs::remove_dir_all(&directory);
            return None;
        }

        Some(Self { directory, path })
    }

    /// The location of the scratch database, as the URL the verification opens.
    fn location(&self) -> String {
        self.path.to_string_lossy().into_owned()
    }

    /// Restores the directory and removes the scratch it held.
    fn release(self) {
        use std::os::unix::fs::PermissionsExt;

        let _ = std::fs::set_permissions(&self.directory, std::fs::Permissions::from_mode(0o700));
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

#[cfg(unix)]
#[test]
fn a_stage_side_that_cannot_be_discarded_reports_both_failures() {
    let scratch = match LockedScratch::new("discard-stage") {
        Some(scratch) => scratch,
        // A superuser writes through the withdrawn permission, so the discard
        // cannot be made to fail and the regression stands down.
        None => return,
    };
    let location = scratch.location();

    let failing = stage(&[("users.sql", HALF_BROKEN)]);

    let error = materialise_stage(&failing, &location, crate::introspect::DEFAULT_SCHEMA)
        .expect_err("the stage cannot write into a scratch it cannot journal");

    assert!(
        matches!(error, StageError::DiscardFailed { .. }),
        "the discard failure has to reach the caller, got {error:?}"
    );

    let message = error.to_string();
    assert!(
        message.contains("users.sql"),
        "the replay cause is lost: {message}"
    );
    assert!(
        message.contains("failed to delete"),
        "the discard cause is lost: {message}"
    );
    assert!(
        message.contains("scratch.db"),
        "the leftover scratch path is lost: {message}"
    );

    scratch.release();
}

#[cfg(unix)]
#[test]
fn a_tail_side_that_cannot_be_discarded_reports_both_failures() {
    let scratch = match LockedScratch::new("discard-tail") {
        Some(scratch) => scratch,
        // A superuser writes through the withdrawn permission, so the discard
        // cannot be made to fail and the regression stands down.
        None => return,
    };
    let location = scratch.location();

    let declared = stage(&[("users.sql", USERS)]);
    let broken_tail = tail(&[("V1__users.sql", HALF_BROKEN)]);

    let error = GoldenEquation::new(&declared, &broken_tail)
        .verify(&location)
        .expect_err("the tail cannot write into a scratch it cannot journal");

    assert!(
        matches!(error, StageError::DiscardFailed { .. }),
        "the discard failure has to reach the caller, got {error:?}"
    );

    let message = error.to_string();
    assert!(
        message.contains("V1__users.sql"),
        "the replay cause is lost: {message}"
    );
    assert!(
        message.contains("failed to delete"),
        "the discard cause is lost: {message}"
    );
    assert!(
        message.contains("scratch.db"),
        "the leftover scratch path is lost: {message}"
    );

    scratch.release();
}
