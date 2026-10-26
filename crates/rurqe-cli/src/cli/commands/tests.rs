use std::path::{Path, PathBuf};
use std::time::Duration;

use rurqe_migrate::checkpoint::Checkpoint;
use rurqe_migrate::runner::MigrationBackend;
use rurqe_migrate::version::MigrationVersion;
use rurqe_migrate::{
    Checksum, DeclarativeStage, InMemoryBackend, SchemaCatalog, StageScript, STAGE_SCRIPT,
};

use crate::config::CodegenConfig;
use crate::driver::SqliteMigrationBackend;
use crate::introspect::DEFAULT_SCHEMA;
use crate::stage::materialise_stage;
use crate::stage::postgres::{current_owner_tag, ScratchAdminSession, ScratchCatalogEntry};

use super::stage::lock::{simulate_section_holder, CheckpointLock};
use super::stage::{
    cleanup_scratch_catalog, flush_boundary_parent, move_failure, recover_interrupted_commit,
    verify_stage, write_staged_boundary,
};
use super::{
    commit_stage, initialise_stage, prune_migrations, schema_of, sweep_stale_temporaries, write_file,
    STALE_TEMPORARY_AGE,
};

fn workspace(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("rurqe-commands-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).expect("create the workspace");
    path
}

fn version(text: &str) -> MigrationVersion {
    text.parse().expect("a valid version")
}

fn scripts(dir: &Path, names: &[&str]) {
    std::fs::create_dir_all(dir).expect("create the migration directory");
    for name in names {
        std::fs::write(dir.join(name), "SELECT 1;").expect("write the script");
    }
}

fn checkpoint_file(path: &Path, version: &str) {
    std::fs::write(
        path,
        Checkpoint::new(
            version.parse().expect("a valid version"),
            Checksum::of(b"stage"),
        )
        .render(),
    )
    .expect("write the checkpoint");
}

/// Applies `sql` to the SQLite database at `path`, creating the file when absent.
fn apply_live_ddl(path: &Path, sql: &str) {
    let connection = rusqlite::Connection::open(path).expect("open the live database");
    connection.execute_batch(sql).expect("apply the live DDL");
}

/// The entry names of `dir`, sorted alphabetically.
fn directory_names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .expect("read the directory")
        .map(|entry| {
            entry
                .expect("a directory entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    names
}

/// Backdates `path`, so a sweep of staged artifacts sees it as abandoned.
fn backdate(path: &Path, age: Duration) {
    let modified = std::time::SystemTime::now()
        .checked_sub(age)
        .expect("a representable modification time");
    // A directory rejects a write handle but accepts the timestamp update.
    let handle = std::fs::File::options()
        .write(true)
        .open(path)
        .or_else(|_| std::fs::File::open(path))
        .expect("open the planted artifact");

    handle
        .set_times(std::fs::FileTimes::new().set_modified(modified))
        .expect("backdate the planted artifact");
}

#[test]
fn the_stage_is_applied_from_the_declarative_directory() {
    let dir = workspace("stage-init");
    let stage_dir = dir.join("schema");
    std::fs::create_dir_all(&stage_dir).expect("create the stage directory");
    std::fs::write(
        stage_dir.join("users.sql"),
        "CREATE TABLE users (id INTEGER);",
    )
    .expect("write the stage script");

    let mut backend = InMemoryBackend::new();
    initialise_stage(&mut backend, &stage_dir, version("3.2"), "stage")
        .expect("the cold start succeeds");

    assert_eq!(backend.history.len(), 1);
    assert_eq!(backend.history[0].script, STAGE_SCRIPT);
    assert_eq!(backend.executed_sql.len(), 2);
}

#[test]
fn the_stage_marker_carries_the_canonical_catalog_hash() {
    let dir = workspace("stage-hash");
    let stage_dir = dir.join("schema");
    std::fs::create_dir_all(&stage_dir).expect("create the stage directory");
    std::fs::write(
        stage_dir.join("users.sql"),
        "CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT NOT NULL);",
    )
    .expect("write the stage script");

    // The marker is written by the backend that really applies the stage, so its
    // digest is read from the catalog that backend introspects.
    let connection = rusqlite::Connection::open_in_memory().expect("open the in-memory database");
    let mut backend = SqliteMigrationBackend::new(connection);
    initialise_stage(&mut backend, &stage_dir, version("3.2"), "stage")
        .expect("the cold start succeeds");

    let stage = DeclarativeStage::load(&stage_dir)
        .expect("the stage is readable")
        .expect("the stage directory exists");
    let catalog = materialise_stage(&stage, ":memory:", DEFAULT_SCHEMA)
        .expect("the stage materialises");

    // The marker anchors the canonical catalog digest the checkpoint compares
    // against, not the raw DDL digest of the stage directory.
    assert_ne!(catalog.checksum(), stage.checksum());
    let history = backend.read_history().expect("the history is readable");
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].checksum, Some(catalog.checksum()));
}

#[test]
fn committing_the_stage_anchors_the_checkpoint() {
    let dir = workspace("stage-commit");
    let stage_dir = dir.join("schema");
    std::fs::create_dir_all(&stage_dir).expect("create the stage directory");
    let lock_path = dir.join("schema.lock");
    let checkpoint_path = dir.join("checkpoint.toml");

    commit_stage(
        version("3.2"),
        ":memory:",
        DEFAULT_SCHEMA,
        &stage_dir,
        &lock_path,
        &checkpoint_path,
    )
    .expect("an empty database commits cleanly");

    let catalog =
        SchemaCatalog::from_lock(&std::fs::read_to_string(&lock_path).expect("read the lock"))
            .expect("the lock is canonical");
    let checkpoint = Checkpoint::load(&checkpoint_path)
        .expect("the checkpoint is readable")
        .expect("the checkpoint exists");

    assert_eq!(checkpoint.pruned_up_to_version().to_string(), "3.2");
    assert_eq!(checkpoint.schema_hash(), catalog.checksum());
}

#[test]
fn committing_the_stage_snapshots_the_boundary_verbatim() {
    let dir = workspace("stage-snapshot");
    let stage_dir = dir.join("schema");
    std::fs::create_dir_all(&stage_dir).expect("create the stage directory");
    let users = "CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT NOT NULL);";
    std::fs::write(stage_dir.join("users.sql"), users).expect("write the stage script");

    // The live database holds exactly what the declarative stage declares, so the
    // parity gate lets the commit seal an agreeing pair of descriptions.
    let database = dir.join("app.db");
    apply_live_ddl(&database, users);

    let lock_path = dir.join("schema.lock");
    let checkpoint_path = dir.join("checkpoint.toml");

    commit_stage(
        version("3.2"),
        &database.to_string_lossy(),
        DEFAULT_SCHEMA,
        &stage_dir,
        &lock_path,
        &checkpoint_path,
    )
    .expect("the boundary is committed together with its snapshot");

    let snapshot = dir.join("checkpoint-stage");
    assert_eq!(
        std::fs::read_to_string(snapshot.join("users.sql")).expect("read the snapshot"),
        users
    );

    // A later commit replaces the snapshot, so a script the new boundary
    // dropped never lingers next to the marker. The live database follows the
    // stage, which keeps the parity gate satisfied.
    std::fs::remove_file(stage_dir.join("users.sql")).expect("drop the script");
    std::fs::write(stage_dir.join("orders.sql"), "CREATE TABLE orders (id INTEGER);")
        .expect("write the new script");
    apply_live_ddl(&database, "DROP TABLE users; CREATE TABLE orders (id INTEGER);");

    commit_stage(
        version("4"),
        &database.to_string_lossy(),
        DEFAULT_SCHEMA,
        &stage_dir,
        &lock_path,
        &checkpoint_path,
    )
    .expect("the boundary is re-committed");

    let mut names: Vec<String> = std::fs::read_dir(&snapshot)
        .expect("read the snapshot directory")
        .map(|entry| {
            entry
                .expect("a directory entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();

    assert_eq!(names, vec!["orders.sql".to_owned()]);

    // The staging directory and the backup of the replaced boundary are gone
    // again, so the swap leaves nothing but the committed snapshot behind.
    let leftovers: Vec<String> = std::fs::read_dir(&dir)
        .expect("read the workspace")
        .map(|entry| {
            entry
                .expect("a directory entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .filter(|name| name.starts_with('.'))
        .collect();

    assert!(leftovers.is_empty(), "leftover artifacts: {leftovers:?}");
}

#[test]
fn the_committed_boundary_is_never_rewound() {
    let dir = workspace("stage-rewind");
    let stage_dir = dir.join("schema");
    std::fs::create_dir_all(&stage_dir).expect("create the stage directory");
    let lock_path = dir.join("schema.lock");
    let checkpoint_path = dir.join("checkpoint.toml");
    commit_stage(
        version("5"),
        ":memory:",
        DEFAULT_SCHEMA,
        &stage_dir,
        &lock_path,
        &checkpoint_path,
    )
    .expect("the first commit succeeds");

    let error = commit_stage(
        version("3"),
        ":memory:",
        DEFAULT_SCHEMA,
        &stage_dir,
        &lock_path,
        &checkpoint_path,
    )
    .expect_err("the boundary cannot move backwards");

    assert!(error.to_string().contains("re-open"));

    // The refusal happens before the swap, so the staged snapshot the preparation
    // assembled is discarded instead of lingering beside the boundary it never
    // became.
    let leftovers: Vec<String> = directory_names(&dir)
        .into_iter()
        .filter(|name| name.starts_with('.'))
        .collect();
    assert!(leftovers.is_empty(), "leftover artifacts: {leftovers:?}");
}

#[test]
fn a_stage_that_drifted_from_the_database_is_never_committed() {
    let dir = workspace("stage-drift");
    let stage_dir = dir.join("schema");
    std::fs::create_dir_all(&stage_dir).expect("create the stage directory");
    // The declarative stage declares a column the live database never received,
    // which is exactly the drift the stage lifecycle exists to catch.
    std::fs::write(
        stage_dir.join("users.sql"),
        "CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT NOT NULL);",
    )
    .expect("write the stage script");

    let database = dir.join("app.db");
    apply_live_ddl(&database, "CREATE TABLE users (id INTEGER PRIMARY KEY);");

    let lock_path = dir.join("schema.lock");
    let checkpoint_path = dir.join("checkpoint.toml");

    let error = commit_stage(
        version("3.2"),
        &database.to_string_lossy(),
        DEFAULT_SCHEMA,
        &stage_dir,
        &lock_path,
        &checkpoint_path,
    )
    .expect_err("the drifted stage cannot be sealed into a checkpoint");

    let message = error.to_string();
    assert!(
        message.contains("does not describe the database"),
        "the diagnostic names the violated parity, got {message}"
    );
    assert!(
        message.contains("email"),
        "the diagnostic names the drifted column, got {message}"
    );
    assert!(
        message.contains("re-run `stage commit`"),
        "the diagnostic names the remedy, got {message}"
    );

    // The gate stands in front of every artifact, so a rejected commit leaves no
    // half-described triple behind: no marker, no snapshot, and no lock. The gate
    // also runs before the checkpoint section is taken, so it creates no entry a
    // later command would have to wait behind.
    assert!(
        !checkpoint_path.exists(),
        "a rejected commit writes no checkpoint"
    );
    assert!(
        !dir.join("checkpoint-stage").exists(),
        "a rejected commit writes no boundary snapshot"
    );
    assert!(!lock_path.exists(), "a rejected commit writes no lock");
    assert!(
        !dir.join("checkpoint.lock").exists(),
        "a rejected commit enters no section and leaves no lock entry"
    );
}

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

/// Whether the operating system enforces directory permissions for this process.
///
/// A superuser bypasses them, so a flush cannot be made to fail by withdrawing
/// read permission from a directory, and the regression below stands down.
#[cfg(unix)]
fn permissions_are_enforced(dir: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    let probe = dir.join(".permission-probe");
    if std::fs::create_dir(&probe).is_err() {
        return true;
    }

    let restricted = std::fs::Permissions::from_mode(0o000);
    let enforced = std::fs::set_permissions(&probe, restricted)
        .map(|_| std::fs::File::open(&probe).is_err())
        .unwrap_or(true);
    let _ = std::fs::remove_dir(&probe);

    enforced
}

#[cfg(unix)]
#[test]
fn a_failed_flush_after_the_rename_fails_the_write() {
    use std::os::unix::fs::PermissionsExt;

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
fn a_commit_that_cannot_take_the_section_writes_no_artifact() {
    let dir = workspace("commit-partial");
    let stage_dir = dir.join("schema");
    std::fs::create_dir_all(&stage_dir).expect("create the stage directory");
    let lock_path = dir.join("schema.lock");
    let blocker = dir.join("blocker");
    std::fs::write(&blocker, "not a directory").expect("write the blocking file");
    let checkpoint_path = blocker.join("checkpoint.toml");

    // The section guards every artifact write of the commit — the lock, the
    // boundary, and the marker — so a commit whose checkpoint directory cannot
    // even be prepared for the lock entry fails before the first one: nothing
    // is left behind for a later commit or a verification to disagree with.
    let error = commit_stage(
        version("3.2"),
        ":memory:",
        DEFAULT_SCHEMA,
        &stage_dir,
        &lock_path,
        &checkpoint_path,
    )
    .expect_err("a file cannot hold the checkpoint artifacts");

    assert!(error.to_string().contains("blocker"), "{error}");
    assert!(
        !lock_path.exists(),
        "a commit that never entered its section writes no lock"
    );
    assert!(
        !checkpoint_path.exists(),
        "a commit that never entered its section writes no checkpoint"
    );
    assert!(
        !blocker.join("checkpoint-stage").exists(),
        "a commit that never entered its section writes no boundary snapshot"
    );
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

#[test]
fn verifying_an_interrupted_commit_restores_the_parked_boundary() {
    let dir = workspace("verify-restore");
    let stage_dir = dir.join("schema");
    std::fs::create_dir_all(&stage_dir).expect("create the stage directory");
    let users = "CREATE TABLE users (id INTEGER);";
    std::fs::write(stage_dir.join("users.sql"), users).expect("write the stage script");
    let migrations_dir = dir.join("migrations");
    std::fs::create_dir_all(&migrations_dir).expect("create the migrations directory");

    // The marker anchors the parked boundary, which a killed swap left beside
    // no live boundary.
    let stage = DeclarativeStage::load(&stage_dir)
        .expect("the stage is readable")
        .expect("the stage directory exists");
    let catalog = materialise_stage(&stage, ":memory:", DEFAULT_SCHEMA)
        .expect("the stage materialises");
    let checkpoint_path = dir.join("checkpoint.toml");
    std::fs::write(
        &checkpoint_path,
        Checkpoint::new(version("1"), catalog.checksum()).render(),
    )
    .expect("write the checkpoint");

    let backup = dir.join(".checkpoint-stage.4242.11.previous");
    std::fs::create_dir_all(&backup).expect("park the committed boundary");
    std::fs::write(backup.join("users.sql"), users).expect("write the parked snapshot");

    // The verification repairs the interrupted commit and then finds the
    // boundary and its marker in agreement.
    verify_stage(
        &stage_dir,
        None,
        &migrations_dir,
        &checkpoint_path,
        DEFAULT_SCHEMA,
        ":memory:",
    )
    .expect("the repaired checkpoint verifies");

    let boundary = dir.join("checkpoint-stage");
    assert_eq!(
        std::fs::read_to_string(boundary.join("users.sql")).expect("read the restored snapshot"),
        users
    );
    assert!(!backup.exists(), "the parked snapshot was renamed into place");
}

#[test]
fn a_concurrent_command_never_preempts_a_live_writer() {
    let dir = workspace("concurrent-live-writer");
    let stage_dir = dir.join("schema");
    std::fs::create_dir_all(&stage_dir).expect("create the stage directory");
    let users = "CREATE TABLE users (id INTEGER);";
    std::fs::write(stage_dir.join("users.sql"), users).expect("write the stage script");
    let migrations_dir = dir.join("migrations");
    std::fs::create_dir_all(&migrations_dir).expect("create the migrations directory");

    // The marker anchors the boundary the database already stands on, so a
    // verification that reaches the equation finds the pair consistent.
    let stage = DeclarativeStage::load(&stage_dir)
        .expect("the stage is readable")
        .expect("the stage directory exists");
    let catalog =
        materialise_stage(&stage, ":memory:", DEFAULT_SCHEMA).expect("the stage materialises");
    let checkpoint_path = dir.join("checkpoint.toml");
    let marker = Checkpoint::new(version("1"), catalog.checksum()).render();
    std::fs::write(&checkpoint_path, &marker).expect("write the checkpoint");
    let boundary = dir.join("checkpoint-stage");
    std::fs::create_dir_all(&boundary).expect("install the boundary");
    std::fs::write(boundary.join("users.sql"), users).expect("write the boundary snapshot");

    // The writer is between the steps of its swap: its previous marker and
    // boundary are parked under its own scope, and the kernel keeps the section
    // for it alone until its description closes.
    let marker_backup = dir.join(".checkpoint.toml.1.2.previous");
    std::fs::write(
        &marker_backup,
        Checkpoint::new(version("1"), Checksum::of(b"stage")).render(),
    )
    .expect("park the previous marker");
    let boundary_backup = dir.join(".checkpoint-stage.1.2.previous");
    std::fs::create_dir_all(&boundary_backup).expect("park the previous boundary");
    std::fs::write(
        boundary_backup.join("older.sql"),
        "CREATE TABLE older (id INTEGER);",
    )
    .expect("write the parked snapshot");

    let holder = simulate_section_holder(&checkpoint_path, 1, 2).expect("a held section");

    // Negative half: the verification refuses, names the holder, and touches
    // nothing — neither the pair the writer parked nor the live pair beside it.
    let error = verify_stage(
        &stage_dir,
        None,
        &migrations_dir,
        &checkpoint_path,
        DEFAULT_SCHEMA,
        ":memory:",
    )
    .expect_err("a live writer keeps its section");

    let message = error.to_string();
    assert!(message.contains("pid 1"), "got {message}");
    assert!(message.contains("nonce 2"), "got {message}");
    assert!(
        message.contains("never preempts"),
        "the refusal names the kernel as the arbiter, got {message}"
    );
    assert!(
        !message.contains("repaired an interrupted commit"),
        "the refusal runs no repair, got {message}"
    );

    // The in-flight writer's parked pair is the only copy of the state its
    // section owns, and no verification touches it while that writer lives.
    assert!(marker_backup.exists(), "the parked marker is untouched");
    assert!(
        boundary_backup.exists(),
        "the parked boundary is untouched"
    );
    assert_eq!(
        std::fs::read_to_string(&checkpoint_path).expect("read the live marker"),
        marker,
        "the live marker is untouched"
    );
    assert_eq!(
        directory_names(&boundary),
        vec!["users.sql".to_owned()],
        "the live boundary is untouched"
    );

    // Positive half: the kernel releases the section the moment the writer's
    // description closes, so the same verification converges and discards the
    // parked pair as the debris of a writer that is provably gone.
    drop(holder);

    verify_stage(
        &stage_dir,
        None,
        &migrations_dir,
        &checkpoint_path,
        DEFAULT_SCHEMA,
        ":memory:",
    )
    .expect("the released section verifies");

    assert!(!marker_backup.exists(), "the parked marker is discarded");
    assert!(
        !boundary_backup.exists(),
        "the parked boundary is discarded"
    );
    assert_eq!(
        directory_names(&boundary),
        vec!["users.sql".to_owned()],
        "the live boundary survives the repair"
    );
    assert_eq!(
        std::fs::read_to_string(boundary.join("users.sql")).expect("read the live snapshot"),
        users,
        "the live snapshot keeps its declarative script"
    );
}

/// Writes the lock entry naming `pid`/`nonce` with a heartbeat `age` old.
fn lock_entry(dir: &Path, pid: u32, nonce: u128, age: Duration) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("a clock after the epoch")
        .as_nanos();

    std::fs::write(
        dir.join("checkpoint.lock"),
        format!(
            "pid={pid}\nnonce={nonce}\nheartbeat={}\n",
            now.saturating_sub(age.as_nanos())
        ),
    )
    .expect("write the lock entry");
}

#[test]
fn the_parked_copies_of_a_live_foreign_writer_are_never_repaired() {
    let dir = workspace("commit-foreign");
    let checkpoint_path = dir.join("checkpoint.toml");

    // The entry records the writer pid 1, so these artifacts belong to a section
    // this process does not hold: the pair that writer parked may still be the only
    // recoverable copy of its state.
    lock_entry(&dir, 1, 2, Duration::ZERO);
    let marker_backup = dir.join(".checkpoint.toml.1.2.previous");
    std::fs::write(
        &marker_backup,
        Checkpoint::new(version("1"), Checksum::of(b"stage")).render(),
    )
    .expect("park the previous marker");
    let boundary_backup = dir.join(".checkpoint-stage.1.2.previous");
    std::fs::create_dir_all(&boundary_backup).expect("park the previous boundary");
    std::fs::write(
        boundary_backup.join("users.sql"),
        "CREATE TABLE users (id INTEGER);",
    )
    .expect("write the parked snapshot");

    let error = recover_interrupted_commit(&checkpoint_path)
        .expect_err("a live foreign writer owns the artifacts");

    let message = error.to_string();
    assert!(message.contains("pid 1"), "got {message}");
    assert!(message.contains("nonce 2"), "got {message}");

    // Nothing is renamed, restored, or discarded while the writer lives: the pair
    // it parked is left exactly where it parked it.
    assert!(marker_backup.exists(), "the foreign marker copy is untouched");
    assert!(
        boundary_backup.exists(),
        "the foreign boundary copy is untouched"
    );
    assert!(!checkpoint_path.exists(), "no marker was restored");
    assert!(
        !dir.join("checkpoint-stage").exists(),
        "no boundary was restored"
    );
}

#[test]
fn the_parked_copies_of_a_crashed_writer_are_repaired() {
    let dir = workspace("commit-dead-owner");
    let checkpoint_path = dir.join("checkpoint.toml");

    // The entry names this process, which acquired the section after the writer that
    // parked this pair died: the kernel released that writer's advisory lock when its
    // process exited, so the copies it left behind carry a foreign scope and are
    // debris this run repairs.
    lock_entry(&dir, std::process::id(), 1, Duration::ZERO);
    let marker_backup = dir.join(".checkpoint.toml.1.2.previous");
    std::fs::write(
        &marker_backup,
        Checkpoint::new(version("1"), Checksum::of(b"stage")).render(),
    )
    .expect("park the previous marker");
    let boundary_backup = dir.join(".checkpoint-stage.1.2.previous");
    std::fs::create_dir_all(&boundary_backup).expect("park the previous boundary");
    std::fs::write(
        boundary_backup.join("users.sql"),
        "CREATE TABLE users (id INTEGER);",
    )
    .expect("write the parked snapshot");

    let notice = recover_interrupted_commit(&checkpoint_path)
        .expect("the crashed writer's pair is repaired")
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
fn the_command_line_schema_wins_over_the_configured_one() {
    let configured = CodegenConfig::parse("schema = \"app\"\n").expect("the schema key parses");

    assert_eq!(schema_of(Some("reporting"), &configured), "reporting");
    assert_eq!(schema_of(Some("   "), &configured), "app");
    assert_eq!(schema_of(None, &configured), "app");
    assert_eq!(schema_of(None, &CodegenConfig::default()), DEFAULT_SCHEMA);
}

/// A scratch admin session recording the catalog scans and drops it performs.
struct CountingSession {
    /// The scratch catalog the simulated server reports.
    catalog: Vec<ScratchCatalogEntry>,
    /// The number of catalog scans performed.
    scans: usize,
    /// The names the session was asked to drop, in order.
    drops: Vec<String>,
}

impl ScratchAdminSession for CountingSession {
    fn scratch_catalog(&mut self) -> Result<Vec<ScratchCatalogEntry>, String> {
        self.scans += 1;
        Ok(self.catalog.clone())
    }

    fn drop_scratch(&mut self, name: &str) -> bool {
        self.drops.push(name.to_owned());
        true
    }
}

/// A scratch catalog holding one stale database tagged for this host and one
/// foreign database.
fn scratch_catalog() -> Vec<ScratchCatalogEntry> {
    vec![
        ScratchCatalogEntry {
            name: "rurqe_scratch_1699999000_4321_0".to_owned(),
            description: Some(current_owner_tag()),
        },
        ScratchCatalogEntry {
            name: "app".to_owned(),
            description: None,
        },
    ]
}

#[test]
fn a_confirmed_cleanup_scans_the_catalog_once() {
    let mut session = CountingSession {
        catalog: scratch_catalog(),
        scans: 0,
        drops: Vec::new(),
    };

    let output = cleanup_scratch_catalog(
        &mut session,
        Duration::from_secs(600),
        true,
        "postgres://db.internal/scratch",
    )
    .expect("the confirmed cleanup drops the stale scratch databases");

    assert_eq!(
        session.scans, 1,
        "the cleanup reads the catalog exactly once"
    );
    assert_eq!(
        session.drops,
        vec!["rurqe_scratch_1699999000_4321_0".to_owned()],
        "the sweep drops exactly the candidates the confirmation listed"
    );
    assert!(
        output
            .lines()
            .iter()
            .any(|line| line.contains("Dropped 1")),
        "unexpected report: {:?}",
        output.lines()
    );
}

#[test]
fn an_unconfirmed_cleanup_names_the_candidates_without_dropping() {
    let mut session = CountingSession {
        catalog: scratch_catalog(),
        scans: 0,
        drops: Vec::new(),
    };

    let error = cleanup_scratch_catalog(
        &mut session,
        Duration::from_secs(600),
        false,
        "postgres://db.internal/scratch",
    )
    .expect_err("the cleanup demands confirmation");

    assert_eq!(
        session.scans, 1,
        "the confirmation names the candidates of a single scan"
    );
    assert!(
        session.drops.is_empty(),
        "nothing is dropped without confirmation"
    );

    let message = error.to_string();
    assert!(message.contains("--yes"), "got {message}");
    assert!(
        message.contains("rurqe_scratch_1699999000_4321_0"),
        "got {message}"
    );
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
fn a_commit_takes_over_the_section_of_a_crashed_writer() {
    let dir = workspace("commit-takeover");
    let stage_dir = dir.join("schema");
    std::fs::create_dir_all(&stage_dir).expect("create the stage directory");
    let lock_path = dir.join("schema.lock");
    let checkpoint_path = dir.join("checkpoint.toml");

    // The entry names a writer that died holding the section. Its record is old
    // because that writer entered the section long before it crashed, and the age
    // keeps it dead: the kernel released the writer's advisory lock when its process
    // exited, so the commit acquires the section at once instead of waiting for an
    // owner that no longer exists.
    lock_entry(&dir, 1, 2, Duration::from_secs(120));

    commit_stage(
        version("3.2"),
        ":memory:",
        DEFAULT_SCHEMA,
        &stage_dir,
        &lock_path,
        &checkpoint_path,
    )
    .expect("the section of the crashed writer is acquired");

    // The entry persists and the commit released the section, so the next writer
    // acquires it again instead of waiting behind a command that already left.
    CheckpointLock::acquire(&checkpoint_path).expect("the committed section is free again");
    assert!(
        checkpoint_path.exists(),
        "a commit over a crashed writer's section still writes its marker"
    );

    let leftovers: Vec<String> = directory_names(&dir)
        .into_iter()
        .filter(|name| name.starts_with('.'))
        .collect();
    assert!(leftovers.is_empty(), "leftover artifacts: {leftovers:?}");
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

#[test]
fn a_verification_takes_over_the_section_of_a_crashed_writer() {
    let dir = workspace("verify-takeover");
    let stage_dir = dir.join("schema");
    std::fs::create_dir_all(&stage_dir).expect("create the stage directory");
    let users = "CREATE TABLE users (id INTEGER);";
    std::fs::write(stage_dir.join("users.sql"), users).expect("write the stage script");
    let migrations_dir = dir.join("migrations");
    std::fs::create_dir_all(&migrations_dir).expect("create the migrations directory");

    // The boundary is live beside the marker and no copy is parked, so the
    // repair the verification runs first finds nothing to classify and the
    // equation replays exactly the pair both artifacts describe.
    let stage = DeclarativeStage::load(&stage_dir)
        .expect("the stage is readable")
        .expect("the stage directory exists");
    let catalog =
        materialise_stage(&stage, ":memory:", DEFAULT_SCHEMA).expect("the stage materialises");
    let checkpoint_path = dir.join("checkpoint.toml");
    std::fs::write(
        &checkpoint_path,
        Checkpoint::new(version("1"), catalog.checksum()).render(),
    )
    .expect("write the checkpoint");
    let boundary = dir.join("checkpoint-stage");
    std::fs::create_dir_all(&boundary).expect("install the boundary");
    std::fs::write(boundary.join("users.sql"), users).expect("write the boundary snapshot");

    lock_entry(&dir, 1, 2, Duration::from_secs(120));

    verify_stage(
        &stage_dir,
        None,
        &migrations_dir,
        &checkpoint_path,
        DEFAULT_SCHEMA,
        ":memory:",
    )
    .expect("the section of the crashed writer is acquired");

    // The entry persists and the verification released the section together with
    // the reads it guarded, so the next writer acquires it again.
    CheckpointLock::acquire(&checkpoint_path).expect("the verified section is free again");
}

#[test]
fn two_sequential_commits_converge_to_the_last_one() {
    let dir = workspace("commit-sequence");
    let stage_dir = dir.join("schema");
    std::fs::create_dir_all(&stage_dir).expect("create the stage directory");
    let users = "CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT NOT NULL);";
    std::fs::write(stage_dir.join("users.sql"), users).expect("write the stage script");

    // The live database holds exactly what the declarative stage declares, so
    // both commits pass the parity gate that stands in front of the artifacts.
    let database = dir.join("app.db");
    apply_live_ddl(&database, users);

    let lock_path = dir.join("schema.lock");
    let checkpoint_path = dir.join("checkpoint.toml");

    commit_stage(
        version("3"),
        &database.to_string_lossy(),
        DEFAULT_SCHEMA,
        &stage_dir,
        &lock_path,
        &checkpoint_path,
    )
    .expect("the first commit succeeds");
    commit_stage(
        version("4"),
        &database.to_string_lossy(),
        DEFAULT_SCHEMA,
        &stage_dir,
        &lock_path,
        &checkpoint_path,
    )
    .expect("the second commit succeeds");

    // The second commit holds the section after the first one released it, so
    // it runs over the pair the first one left live: the marker it parks is
    // the first commit's own, and the workspace converges on the second one.
    let checkpoint = Checkpoint::load(&checkpoint_path)
        .expect("the checkpoint is readable")
        .expect("the checkpoint exists");
    assert_eq!(checkpoint.pruned_up_to_version().to_string(), "4");
    assert_eq!(
        std::fs::read_to_string(dir.join("checkpoint-stage").join("users.sql"))
            .expect("read the committed boundary"),
        users
    );

    // No recoverable copy of a previous pair is left behind: the first commit's
    // pair was parked by the second and discarded once the second marker was
    // durable, and the second commit has no previous pair of its own.
    let leftovers: Vec<String> = directory_names(&dir)
        .into_iter()
        .filter(|name| name.starts_with('.'))
        .collect();
    assert!(leftovers.is_empty(), "leftover artifacts: {leftovers:?}");
}

#[test]
fn a_stage_rejected_by_the_parity_gate_leaves_an_interrupted_commit_parked() {
    let dir = workspace("commit-recovery-failure");
    let stage_dir = dir.join("schema");
    std::fs::create_dir_all(&stage_dir).expect("create the stage directory");
    // The declarative stage declares a column the live database never received,
    // so the parity gate rejects the commit during its preparation, before the
    // section is taken and before any repair could run.
    std::fs::write(
        stage_dir.join("users.sql"),
        "CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT NOT NULL);",
    )
    .expect("write the stage script");
    let database = dir.join("app.db");
    apply_live_ddl(&database, "CREATE TABLE users (id INTEGER PRIMARY KEY);");

    let lock_path = dir.join("schema.lock");
    let checkpoint_path = dir.join("checkpoint.toml");

    // A killed commit left the previous marker and boundary parked. The gate runs
    // outside the section, so the commit neither restores nor discards the pair:
    // the only copy of the previous state stays exactly where it was parked.
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

    let error = commit_stage(
        version("3.2"),
        &database.to_string_lossy(),
        DEFAULT_SCHEMA,
        &stage_dir,
        &lock_path,
        &checkpoint_path,
    )
    .expect_err("the drifted stage cannot be sealed into a checkpoint");

    let message = error.to_string();
    assert!(
        message.contains("does not describe the database"),
        "the diagnostic names the violated parity, got {message}"
    );
    assert!(
        message.contains("email"),
        "the diagnostic names the drifted column, got {message}"
    );
    assert!(
        !message.contains("repaired an interrupted commit"),
        "the gate runs before any repair, so the diagnostic carries no repair note, got {message}"
    );

    // The gate wrote nothing and repaired nothing: the parked pair is the only
    // copy of the previous state and stays parked, and no artifact of the commit
    // stands beside it.
    assert!(marker_backup.exists(), "the parked marker is untouched");
    assert!(boundary_backup.exists(), "the parked boundary is untouched");
    assert!(!checkpoint_path.exists(), "no marker was written");
    assert!(
        !dir.join("checkpoint-stage").exists(),
        "no boundary snapshot was written"
    );
    assert!(!lock_path.exists(), "no lock was written");
    assert!(
        !dir.join("checkpoint.lock").exists(),
        "the commit never entered its section"
    );
}

#[test]
fn a_failed_verification_reports_the_interrupted_commit_it_repaired() {
    let dir = workspace("verify-recovery-failure");
    let stage_dir = dir.join("schema");
    std::fs::create_dir_all(&stage_dir).expect("create the stage directory");
    std::fs::write(stage_dir.join("users.sql"), "CREATE TABLE users (id INTEGER);")
        .expect("write the stage script");
    let migrations_dir = dir.join("migrations");
    std::fs::create_dir_all(&migrations_dir).expect("create the migrations directory");
    let checkpoint_path = dir.join("checkpoint.toml");

    // The parked boundary replays a different relation than the declarative
    // stage, so the repaired checkpoint cannot satisfy the golden equation and
    // the failure has to say that the workspace already changed underneath it.
    let marker_backup = dir.join(".checkpoint.toml.4242.7.previous");
    std::fs::write(
        &marker_backup,
        Checkpoint::new(version("1"), Checksum::of(b"stage")).render(),
    )
    .expect("park the previous marker");
    let boundary_backup = dir.join(".checkpoint-stage.4242.7.previous");
    std::fs::create_dir_all(&boundary_backup).expect("park the previous boundary");
    std::fs::write(
        boundary_backup.join("orders.sql"),
        "CREATE TABLE orders (id INTEGER);",
    )
    .expect("write the parked snapshot");

    let error = verify_stage(
        &stage_dir,
        None,
        &migrations_dir,
        &checkpoint_path,
        DEFAULT_SCHEMA,
        ":memory:",
    )
    .expect_err("the diverging stage cannot satisfy the golden equation");

    let message = error.to_string();
    assert!(
        message.contains("Restored the checkpoint marker"),
        "the failure names the repair that already moved artifacts, got {message}"
    );
    assert!(
        checkpoint_path.exists(),
        "the repair really restored the marker the failure reports"
    );
}
