//! Unit tests of the embedded lifecycle protocol.

use super::*;
// Only the asynchronous baseline tests name `MigrationKind`, so the import
// carries the same feature gate as the tests that read it.
#[cfg(feature = "tokio")]
use crate::migration::MigrationKind;
use crate::runner::{InMemoryBackend, InMemoryTransaction};

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

/// Cloning a collection shares every script with the original, so the clone stays
/// value-equal, keeps the canonical order, and materialises that order only when it
/// is consumed by value.
#[test]
fn a_cloned_collection_keeps_the_canonical_order() {
    let embedded = embedded();
    let clone = embedded.clone();

    assert_eq!(clone, embedded);

    let owned: Vec<Migration> = clone.into_iter().collect();
    let names: Vec<&str> = owned
        .iter()
        .map(|migration| migration.script.as_str())
        .collect();
    assert_eq!(names, vec!["V1__users.sql", "V2__orders.sql"]);

    assert_eq!(
        embedded.len(),
        2,
        "the clone shares its scripts with the original"
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

    let (outcome, backend) = runtime
        .block_on(embedded().migrate_async(InMemoryBackend::new()));

    let report = outcome.expect("the blocking run succeeds");
    let backend = backend.expect("the backend is handed back with the report");

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
    let (outcome, backend) = runtime
        .block_on(embedded.migrate_async(InMemoryBackend::new()));
    outcome.expect("the blocking run succeeds");
    let backend = backend.expect("the backend is handed back with the report");

    let (outcome, backend) = runtime.block_on(embedded.status_async(backend));
    let statuses = outcome.expect("the blocking status read succeeds");
    let backend = backend.expect("the backend is handed back with the statuses");

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

    let (outcome, handed) = runtime.block_on(embedded.migrate_async(backend));

    match outcome {
        Err(Error::MigrationExecutionFailed { .. }) => {}
        Err(other) => panic!("expected MigrationExecutionFailed, got {other:?}"),
        Ok(_) => panic!("the failing script aborts the run"),
    }
    assert!(
        handed.is_some(),
        "a failed run still hands the healthy connection back"
    );
}

#[test]
#[cfg(feature = "tokio")]
fn baseline_async_records_the_baseline_and_hands_the_backend_back() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("build tokio runtime");

    let version: MigrationVersion = "1.0.0".parse().expect("a valid version");

    let (outcome, backend) = runtime
        .block_on(embedded().baseline_async(
            InMemoryBackend::new(),
            version.clone(),
            "legacy baseline",
        ));

    let entry = outcome.expect("the blocking baseline succeeds");
    let backend = backend.expect("the backend is handed back with the entry");

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

    let (outcome, backend) = runtime.block_on(fixed.repair_async(backend));

    let report = outcome.expect("the blocking repair succeeds");
    let backend = backend.expect("the backend is handed back with the report");

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

    let (outcome, backend) = runtime
        .block_on(embedded().migrate_async(InMemoryBackend::new()));

    let report = outcome.expect("the blocking run succeeds under `strict-async`");
    let backend = backend.expect("the backend is handed back with the report");

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

    let (outcome, backend) = runtime
        .block_on(embedded().baseline_async(InMemoryBackend::new(), version, "legacy baseline"));

    let entry = outcome.expect("the blocking baseline succeeds under `strict-async`");
    let backend = backend.expect("the backend is handed back with the entry");

    assert_eq!(entry.kind, MigrationKind::Baseline);
    assert_eq!(backend.history.len(), 1);
    assert!(backend.history[0].success);
}

/// An [`InMemoryBackend`] that records whether the exclusive run lock was held
/// while the final catalog digest was read.
///
/// The lifecycle contract is delegated to the wrapped backend, so the inner
/// `locked` flag mirrors the outer run, and the overridden
/// [`catalog_checksum`](MigrationBackend::catalog_checksum) records that flag at
/// each call.
#[derive(Debug, Default)]
struct LockObservingBackend {
    inner: InMemoryBackend,
    lock_states_at_checksum: Vec<bool>,
}

impl MigrationBackend for LockObservingBackend {
    type Transaction<'a>
        = InMemoryTransaction<'a>
    where
        Self: 'a;

    fn ensure_schema_history_table(&mut self) -> Result<(), Error> {
        self.inner.ensure_schema_history_table()
    }

    fn read_history(&mut self) -> Result<Vec<SchemaHistoryEntry>, Error> {
        self.inner.read_history()
    }

