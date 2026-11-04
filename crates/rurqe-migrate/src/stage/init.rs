//! Cold start of an empty database from the declarative stage.

use crate::error::Error;
use crate::history::SchemaHistoryEntry;
use crate::runner::{current_timestamp, LockGuard, MigrationBackend, MigrationTransaction};
use crate::sleeper::guard_blocking_entry;

use super::{DeclarativeStage, StageMarker, StageScript};

/// Summary of a cold start run.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ColdStartReport {
    /// Names of the DDL scripts applied in this run, in application order.
    pub stage_scripts: Vec<String>,
    /// The rank the stage marker was recorded at, `None` when it already existed.
    pub installed_rank: Option<i64>,
    /// Total execution time of the DDL scripts in milliseconds.
    pub total_time_ms: i64,
}

/// Initialises an empty database from the declarative stage.
///
/// The transactional DDL scripts of `stage` share one transaction with the
/// synthetic [`StageMarker`] row that anchors them — a failure among the
/// scripts, a failing catalog read, or an unwritable marker discards the whole
/// attempt — while a script marked non-transactional runs on its own, outside
/// any transaction. Because the marker
/// is recorded as a baseline, every versioned migration at or below
/// [`StageMarker::version`] is reported as applied while the active tail above
/// it still runs.
///
/// The digest the marker anchors is the one its own transaction derives from the
/// catalog the stage produced — see [`MigrationTransaction::catalog_checksum`] —
/// so the row and the schema it describes become visible together, and the digest
/// matches the checkpoint `stage commit` writes whatever engine executed the DDL.
/// A backend that cannot introspect itself keeps the digest of the marker.
///
/// A database carrying nothing but a stage marker for the same version and the
/// same catalog digest is left untouched and reported as a no-op; any other
/// history is rejected, because a cold start never overwrites an initialised
/// database. A marker the migration tail advanced past is one such history: the
/// database it describes is initialised, whatever the live catalog reports.
///
/// A stage holding a script marked non-transactional cannot be applied
/// atomically, so the cold start commits an in-progress marker before the first
/// of those scripts and replaces it with the applied marker once every script has
/// run. A run interrupted in between leaves the in-progress marker behind, and
/// the next attempt reports the database as partially initialised instead of
/// treating it as an unknown schema or as one that already carries history.
///
/// A backend that cannot execute a script outside a transaction refuses it
/// before the database is reached. When the refused script is the first of the
/// stage, the in-progress marker is withdrawn again and the refusal is reported
/// as [`Error::NonTransactionalUnsupported`] naming that script, so an attempt
/// that never started leaves no history behind; a stage that already applied an
/// earlier script keeps the marker, because its schema really is partially
/// initialised.
///
/// The call blocks the calling thread for as long as the DDL takes, so it guards
/// the blocking entry described in the module documentation: the call warns by
/// default and is refused under the `strict-async` feature. An asynchronous
/// startup path uses `cold_start_async` instead, which runs the very same work on
/// a dedicated blocking thread.
pub fn cold_start<B: MigrationBackend>(
    backend: &mut B,
    stage: &DeclarativeStage,
    marker: &StageMarker,
) -> Result<ColdStartReport, Error> {
    guard_blocking_entry("cold_start")?;
    cold_start_core(backend, stage, marker)
}

