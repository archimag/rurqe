//! Tokio offload entry points for the migration lifecycle.
//!
//! The synchronous lifecycle of [`Migrator`] blocks the calling thread until the
//! database answers, which Conventions.md §3 forbids on an async worker thread.
//! Every wrapper here therefore runs the guard-free core of one lifecycle
//! operation — `migrate_core`, `status_core`, `baseline_core`, `repair_core` — on
//! a dedicated blocking thread through [`tokio::task::spawn_blocking`], so the
//! runtime never starves while the connection works.
//!
//! The backend is moved into the blocking task and handed back together with the
//! outcome, so a connection that cannot be shared across threads stays usable after
//! the run — a failed run returns it too, which keeps the follow-up the failure
//! demands, `repair` or an inspection of the recorded history, on that very
//! connection. Only a task that never returned hands nothing back: the backend is
//! dropped inside it, and such a panic — which this crate never produces — surfaces
//! as [`Error::Backend`] instead of unwinding the caller.
//!
//! A caller that hands over a script slice pays exactly one copy per call, because
//! the `'static` bound of the blocking task requires an owned list and a slice is
//! borrowed; a collection that already shares its scripts behind an [`Arc`] hands
//! that allocation over instead and pays nothing. The lifecycle core orders the list
//! by reference, so the script bodies are never duplicated a second time either way.
//! Under the `strict-async` feature these wrappers are the sanctioned way to drive
//! the lifecycle from inside a runtime: the synchronous entry points then refuse to
//! run there, while the wrappers keep working because they call the guard-free
//! cores. That also covers a runner anchored by [`Migrator::with_checkpoint`],
//! which has no Tokio-safe synchronous alternative at all.

use std::sync::Arc;

use crate::error::Error;
use crate::history::SchemaHistoryEntry;
use crate::migration::Migration;
use crate::runner::{MigrationBackend, MigrationReport, MigrationStatus, Migrator, RepairReport};
use crate::version::MigrationVersion;