    fn begin(&mut self) -> Result<Self::Transaction<'_>, Error> {
        self.inner.begin()
    }

    fn acquire_lock(&mut self) -> Result<(), Error> {
        self.inner.acquire_lock()
    }

    fn release_lock(&mut self) -> Result<(), Error> {
        self.inner.release_lock()
    }

    fn catalog_checksum(&mut self) -> Result<Option<Checksum>, Error> {
        self.lock_states_at_checksum.push(self.inner.locked);
        self.inner.catalog_checksum()
    }
}

/// The final comparison is a backend interaction like every other step of the
/// lifecycle, so it reads the catalog under the same exclusive run lock: a
/// concurrent `clean` or `migrate` can then neither drop nor advance the schema
/// between the commit of the tail and the comparison.
#[test]
fn the_final_catalog_digest_is_read_under_the_run_lock() {
    let catalog = SchemaCatalog::default();
    let snapshot = catalog.to_lock();

    let embedded = EmbeddedMigrations::from_artifacts(EmbeddedArtifacts {
        tail: &[("V1__users.sql", "CREATE TABLE users (id INTEGER);")],
        checkpoint: None,
        stage: &[],
        schema_lock: Some(&snapshot),
    })
    .expect("valid artifacts");

    let mut backend = LockObservingBackend::default();
    backend.inner.catalog_checksum = Some(catalog.checksum());

    embedded
        .migrate(&mut backend)
        .expect("the produced catalog matches the baked digest");

    assert_eq!(
        backend.lock_states_at_checksum,
        vec![true],
        "the final catalog read runs under the same run lock as the other steps"
    );
    assert!(
        !backend.inner.locked,
        "the run lock is released once the run returns"
    );
}

/// A collection baking a checkpoint and a stage but no snapshot anchors its cold
/// start marker with the boundary hash the checkpoint carries, never with the
/// digest of the stage text.
///
/// The marker recorded at the pruned boundary is measured against that hash, and a
/// digest over file bodies can never equal a digest over a catalog. Anchoring the
/// stage digest would therefore reject the target the stage had just initialised,
/// and — the cold start commits before the tail runs — keep rejecting it on every
/// later run, leaving the target unusable for the binary that created it.
#[test]
fn a_baked_checkpoint_anchors_the_cold_start_marker() {
    let boundary = Checksum::of(b"the catalog of the boundary stage");
    let marker = format!(
        "[checkpoint]\npruned_up_to_version = \"2\"\nschema_hash = \"{}\"\n",
        boundary.to_hex()
    );

    let embedded = EmbeddedMigrations::from_artifacts(EmbeddedArtifacts {
        tail: &[],
        checkpoint: Some(marker.as_str()),
        stage: &[("users.sql", "CREATE TABLE users (id INTEGER);")],
        schema_lock: None,
    })
    .expect("valid artifacts");

    let mut backend = InMemoryBackend::new();
    let report = embedded
        .migrate(&mut backend)
        .expect("the checkpoint accepts the database its own stage initialised");

    assert_eq!(
        report.cold_start.expect("the cold start runs").stage_scripts,
        vec!["users.sql"]
    );

    let recorded = backend
        .history
        .first()
        .expect("the cold start records its marker");
    assert!(recorded.success);
    assert_eq!(
        recorded.version.as_ref().map(ToString::to_string),
        Some("2".to_owned()),
        "the marker sits at the pruned boundary the checkpoint declares"
    );
    assert_eq!(
        recorded.checksum,
        Some(boundary),
        "the marker anchors the boundary catalog digest, not the stage text digest"
    );

    let rerun = embedded
        .migrate(&mut backend)
        .expect("the initialised target is accepted again");
    assert!(rerun.cold_start.is_none());
    assert_eq!(rerun.applied_count, 0);
    assert_eq!(backend.history.len(), 1);
}

/// The baked `schema.lock` digest outranks the boundary hash: it anchors the
/// catalog the final check measures the run against, so it stays the digest
/// whenever the collection bakes one.
#[test]
fn a_baked_snapshot_outranks_the_checkpoint_boundary_hash() {
    let catalog = SchemaCatalog::default();
    let snapshot = catalog.to_lock();
    let boundary = Checksum::of(b"an earlier boundary stage");
    let marker = format!(
        "[checkpoint]\npruned_up_to_version = \"2\"\nschema_hash = \"{}\"\n",
        boundary.to_hex()
    );

    let embedded = EmbeddedMigrations::from_artifacts(EmbeddedArtifacts {
        tail: &[("V3__orders.sql", "CREATE TABLE orders (id INTEGER);")],
        checkpoint: Some(marker.as_str()),
        stage: &[("users.sql", "CREATE TABLE users (id INTEGER);")],
        schema_lock: Some(&snapshot),
    })
    .expect("valid artifacts");

    let mut backend = InMemoryBackend::new();
    backend.catalog_checksum = Some(catalog.checksum());

    embedded
        .migrate(&mut backend)
        .expect("the catalog matches the baked snapshot");

    let recorded = backend
        .history
        .first()
        .expect("the cold start records its marker");
    assert_eq!(
        recorded.checksum,
        Some(catalog.checksum()),
        "the snapshot digest anchors the marker, not the boundary hash"
    );
    assert_eq!(
        recorded.version.as_ref().map(ToString::to_string),
        Some("3".to_owned()),
        "the marker covers the newest tail version above the boundary"
    );
}