/// Guard-free core of [`cold_start`].
///
/// The asynchronous entry points — [`cold_start_async`] and the lifecycle
/// protocol of [`EmbeddedMigrations`](crate::embedded::EmbeddedMigrations) — run
/// it on a `spawn_blocking` thread, where the blocking-entry guard cannot tell a
/// blocking-pool thread from a worker thread and would refuse work that is
/// exactly where it belongs.
pub(crate) fn cold_start_core<B: MigrationBackend>(
    backend: &mut B,
    stage: &DeclarativeStage,
    marker: &StageMarker,
) -> Result<ColdStartReport, Error> {
    let mut guard = LockGuard::acquire(backend)?;
    guard.ensure_schema_history_table()?;
    let history = guard.read_history()?;

    let recorded = history
        .iter()
        .filter(|entry| entry.success && entry.script == super::STAGE_SCRIPT)
        .max_by_key(|entry| entry.installed_rank);

    if let Some(existing) = recorded {
        // The migration tail advanced the database past the marker, so the
        // history carries more than the stage it was initialised from. That is
        // the same "already initialised" state as any other recorded history,
        // and it is classified before the live catalog is read: comparing the
        // catalog first reported a healthy, fully migrated database as a
        // diverged stage, while a backend that cannot introspect itself reported
        // the very same history as an untouched stage, so the outcome followed
        // the capability of the backend instead of the state of the schema.
        if let Some(later) = history
            .iter()
            .filter(|entry| entry.installed_rank != existing.installed_rank)
            .max_by_key(|entry| entry.installed_rank)
        {
            return Err(already_initialised(later));
        }

        // Nothing but the marker is recorded, so the database still describes
        // the stage it was initialised from. The backend reports the digest of
        // the catalog really present, so a database whose schema drifted from
        // the recorded marker is rejected instead of being reported as
        // initialised. A backend that cannot introspect itself keeps the digest
        // the caller supplied.
        let current = match guard.catalog_checksum() {
            Ok(Some(checksum)) => checksum,
            Ok(None) => marker.checksum(),
            Err(error) => return Err(error),
        };

        if existing.checksum == Some(current) && existing.version.as_ref() == Some(marker.version())
        {
            tracing::debug!(version = %marker.version(), "stage is already recorded");
            return Ok(ColdStartReport::default());
        }

        // The database was initialised from a stage that no longer matches the
        // one on disk, so the two digests name different schemas and a cold start
        // would silently overwrite an initialised database.
        return Err(stage_error(format!(
            "the database already carries a stage marker for version `{}` with a diverged digest; \
             re-initialise it from scratch or commit the current stage first",
            existing
                .version
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_default()
        )));
    }

    // A cold start that died while running a non-transactional script left the
    // in-progress marker it committed before the script ran, so the database is
    // reported as partially initialised instead of as one already carrying history.
    if let Some(interrupted) = history
        .iter()
        .filter(|entry| !entry.success && entry.script == super::STAGE_SCRIPT)
        .max_by_key(|entry| entry.installed_rank)
    {
        return Err(partial_initialisation(interrupted));
    }
    if let Some(existing) = history.iter().max_by_key(|entry| entry.installed_rank) {
        return Err(already_initialised(existing));
    }

    let start_time = std::time::Instant::now();

    // A stage holding a non-transactional script is applied outside any
    // transaction, so nothing would record the attempt if the process died
    // midway. The in-progress marker is committed first and replaced by the
    // applied one at the end, so an interrupted cold start stays visible in the
    // history instead of leaving a schema nobody can name. A fully transactional
    // stage needs none of that: its scripts and the marker that records them
    // share one transaction, so the database carries both or neither of them.
    let in_progress = stage
        .scripts()
        .iter()
        .any(|script| !script.is_transactional());
    let in_progress_rank = if in_progress {
        let entry = marker.entry(1, current_timestamp(), marker.checksum(), false);
        record_marker(&mut guard, &entry)?;
        Some(entry.installed_rank)
    } else {
        None
    };

    // The marker anchors the digest of the catalog the stage produced, so the
    // checkpoint written by `stage commit` recognises the initialised database
    // whatever engine executed the DDL. The digest is read inside the very
    // transaction that records the marker, so the row and the schema it
    // describes become visible together; a backend that cannot introspect itself
    // keeps the digest the caller supplied.
    let entry = match in_progress_rank {
        Some(installed_rank) => {
            apply_stage(&mut guard, stage, installed_rank)?;
            record_applied_marker(&mut guard, marker, installed_rank)?
        }
        None => apply_stage_atomically(&mut guard, stage, marker)?,
    };

    let elapsed_ms = start_time.elapsed().as_millis() as i64;

    tracing::info!(
        version = %marker.version(),
        scripts = stage.scripts().len(),
        elapsed_ms = elapsed_ms,
        "initialised the database from the declarative stage"
    );

    Ok(ColdStartReport {
        stage_scripts: stage
            .scripts()
            .iter()
            .map(|script| script.name().to_owned())
            .collect(),
        installed_rank: Some(entry.installed_rank),
        total_time_ms: elapsed_ms,
    })
}

/// Initialises an empty database from the declarative stage on a blocking thread.
///
/// This is the asynchronous counterpart of [`cold_start`]: the stage is applied on
/// a dedicated blocking thread, so the runtime's worker threads stay free for as
/// long as the DDL takes. The backend is moved into the blocking task and handed
/// back together with the report, so a connection that cannot be shared across
/// threads stays usable after the run. A panic inside the blocking task — which
/// this crate never produces — surfaces as [`Error::Backend`] instead of unwinding
/// the caller.
///
/// The task runs the guard-free core of the lifecycle, so it never trips the
/// `strict-async` refusal: a blocking-pool thread is exactly where this work
/// belongs, which makes this the sanctioned runtime path.
#[cfg(feature = "tokio")]
pub async fn cold_start_async<B: MigrationBackend + Send + 'static>(
    mut backend: B,
    stage: &DeclarativeStage,
    marker: &StageMarker,
) -> Result<(ColdStartReport, B), Error> {
    let stage = stage.clone();
    let marker = marker.clone();

    let joined = tokio::task::spawn_blocking(move || {
        let report = cold_start_core(&mut backend, &stage, &marker);
        (report, backend)
    })
    .await;

    match joined {
        Ok((report, backend)) => report.map(|report| (report, backend)),
        Err(error) => Err(Error::Backend(format!(
            "blocking cold start task failed: {error}"
        ))),
    }
}