impl Migrator {
    /// Runs all pending versioned and modified repeatable migrations on a dedicated
    /// blocking thread.
    ///
    /// The run happens on a `spawn_blocking` thread, so no Tokio worker thread
    /// performs the blocking database I/O Conventions.md §3 forbids. The backend is
    /// moved into the blocking task and handed back together with the outcome — a
    /// failed run returns it too — so the caller keeps the connection it migrated,
    /// and only a task that never returned hands nothing back. Such a panic — which
    /// this crate never produces — surfaces as [`Error::Backend`] instead of
    /// unwinding the caller. Under `strict-async` this is the sanctioned runtime
    /// entry point, because it drives the guard-free core rather than the guarded
    /// synchronous method.
    pub async fn migrate_async<B: MigrationBackend + Send + 'static>(
        &self,
        backend: B,
        migrations: &[Migration],
    ) -> (Result<MigrationReport, Error>, Option<B>) {
        self.migrate_shared_async(backend, Arc::from(migrations))
            .await
    }

    /// Runs all pending versioned and modified repeatable migrations from a script
    /// list the caller already shares on a dedicated blocking thread.
    ///
    /// [`migrate_async`](Self::migrate_async) copies the slice it is handed into such
    /// a list; a caller that owns one — an embedded collection, whose scripts live
    /// behind an [`Arc`] — passes it here and pays no copy at all. Every other
    /// guarantee of the public entry point holds: the run happens on a
    /// `spawn_blocking` thread, the backend comes back with the outcome whether the
    /// run succeeded or failed, and a panic inside the task surfaces as
    /// [`Error::Backend`] instead of unwinding the caller.
    pub(crate) async fn migrate_shared_async<B: MigrationBackend + Send + 'static>(
        &self,
        mut backend: B,
        migrations: Arc<[Migration]>,
    ) -> (Result<MigrationReport, Error>, Option<B>) {
        let migrator = self.clone();

        let joined = tokio::task::spawn_blocking(move || {
            let report = migrator.migrate_core(&mut backend, &migrations);
            (report, backend)
        })
        .await;

        join_offloaded(joined, "migration")
    }

    /// Inspects the status of migrations against the database history on a
    /// dedicated blocking thread.
    ///
    /// The read runs on a `spawn_blocking` thread, so no Tokio worker thread
    /// performs the blocking database I/O Conventions.md §3 forbids. The backend is
    /// moved into the blocking task and handed back together with the outcome — a
    /// failed read returns it too — so the caller keeps the connection it inspected,
    /// and only a task that never returned hands nothing back. Such a panic — which
    /// this crate never produces — surfaces as [`Error::Backend`] instead of
    /// unwinding the caller. Under `strict-async` this is the sanctioned runtime
    /// entry point, because it drives the guard-free core rather than the guarded
    /// synchronous method.
    pub async fn status_async<B: MigrationBackend + Send + 'static>(
        &self,
        backend: B,
        migrations: &[Migration],
    ) -> (Result<Vec<MigrationStatus>, Error>, Option<B>) {
        self.status_shared_async(backend, Arc::from(migrations))
            .await
    }

    /// Inspects the status of migrations from a script list the caller already shares
    /// on a dedicated blocking thread.
    ///
    /// [`status_async`](Self::status_async) copies the slice it is handed into such a
    /// list; a caller that owns one passes it here and pays no copy. The read happens
    /// on a `spawn_blocking` thread, the backend comes back with the outcome whether
    /// the read succeeded or failed, and a panic inside the task surfaces as
    /// [`Error::Backend`] instead of unwinding the caller.
    pub(crate) async fn status_shared_async<B: MigrationBackend + Send + 'static>(
        &self,
        mut backend: B,
        migrations: Arc<[Migration]>,
    ) -> (Result<Vec<MigrationStatus>, Error>, Option<B>) {
        let migrator = self.clone();

        let joined = tokio::task::spawn_blocking(move || {
            let statuses = migrator.status_core(&mut backend, &migrations);
            (statuses, backend)
        })
        .await;

        join_offloaded(joined, "status")
    }

    /// Records a baseline for a pre-existing database on a dedicated blocking
    /// thread.
    ///
    /// The write runs on a `spawn_blocking` thread, so no Tokio worker thread
    /// performs the blocking database I/O Conventions.md §3 forbids. The backend is
    /// moved into the blocking task and handed back together with the outcome — a
    /// refused baseline returns it too — so the caller keeps the connection it
    /// baselined, and only a task that never returned hands nothing back. Such a
    /// panic — which this crate never produces — surfaces as [`Error::Backend`]
    /// instead of unwinding the caller. Under `strict-async` this is the sanctioned
    /// runtime entry point, because it drives the guard-free core rather than the
    /// guarded synchronous method.
    pub async fn baseline_async<B: MigrationBackend + Send + 'static>(
        &self,
        mut backend: B,
        version: MigrationVersion,
        description: &str,
    ) -> (Result<SchemaHistoryEntry, Error>, Option<B>) {
        let migrator = self.clone();
        let description = description.to_owned();

        let joined = tokio::task::spawn_blocking(move || {
            let entry = migrator.baseline_core(&mut backend, version, &description);
            (entry, backend)
        })
        .await;

        join_offloaded(joined, "baseline")
    }

    /// Repairs the schema history against updated script checksums and purges failed
    /// entries on a dedicated blocking thread.
    ///
    /// The repair runs on a `spawn_blocking` thread, so no Tokio worker thread
    /// performs the blocking database I/O Conventions.md §3 forbids. The backend is
    /// moved into the blocking task and handed back together with the outcome — a
    /// failed repair returns it too — so the caller keeps the connection it
    /// repaired, and only a task that never returned hands nothing back. Such a
    /// panic — which this crate never produces — surfaces as [`Error::Backend`]
    /// instead of unwinding the caller. Under `strict-async` this is the sanctioned
    /// runtime entry point,
    /// because it drives the guard-free core rather than the guarded synchronous
    /// method.
    pub async fn repair_async<B: MigrationBackend + Send + 'static>(
        &self,
        backend: B,
        migrations: &[Migration],
    ) -> (Result<RepairReport, Error>, Option<B>) {
        self.repair_shared_async(backend, Arc::from(migrations))
            .await
    }

    /// Repairs the schema history against a script list the caller already shares on
    /// a dedicated blocking thread.
    ///
    /// [`repair_async`](Self::repair_async) copies the slice it is handed into such a
    /// list; a caller that owns one passes it here and pays no copy. The repair runs
    /// on a `spawn_blocking` thread, the backend comes back with the outcome whether
    /// the repair succeeded or failed, and a panic inside the task surfaces as
    /// [`Error::Backend`] instead of unwinding the caller.
    pub(crate) async fn repair_shared_async<B: MigrationBackend + Send + 'static>(
        &self,
        mut backend: B,
        migrations: Arc<[Migration]>,
    ) -> (Result<RepairReport, Error>, Option<B>) {
        let migrator = self.clone();

        let joined = tokio::task::spawn_blocking(move || {
            let report = migrator.repair_core(&mut backend, &migrations);
            (report, backend)
        })
        .await;

        join_offloaded(joined, "repair")
    }
}