/// A collection baking no catalog artifact keeps the digest of its own stage: no
/// checkpoint and no snapshot measures it against a catalog, so it stays the
/// change detector of the stage text the database was initialised from.
#[test]
fn a_stage_without_a_catalog_artifact_anchors_its_own_digest() {
    let embedded = EmbeddedMigrations::from_artifacts(EmbeddedArtifacts {
        tail: &[],
        checkpoint: None,
        stage: &[("users.sql", "CREATE TABLE users (id INTEGER);")],
        schema_lock: None,
    })
    .expect("valid artifacts");

    let mut backend = InMemoryBackend::new();
    embedded
        .migrate(&mut backend)
        .expect("the cold start succeeds");

    let expected = DeclarativeStage::new(vec![StageScript::new(
        "users.sql",
        "CREATE TABLE users (id INTEGER);",
    )])
    .checksum();
    assert_eq!(backend.history[0].checksum, Some(expected));
}

/// A snapshot disagreeing with the boundary is refused at construction when the
/// cold-start marker would land exactly on that boundary.
///
/// The marker sits at the higher of the newest versioned migration and the
/// pruned boundary, and a marker recorded at the boundary is measured against
/// the checkpoint hash. Recording the snapshot digest there would initialise the
/// target and only then reject it — on every later run as well.
#[test]
fn a_snapshot_disagreeing_with_the_boundary_is_refused_at_construction() {
    let boundary = Checksum::of(b"the boundary stage");
    let snapshot = SchemaCatalog::default().to_lock();
    let marker = format!(
        "[checkpoint]\npruned_up_to_version = \"2\"\nschema_hash = \"{}\"\n",
        boundary.to_hex()
    );

    let error = EmbeddedMigrations::from_artifacts(EmbeddedArtifacts {
        tail: &[("V2__orders.sql", "CREATE TABLE orders (id INTEGER);")],
        checkpoint: Some(marker.as_str()),
        stage: &[("users.sql", "CREATE TABLE users (id INTEGER);")],
        schema_lock: Some(&snapshot),
    })
    .expect_err("the marker would land exactly at the boundary with the snapshot digest");

    assert_eq!(
        error,
        Error::IncoherentArtifacts {
            pruned_up_to_version: "2".to_owned(),
            snapshot_hash: SchemaCatalog::default().checksum().to_hex(),
            checkpoint_hash: boundary.to_hex(),
        }
    );
}

/// The refusal does not depend on a stage or a tail migration: a collection
/// declaring neither still anchors its marker at the boundary it bakes.
#[test]
fn a_snapshot_disagreeing_with_the_boundary_is_refused_without_a_tail() {
    let boundary = Checksum::of(b"the boundary stage");
    let snapshot = SchemaCatalog::default().to_lock();
    let marker = format!(
        "[checkpoint]\npruned_up_to_version = \"2\"\nschema_hash = \"{}\"\n",
        boundary.to_hex()
    );

    let error = EmbeddedMigrations::from_artifacts(EmbeddedArtifacts {
        tail: &[],
        checkpoint: Some(marker.as_str()),
        stage: &[],
        schema_lock: Some(&snapshot),
    })
    .expect_err("a collection declaring neither a tail nor a stage still anchors the boundary");

    assert_eq!(
        error,
        Error::IncoherentArtifacts {
            pruned_up_to_version: "2".to_owned(),
            snapshot_hash: SchemaCatalog::default().checksum().to_hex(),
            checkpoint_hash: boundary.to_hex(),
        }
    );
}