/// Writes `entry` in a transaction of its own.
///
/// The in-progress marker of a stage holding a non-transactional script is
/// committed before the first such script runs and replaced by the applied one
/// afterwards, so an interrupted cold start stays visible in the history.
fn record_marker<B: MigrationBackend>(
    guard: &mut LockGuard<'_, B>,
    entry: &SchemaHistoryEntry,
) -> Result<(), Error> {
    let mut tx = guard.begin()?;

    if let Err(error) = tx.record_entry(entry) {
        let _ = tx.rollback();
        return Err(error);
    }

    tx.commit()
}

/// Records the applied stage marker, anchoring the digest its transaction observes.
///
/// The catalog digest is read through
/// [`MigrationTransaction::catalog_checksum`] inside the very transaction that
/// writes the row, so the marker and the schema it describes become visible
/// together. A backend that cannot introspect itself reports nothing, which
/// keeps the digest of the marker.
///
/// `replaces` names the rank of the in-progress marker the write supersedes: both
/// rows describe the same initialisation, so the superseded one is dropped inside
/// the same transaction instead of leaving a failed entry next to the applied one.
fn record_applied_marker<B: MigrationBackend>(
    guard: &mut LockGuard<'_, B>,
    marker: &StageMarker,
    replaces: i64,
) -> Result<SchemaHistoryEntry, Error> {
    let mut tx = guard.begin()?;

    let checksum = match tx.catalog_checksum() {
        Ok(Some(checksum)) => checksum,
        Ok(None) => marker.checksum(),
        Err(error) => {
            let _ = tx.rollback();
            return Err(error);
        }
    };

    let entry = marker.entry(1, current_timestamp(), checksum, true);

    if let Err(error) = tx.delete_entry(replaces) {
        let _ = tx.rollback();
        return Err(error);
    }

    if let Err(error) = tx.record_entry(&entry) {
        let _ = tx.rollback();
        return Err(error);
    }

    tx.commit()?;

    Ok(entry)
}

/// Applies a fully transactional stage and records its marker in one transaction.
///
/// None of the scripts of such a stage has to run outside a transaction, so the
/// DDL and the row that records it share one: a failing script, a failing catalog
/// read, an unwritable marker, or a failing commit discards the whole attempt, and
/// the database stays as cold as it was instead of holding a schema the history
/// knows nothing about.
///
/// The digest is read through [`MigrationTransaction::catalog_checksum`] once
/// every script has been applied and inside the very transaction that records the
/// marker, so the row anchors the catalog the stage produced under the engine that
/// produced it and becomes visible together with it; a backend that cannot
/// introspect itself keeps the digest of the marker.
fn apply_stage_atomically<B: MigrationBackend>(
    guard: &mut LockGuard<'_, B>,
    stage: &DeclarativeStage,
    marker: &StageMarker,
) -> Result<SchemaHistoryEntry, Error> {
    let mut tx = guard.begin()?;

    for script in stage.scripts() {
        if let Err(error) = tx.execute(script.sql()) {
            let _ = tx.rollback();
            return Err(stage_script_failed(script, &error));
        }
    }

    let checksum = match tx.catalog_checksum() {
        Ok(Some(checksum)) => checksum,
        Ok(None) => marker.checksum(),
        Err(error) => {
            let _ = tx.rollback();
            return Err(error);
        }
    };

    let entry = marker.entry(1, current_timestamp(), checksum, true);

    if let Err(error) = tx.record_entry(&entry) {
        let _ = tx.rollback();
        return Err(error);
    }

    tx.commit()?;

    Ok(entry)
}

