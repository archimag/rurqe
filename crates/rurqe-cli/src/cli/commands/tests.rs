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

use super::stage::{cleanup_scratch_catalog, swap_snapshot, verify_stage, write_staged_boundary};
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
    let catalog = materialise_stage(&stage, ":memory:").expect("the stage materialises");

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
    .expect("the boundary is committed together with its snapshot");

    let snapshot = dir.join("checkpoint-stage");
    assert_eq!(
        std::fs::read_to_string(snapshot.join("users.sql")).expect("read the snapshot"),
        users
    );

    // A later commit replaces the snapshot, so a script the new boundary
    // dropped never lingers next to the marker.
    std::fs::remove_file(stage_dir.join("users.sql")).expect("drop the script");
    std::fs::write(stage_dir.join("orders.sql"), "CREATE TABLE orders (id INTEGER);")
        .expect("write the new script");

    commit_stage(
        version("4"),
        ":memory:",
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
fn a_failed_checkpoint_write_keeps_the_committed_lock_readable() {
    let dir = workspace("commit-partial");
    let stage_dir = dir.join("schema");
    std::fs::create_dir_all(&stage_dir).expect("create the stage directory");
    let lock_path = dir.join("schema.lock");
    let blocker = dir.join("blocker");
    std::fs::write(&blocker, "not a directory").expect("write the blocking file");
    let checkpoint_path = blocker.join("checkpoint.toml");

    // The lock lands before the boundary artifacts, and the marker lands last,
    // so a boundary that cannot be written never leaves a checkpoint pointing
    // at a lock file that describes a different database.
    let error = commit_stage(
        version("3.2"),
        ":memory:",
        DEFAULT_SCHEMA,
        &stage_dir,
        &lock_path,
        &checkpoint_path,
    )
    .expect_err("a file cannot hold the boundary artifacts");

    assert!(error.to_string().contains("blocker"), "{error}");

    let lock = std::fs::read_to_string(&lock_path).expect("read the lock");
    SchemaCatalog::from_lock(&lock).expect("the committed lock stays canonical");
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
fn an_interrupted_snapshot_swap_restores_the_committed_boundary() {
    let dir = workspace("snapshot-swap");
    let boundary = dir.join("checkpoint-stage");
    std::fs::create_dir_all(&boundary).expect("create the committed boundary");
    std::fs::write(boundary.join("users.sql"), "CREATE TABLE users (id INTEGER);")
        .expect("write the committed snapshot");

    // The staging directory is gone, so installing the new snapshot fails right
    // after the previous boundary has been moved aside.
    let staged = dir.join(".checkpoint-stage.staged.tmp");

    let error = swap_snapshot(&boundary, &staged)
        .expect_err("a missing staging directory cannot be installed");

    assert!(error.to_string().contains("checkpoint-stage"), "{error}");
    assert_eq!(
        std::fs::read_to_string(boundary.join("users.sql")).expect("read the restored snapshot"),
        "CREATE TABLE users (id INTEGER);"
    );

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

    assert_eq!(entries, vec!["checkpoint-stage".to_owned()]);
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
fn a_boundary_parked_by_a_killed_swap_is_restored_by_the_next_one() {
    let dir = workspace("snapshot-parked");
    let boundary = dir.join("checkpoint-stage");

    // The swap parked the committed boundary and was killed before the staged
    // replacement reached its place.
    let backup = dir.join(".checkpoint-stage.previous");
    std::fs::create_dir_all(&backup).expect("park the committed boundary");
    std::fs::write(
        backup.join("users.sql"),
        "CREATE TABLE users (id INTEGER);",
    )
    .expect("write the parked snapshot");

    let staged = dir.join(".checkpoint-stage.4242.1.tmp");
    std::fs::create_dir_all(&staged).expect("stage the replacement boundary");
    std::fs::write(
        staged.join("orders.sql"),
        "CREATE TABLE orders (id INTEGER);",
    )
    .expect("write the staged snapshot");

    swap_snapshot(&boundary, &staged).expect("the replacement boundary is installed");

    assert_eq!(directory_names(&boundary), vec!["orders.sql".to_owned()]);
    assert!(!backup.exists(), "the superseded parked snapshot is discarded");
    assert!(!staged.exists(), "the staged snapshot was renamed into place");
}

#[test]
fn a_parked_backup_beside_a_live_boundary_is_discarded_by_the_next_swap() {
    let dir = workspace("snapshot-superseded");
    let boundary = dir.join("checkpoint-stage");
    std::fs::create_dir_all(&boundary).expect("create the committed boundary");
    std::fs::write(
        boundary.join("users.sql"),
        "CREATE TABLE users (id INTEGER);",
    )
    .expect("write the committed snapshot");

    // The swap that parked this backup already published its replacement, so
    // the parked copy is debris rather than the only copy of the boundary.
    let backup = dir.join(".checkpoint-stage.previous");
    std::fs::create_dir_all(&backup).expect("park the superseded boundary");
    std::fs::write(
        backup.join("legacy.sql"),
        "CREATE TABLE legacy (id INTEGER);",
    )
    .expect("write the superseded snapshot");

    let staged = dir.join(".checkpoint-stage.4242.2.tmp");
    std::fs::create_dir_all(&staged).expect("stage the replacement boundary");
    std::fs::write(
        staged.join("orders.sql"),
        "CREATE TABLE orders (id INTEGER);",
    )
    .expect("write the staged snapshot");

    swap_snapshot(&boundary, &staged).expect("the replacement boundary is installed");

    assert_eq!(directory_names(&boundary), vec!["orders.sql".to_owned()]);
    assert!(!backup.exists(), "the superseded parked snapshot is discarded");
}

#[test]
fn verifying_a_missing_boundary_names_the_parked_snapshot_that_restores_it() {
    let dir = workspace("verify-parked");
    let stage_dir = dir.join("schema");
    std::fs::create_dir_all(&stage_dir).expect("create the stage directory");
    std::fs::write(stage_dir.join("users.sql"), "CREATE TABLE users (id INTEGER);")
        .expect("write the stage script");
    let migrations_dir = dir.join("migrations");
    std::fs::create_dir_all(&migrations_dir).expect("create the migrations directory");

    // The marker anchors a non-empty boundary whose snapshot a killed swap
    // parked, so the committed boundary is no longer where a verification
    // looks for it.
    let checkpoint_path = dir.join("checkpoint.toml");
    checkpoint_file(&checkpoint_path, "1");
    let backup = dir.join(".checkpoint-stage.previous");
    std::fs::create_dir_all(&backup).expect("park the committed boundary");
    std::fs::write(
        backup.join("users.sql"),
        "CREATE TABLE users (id INTEGER);",
    )
    .expect("write the parked snapshot");

    let error = verify_stage(&stage_dir, None, &migrations_dir, &checkpoint_path, ":memory:")
        .expect_err("the boundary snapshot is missing");

    let message = error.to_string();
    assert!(
        message.contains(".checkpoint-stage.previous"),
        "the diagnostic names the parked snapshot, got {message}"
    );
    assert!(
        message.contains("mv "),
        "the diagnostic names the rename that restores it, got {message}"
    );
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