/// A versioned migration above the boundary lets the tail supersede the boundary
/// state, so the checkpoint and the snapshot legitimately describe two different
/// catalogs and the collection is assembled.
#[test]
fn a_versioned_script_above_the_boundary_keeps_the_artifacts_decodable() {
    let boundary = Checksum::of(b"an earlier boundary stage");
    let snapshot = SchemaCatalog::default().to_lock();
    let marker = format!(
        "[checkpoint]\npruned_up_to_version = \"2\"\nschema_hash = \"{}\"\n",
        boundary.to_hex()
    );

    let embedded = EmbeddedMigrations::from_artifacts(EmbeddedArtifacts {
        tail: &[("V3__orders.sql", "CREATE TABLE orders (id INTEGER);")],
        checkpoint: Some(marker.as_str()),
        stage: &[("users.sql", "CREATE TABLE users (id INTEGER);")],
        schema_lock: Some(&snapshot),
    })
    .expect("the tail supersedes the boundary state, so the digests may differ");

    assert_eq!(embedded.len(), 1);
    assert!(embedded.checkpoint().is_some());
    assert!(embedded.schema_hash().is_some());
}

/// The refusal targets the disagreement, never the combination: agreeing digests
/// at the boundary construct and complete the run they describe.
#[test]
fn an_agreeing_snapshot_initialises_the_boundary_target() {
    let catalog = SchemaCatalog::default();
    let snapshot = catalog.to_lock();
    let marker = format!(
        "[checkpoint]\npruned_up_to_version = \"2\"\nschema_hash = \"{}\"\n",
        catalog.checksum().to_hex()
    );

    let embedded = EmbeddedMigrations::from_artifacts(EmbeddedArtifacts {
        tail: &[],
        checkpoint: Some(marker.as_str()),
        stage: &[("users.sql", "CREATE TABLE users (id INTEGER);")],
        schema_lock: Some(&snapshot),
    })
    .expect("the baked artifacts describe one catalog");

    let mut backend = InMemoryBackend::new();
    backend.catalog_checksum = Some(catalog.checksum());

    let report = embedded
        .migrate(&mut backend)
        .expect("the target is initialised at the boundary");

    assert_eq!(
        report.cold_start.expect("the cold start runs").stage_scripts,
        vec!["users.sql"]
    );
    assert_eq!(report.applied_count, 0);

    assert_eq!(backend.history.len(), 1);
    let recorded = backend.history.first().expect("the marker is recorded");
    assert!(recorded.success);
    assert_eq!(recorded.checksum, Some(catalog.checksum()));
    assert_eq!(
        recorded.version.as_ref().map(ToString::to_string),
        Some("2".to_owned())
    );
}

/// A backend that records every interaction with the run lock, so a test can
/// assert that one span covers the whole embedded lifecycle.
#[derive(Debug, Default)]
struct SpanObservingBackend {
    inner: InMemoryBackend,
    acquires: usize,
    releases: usize,
    /// Every recorded backend interaction and whether the run lock was held.
    interactions: Vec<(&'static str, bool)>,
}

impl SpanObservingBackend {
    fn record(&mut self, interaction: &'static str) {
        self.interactions.push((interaction, self.inner.locked));
    }
}

impl MigrationBackend for SpanObservingBackend {
    type Transaction<'a>
        = InMemoryTransaction<'a>
    where
        Self: 'a;

    fn ensure_schema_history_table(&mut self) -> Result<(), Error> {
        self.record("ensure");
        self.inner.ensure_schema_history_table()
    }

    fn read_history(&mut self) -> Result<Vec<SchemaHistoryEntry>, Error> {
        self.record("read");
        self.inner.read_history()
    }

    fn catalog_checksum(&mut self) -> Result<Option<Checksum>, Error> {
        self.record("catalog");
        self.inner.catalog_checksum()
    }

    fn begin(&mut self) -> Result<Self::Transaction<'_>, Error> {
        self.record("begin");
        self.inner.begin()
    }

    fn acquire_lock(&mut self) -> Result<(), Error> {
        self.acquires += 1;
        self.inner.acquire_lock()
    }

    fn release_lock(&mut self) -> Result<(), Error> {
        self.releases += 1;
        self.inner.release_lock()
    }
}