/// Withdraws the in-progress marker of a stage the backend refused to run.
///
/// The refusal proves nothing reached the database, so the marker of a cold
/// start that applied no script is removed again instead of making every later
/// attempt report a partial initialisation. The removal is best effort: the
/// caller already holds the authoritative diagnostic, which must not be replaced
/// by a second error about the marker.
fn withdraw_marker<B: MigrationBackend>(guard: &mut LockGuard<'_, B>, installed_rank: i64) {
    match guard.begin() {
        Ok(mut tx) => {
            if let Err(error) = tx.delete_entry(installed_rank) {
                tracing::warn!(
                    installed_rank = installed_rank,
                    error = %error,
                    "failed to withdraw the in-progress stage marker"
                );
                let _ = tx.rollback();
            } else if let Err(error) = tx.commit() {
                tracing::warn!(
                    installed_rank = installed_rank,
                    error = %error,
                    "failed to commit the withdrawal of the in-progress stage marker"
                );
            }
        }
        Err(error) => tracing::warn!(
            installed_rank = installed_rank,
            error = %error,
            "failed to open the transaction withdrawing the in-progress stage marker"
        ),
    }
}

/// Builds the diagnostic of an initialised database a cold start never overwrites.
///
/// The newest recorded entry names the state the operator has to inspect: its
/// version, or its script name when the entry carries none.
fn already_initialised(entry: &SchemaHistoryEntry) -> Error {
    Error::BaselineAlreadyExists {
        version: entry
            .version
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_else(|| entry.script.clone()),
    }
}

/// Builds the diagnostic of a cold start interrupted while applying the stage.
fn partial_initialisation(entry: &SchemaHistoryEntry) -> Error {
    let version = entry
        .version
        .as_ref()
        .map(ToString::to_string)
        .unwrap_or_default();

    tracing::error!(version = %version, "found an interrupted cold start");

    stage_error(format!(
        "an earlier cold start was interrupted while applying stage `{version}`: the database \
         carries the in-progress marker recorded before its non-transactional scripts ran, so it \
         may hold a partially initialised schema; `repair` clears the marker, and the database \
         itself is re-initialised from scratch before the cold start runs again"
    ))
}

/// Applies a stage holding at least one non-transactional script.
///
/// Consecutive transactional scripts share one transaction, so a failure among
/// them leaves the database untouched. A script marked non-transactional commits
/// the work staged so far and then runs on its own: engines refuse such
/// statements inside a transaction, so wrapping them would abort the cold start
/// on a perfectly valid stage. Such a stage cannot share one boundary with the
/// marker, which follows in a transaction of its own; a fully transactional
/// stage goes through [`apply_stage_atomically`] instead.
///
/// `in_progress_rank` names the marker a cold start committed before the first
/// non-transactional script. A backend that cannot run such a script refuses it
/// before it reaches the database, which leaves the database exactly as cold as
/// it was: when the refused script is the first of the stage, the marker of an
/// attempt that applied nothing is withdrawn instead of being left to report a
/// partial initialisation on every later run.
fn apply_stage<B: MigrationBackend>(
    guard: &mut LockGuard<'_, B>,
    stage: &DeclarativeStage,
    in_progress_rank: i64,
) -> Result<(), Error> {
    let scripts = stage.scripts();
    let mut index = 0usize;

    while let Some(script) = scripts.get(index) {
        if !script.is_transactional() {
            match guard.execute_non_transactional(script.sql()) {
                Ok(()) => {}
                // The refusal proves nothing reached the database, so the
                // capability gap travels under its own identity and names the
                // script instead of masquerading as a failed migration.
                Err(Error::NonTransactionalUnsupported { .. }) => {
                    // Withdrawing the marker is only honest while nothing of the
                    // stage has been applied: an earlier script proves the schema
                    // really is partially initialised, and then the marker has to
                    // stay.
                    if index == 0 {
                        withdraw_marker(guard, in_progress_rank);
                    }
                    return Err(Error::NonTransactionalUnsupported {
                        script: Some(script.name().to_owned()),
                    });
                }
                Err(error) => return Err(stage_script_failed(script, &error)),
            }
            index += 1;
            continue;
        }

        let start = index;
        while matches!(scripts.get(index), Some(next) if next.is_transactional()) {
            index += 1;
        }

        let batch = scripts.get(start..index).unwrap_or_default();
        let mut tx = guard.begin()?;

        for script in batch {
            if let Err(error) = tx.execute(script.sql()) {
                let _ = tx.rollback();
                return Err(stage_script_failed(script, &error));
            }
        }

        tx.commit()?;
    }

    Ok(())
}

/// Builds the failure a stage script is reported with.
fn stage_script_failed(script: &StageScript, error: &Error) -> Error {
    tracing::error!(script = %script.name(), error = %error, "stage script failed");
    Error::MigrationExecutionFailed {
        script: script.name().to_owned(),
        source: error.to_string(),
    }
}

/// Builds an [`Error::InvalidStage`] from `message`.
fn stage_error(message: impl Into<String>) -> Error {
    Error::InvalidStage {
        message: message.into(),
    }
}