/// Joins an offloaded lifecycle task, handing the backend back with its outcome.
///
/// The blocking task returns the outcome of a guard-free core together with the
/// backend it drove, so a lifecycle failure keeps the connection usable for the
/// follow-up that failure demands. Only a task that never returned — a panic
/// inside it, or a cancellation — hands nothing back, because the backend is
/// dropped inside it; the join error then travels as [`Error::Backend`] named
/// after `context`, the operation the caller asked for.
pub(crate) fn join_offloaded<T, B>(
    joined: Result<(Result<T, Error>, B), tokio::task::JoinError>,
    context: &str,
) -> (Result<T, Error>, Option<B>) {
    match joined {
        Ok((outcome, backend)) => (outcome, Some(backend)),
        Err(error) => (
            Err(Error::Backend(format!(
                "blocking {context} task failed: {error}"
            ))),
            None,
        ),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::checkpoint::Checkpoint;
    use crate::checksum::Checksum;
    use crate::migration::MigrationKind;
    use crate::runner::{InMemoryBackend, MigrationStatusKind};

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("build tokio runtime")
    }

    fn boundary(version: &str) -> Checkpoint {
        Checkpoint::new(
            version.parse().expect("a valid version"),
            Checksum::of(b"stage"),
        )
    }

    fn sample() -> Vec<Migration> {
        vec![
            Migration::parse("V1__users.sql", "CREATE TABLE users (id INTEGER);")
                .expect("a valid script name"),
            Migration::parse("V2__orders.sql", "CREATE TABLE orders (id INTEGER);")
                .expect("a valid script name"),
        ]
    }

    #[test]
    fn migrate_async_applies_versioned_scripts_in_order() {
        let runtime = runtime();
        let migrations = sample();

        let (outcome, backend) =
            runtime.block_on(Migrator::new().migrate_async(InMemoryBackend::new(), &migrations));
        let report = outcome.expect("the blocking run succeeds");
        let backend = backend.expect("the backend is handed back with the report");

        assert_eq!(report.applied_count, 2);
        assert_eq!(
            report.applied_scripts,
            vec!["V1__users.sql", "V2__orders.sql"]
        );
        assert_eq!(backend.history.len(), 2);
        assert!(backend.history.iter().all(|entry| entry.success));
        assert_eq!(backend.history[0].installed_rank, 1);
        assert_eq!(backend.history[1].installed_rank, 2);
    }

    #[test]
    fn baseline_async_records_the_baseline_and_hands_the_backend_back() {
        let runtime = runtime();
        let version: MigrationVersion = "1.0.0".parse().expect("a valid version");

        let (outcome, backend) = runtime.block_on(Migrator::new().baseline_async(
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

        let (refused, _) =
            runtime.block_on(Migrator::new().baseline_async(backend, version, "too late"));

        match refused {
            Err(Error::BaselineAlreadyExists { version }) => assert_eq!(version, "1.0.0"),
            Err(other) => panic!("expected BaselineAlreadyExists, got {other:?}"),
            Ok(_) => panic!("a second baseline must be rejected"),
        }
    }

    #[test]
    fn repair_async_purges_a_failed_entry() {
        let runtime = runtime();
        let mut backend = InMemoryBackend::new();
        backend.failure_script = Some("BOOM".to_owned());
        let broken =
            vec![Migration::parse("V1__broken.sql", "BOOM;").expect("a valid script name")];

        Migrator::new()
            .migrate(&mut backend, &broken)
            .expect_err("the failing script aborts the run");
        assert_eq!(backend.history.len(), 1);
        assert!(!backend.history[0].success);

        backend.failure_script = None;
        let fixed =
            vec![Migration::parse("V1__broken.sql", "SELECT 1;").expect("a valid script name")];

        let (outcome, backend) = runtime.block_on(Migrator::new().repair_async(backend, &fixed));
        let report = outcome.expect("the blocking repair succeeds");
        let backend = backend.expect("the backend is handed back with the report");

        assert_eq!(report.removed_failed_entries, 1);
        assert!(report.updated_checksums.is_empty());
        assert!(
            backend.history.is_empty(),
            "the purge leaves a clean history behind"
        );
    }

    #[test]
    fn status_async_reports_applied_migrations() {
        let runtime = runtime();
        let migrations = sample();

        let (outcome, backend) =
            runtime.block_on(Migrator::new().migrate_async(InMemoryBackend::new(), &migrations));
        outcome.expect("the blocking run succeeds");
        let backend = backend.expect("the backend is handed back with the report");

        let (outcome, backend) =
            runtime.block_on(Migrator::new().status_async(backend, &migrations));
        let statuses = outcome.expect("the blocking status read succeeds");
        let backend = backend.expect("the backend is handed back with the statuses");

        assert_eq!(statuses.len(), 2);
        assert!(statuses
            .iter()
            .all(|status| status.status == MigrationStatusKind::Applied));
        assert_eq!(backend.history.len(), 2);
    }

    #[test]
    fn a_checkpoint_anchored_runner_is_drivable_from_an_async_startup() {
        let runtime = runtime();
        let stage = vec![
            Migration::parse("V1__stage.sql", "CREATE TABLE users (id INTEGER);")
                .expect("a valid script name"),
        ];

        let (outcome, backend) =
            runtime.block_on(Migrator::new().migrate_async(InMemoryBackend::new(), &stage));
        outcome.expect("the stage migration applies");
        let backend = backend.expect("the backend is handed back with the report");

        let tail = vec![
            Migration::parse("V2__tail.sql", "CREATE TABLE orders (id INTEGER);")
                .expect("a valid script name"),
        ];

        let (refused, _) = runtime
            .block_on(Migrator::with_checkpoint(boundary("2")).migrate_async(backend, &tail));

        match refused {
            Err(Error::OutdatedCheckpoint {
                database_version,
                pruned_up_to_version,
                ..
            }) => {
                assert_eq!(database_version.as_deref(), Some("1"));
                assert_eq!(pruned_up_to_version, "2");
            }
            Err(other) => panic!("expected OutdatedCheckpoint, got {other:?}"),
            Ok(_) => panic!("the outdated target must be rejected"),
        }
    }

    #[test]
    #[cfg(all(feature = "tokio", not(feature = "strict-async")))]
    fn the_synchronous_lifecycle_stays_advisory_inside_a_runtime() {
        let runtime = runtime();
        let migrations = sample();

        let (report, backend) = runtime.block_on(async {
            let mut backend = InMemoryBackend::new();
            let report = Migrator::new()
                .migrate(&mut backend, &migrations)
                .expect("the default guard only warns");
            (report, backend)
        });

        assert_eq!(report.applied_count, 2);
        assert_eq!(backend.history.len(), 2);
        assert!(backend.history.iter().all(|entry| entry.success));
    }

    #[test]
    #[cfg(feature = "strict-async")]
    fn the_async_wrapper_is_the_sanctioned_runtime_entry_point() {
        let runtime = runtime();
        let migrations = sample();

        let mut backend = InMemoryBackend::new();
        let refused =
            runtime.block_on(async { Migrator::new().migrate(&mut backend, &migrations) });

        match refused {
            Err(Error::Backend(_)) => {}
            Err(other) => panic!("expected a blocking-entry refusal, got {other:?}"),
            Ok(_) => panic!("`strict-async` must refuse a synchronous run inside a runtime"),
        }

        let (outcome, backend) =
            runtime.block_on(Migrator::new().migrate_async(InMemoryBackend::new(), &migrations));
        let report = outcome.expect("the blocking wrapper runs the guard-free core");
        let backend = backend.expect("the backend is handed back with the report");

        assert_eq!(report.applied_count, 2);
        assert_eq!(backend.history.len(), 2);
        assert!(backend.history.iter().all(|entry| entry.success));
    }
}