/// The run lock is taken once at entry and released once when the run returns, so
/// neither the cold start nor the final comparison sits in a window a concurrent
/// lifecycle operation could exploit. The history is read once on a warm target and
/// twice when the cold start ran, because the marker the cold start records has to
/// be visible to the tail.
#[test]
fn the_whole_lifecycle_runs_inside_one_lock_span() {
    let catalog = SchemaCatalog::default();
    let snapshot = catalog.to_lock();

    let embedded = EmbeddedMigrations::from_artifacts(EmbeddedArtifacts {
        tail: &[(
            "R__audit.sql",
            "CREATE VIEW audit AS SELECT id FROM users;",
        )],
        checkpoint: None,
        stage: &[("users.sql", "CREATE TABLE users (id INTEGER);")],
        schema_lock: Some(&snapshot),
    })
    .expect("valid artifacts");

    let mut backend = SpanObservingBackend::default();
    backend.inner.catalog_checksum = Some(catalog.checksum());

    let cold = embedded
        .migrate(&mut backend)
        .expect("the database is cold started");
    assert!(cold.cold_start.is_some());

    assert_eq!(backend.acquires, 1, "one acquire covers the whole run");
    assert_eq!(backend.releases, 1, "one release when the run returns");
    assert!(
        backend.interactions.iter().all(|entry| entry.1),
        "every backend interaction runs inside the lock: {:?}",
        backend.interactions
    );
    assert_eq!(
        backend
            .interactions
            .iter()
            .filter(|entry| entry.0 == "read")
            .count(),
        2,
        "the probe reads the history and the cold start makes the tail re-read it"
    );
    assert!(
        !backend.inner.locked,
        "the run releases the lock when it returns"
    );

    backend.interactions.clear();

    let warm = embedded
        .migrate(&mut backend)
        .expect("the initialised target is accepted again");

    assert!(warm.cold_start.is_none());
    assert_eq!(backend.acquires, 2, "the warm run takes the lock once as well");
    assert_eq!(backend.releases, 2);
    assert!(
        backend.interactions.iter().all(|entry| entry.1),
        "every backend interaction runs inside the lock: {:?}",
        backend.interactions
    );
    assert_eq!(
        backend
            .interactions
            .iter()
            .filter(|entry| entry.0 == "read")
            .count(),
        1,
        "a warm run classifies the tail against the probe read alone"
    );
    assert!(
        !backend.inner.locked,
        "the run releases the lock when it returns"
    );
}

/// The script checks moved into the tail's guarded span keep their place: a list a
/// lifecycle run cannot apply is still refused before any script executes, and the
/// refusal releases the run lock.
#[test]
fn duplicate_versions_are_rejected_when_the_run_starts() {
    let embedded = EmbeddedMigrations::new(&[
        ("V7__users.sql", "CREATE TABLE users (id INTEGER);"),
        ("V7__orders.sql", "CREATE TABLE orders (id INTEGER);"),
    ])
    .expect("assembling the list needs no version to be unique");

    let mut backend = InMemoryBackend::new();
    let error = embedded
        .migrate(&mut backend)
        .expect_err("two scripts of one version cannot be applied in order");

    assert_eq!(
        error,
        Error::DuplicateMigrationVersion {
            version: "7".to_owned(),
            first: "V7__users.sql".to_owned(),
            second: "V7__orders.sql".to_owned(),
        }
    );
    assert!(
        !backend.table_created,
        "the refusal precedes the creation of the metadata table"
    );
    assert!(
        backend.executed_sql.is_empty(),
        "no script of the refused run was applied"
    );
    assert!(
        backend.history.is_empty(),
        "the refused run records no history entry"
    );
    assert!(
        !backend.locked,
        "the refusal precedes the acquisition of the run lock"
    );
}

/// A malformed tail is refused before the cold start runs, so a target the
/// collection would have initialised is left exactly as the run found it.
///
/// The refusal is decidable from the migration list alone, and the cold start
/// commits the stage and its marker before the tail runs: validating afterwards
/// would leave an initialised target behind a run that never applied a script.
#[test]
fn a_duplicate_version_is_refused_before_the_cold_start_runs() {
    let embedded = EmbeddedMigrations::from_artifacts(EmbeddedArtifacts {
        tail: &[
            ("V7__users.sql", "CREATE TABLE users (id INTEGER);"),
            ("V7__orders.sql", "CREATE TABLE orders (id INTEGER);"),
        ],
        checkpoint: None,
        stage: &[("users.sql", "CREATE TABLE users (id INTEGER, email TEXT);")],
        schema_lock: None,
    })
    .expect("assembling the list needs no version to be unique");

    let mut backend = InMemoryBackend::new();
    let error = embedded
        .migrate(&mut backend)
        .expect_err("two scripts of one version cannot be applied in order");

    assert_eq!(
        error,
        Error::DuplicateMigrationVersion {
            version: "7".to_owned(),
            first: "V7__users.sql".to_owned(),
            second: "V7__orders.sql".to_owned(),
        }
    );
    assert!(
        !backend.table_created,
        "the cold start never creates the metadata table"
    );
    assert!(
        backend.executed_sql.is_empty(),
        "the declarative stage is never applied"
    );
    assert!(
        backend.history.is_empty(),
        "no cold-start marker is recorded"
    );
    assert!(
        !backend.locked,
        "the refusal precedes the acquisition of the run lock"
    );
}