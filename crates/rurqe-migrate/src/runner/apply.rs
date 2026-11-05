//! Application of a single migration script and its history rows.

use crate::error::Error;
use crate::history::SchemaHistoryEntry;
use crate::migration::Migration;

use super::backend::{with_transaction, MigrationBackend, MigrationTransaction};
use super::guard::LockGuard;
use super::time::current_timestamp;

/// Applies one migration and records its outcome in the schema history.
///
/// A transactional script runs inside the transaction the runner opens, so the
/// script and its history row commit together. A script marked non-transactional
/// is applied outside any transaction, so its history row cannot share the
/// script's commit: an in-progress row is committed before the script runs, and
/// the applied row replaces it afterwards. An interruption between the two leaves
/// the schema changed and the in-progress row behind, which the next run reports
/// as a failed migration and `repair` clears — instead of the work being invisible.
pub(super) fn apply_migration<B: MigrationBackend>(
    guard: &mut LockGuard<'_, B>,
    migration: &Migration,
    installed_rank: i64,
) -> Result<i64, Error> {
    let start_time = std::time::Instant::now();

    if migration.is_transactional() {
        let outcome = with_transaction(guard, |tx| {
            tx.execute(&migration.sql)?;

            let elapsed_ms = start_time.elapsed().as_millis() as i64;
            let entry = SchemaHistoryEntry::applied(
                installed_rank,
                migration,
                current_timestamp(),
                elapsed_ms,
            );
            tx.record_entry(&entry)?;

            Ok(elapsed_ms)
        });

        let elapsed_ms = match outcome {
            Ok(elapsed_ms) => elapsed_ms,
            // The helper rolled the transaction back, so a failing script, a failing
            // history write, and a failing commit all leave the connection clean and
            // keep the migration lock from outliving an open transaction. The failed
            // attempt is recorded best effort, and the propagated error names the
            // script instead of the bare driver diagnostic.
            Err(error) => {
                let elapsed_ms = start_time.elapsed().as_millis() as i64;
                tracing::error!(
                    script = %migration.script,
                    error = %error,
                    "migration failed"
                );
                record_failure(guard, migration, installed_rank, elapsed_ms);
                return Err(execution_failed(migration, &error));
            }
        };

        tracing::info!(
            script = %migration.script,
            elapsed_ms = elapsed_ms,
            "migration applied"
        );

        return Ok(elapsed_ms);
    }

    // The script commits on its own, so nothing would record the attempt if the
    // process died right after it: the in-progress row is committed first, and
    // only its presence makes an interruption visible to the next run, which
    // reports the script as failed and lets `repair` clear it.
    let in_progress = SchemaHistoryEntry::failed(installed_rank, migration, current_timestamp(), 0);
    // The row stops the run before the script reaches the database, and the
    // diagnostic names the script like every neighbouring failure path does.
    record_in_progress(guard, &in_progress).map_err(|error| execution_failed(migration, &error))?;

    let execution_started = std::time::Instant::now();

    let elapsed_ms = match guard.execute_non_transactional(&migration.sql) {
        Ok(()) => execution_started.elapsed().as_millis() as i64,
        // The backend refused the script before it touched the database, so the
        // in-progress row is withdrawn again: a failed entry would demand a repair
        // for a migration that never ran.
        Err(Error::NonTransactionalUnsupported { script }) => {
            discard_in_progress(guard, installed_rank);
            return Err(Error::NonTransactionalUnsupported {
                script: Some(script.unwrap_or_else(|| migration.script.clone())),
            });
        }
        // The in-progress row already records the failed attempt at this rank, so
        // it is left untouched: rewriting it could fail for the very reason the
        // script did, and would drop the only marker of the attempt.
        Err(error) => {
            tracing::error!(
                script = %migration.script,
                error = %error,
                "migration failed"
            );
            return Err(execution_failed(migration, &error));
        }
    };

    let entry =
        SchemaHistoryEntry::applied(installed_rank, migration, current_timestamp(), elapsed_ms);
    // The applied row replaces the in-progress one at the same rank inside the
    // very transaction that writes it, so the two never coexist.
    with_transaction(guard, |tx| {
        tx.delete_entry(installed_rank)?;
        tx.record_entry(&entry)
    })
    .map_err(|error| history_not_recorded(migration, &error))?;
    tracing::info!(
        script = %migration.script,
        elapsed_ms = elapsed_ms,
        transactional = false,
        "migration applied"
    );

    Ok(elapsed_ms)
}

/// Commits the in-progress row of a non-transactional script before it runs.
///
/// The row has to be durable before the script touches the database, so it closes
/// its own transaction instead of sharing the one the applied row is written in.
/// A backend that cannot write the row stops the run before anything is executed.
fn record_in_progress<B: MigrationBackend>(
    guard: &mut LockGuard<'_, B>,
    entry: &SchemaHistoryEntry,
) -> Result<(), Error> {
    with_transaction(guard, |tx| tx.record_entry(entry))
}

/// Withdraws the in-progress row of a script the backend refused to run.
///
/// The refusal proves nothing reached the database, so the marker of an attempt
/// that never happened is removed again. The removal is best effort: the caller
/// already holds the authoritative diagnostic, which must not be replaced by a
/// second error about the marker.
fn discard_in_progress<B: MigrationBackend>(guard: &mut LockGuard<'_, B>, installed_rank: i64) {
    match guard.begin() {
        Ok(mut tx) => {
            if let Err(error) = tx.delete_entry(installed_rank) {
                tracing::warn!(
                    installed_rank = installed_rank,
                    error = %error,
                    "failed to withdraw the in-progress migration entry"
                );
                let _ = tx.rollback();
            } else if let Err(error) = tx.commit() {
                tracing::warn!(
                    installed_rank = installed_rank,
                    error = %error,
                    "failed to commit the withdrawal of the in-progress migration entry"
                );
            }
        }
        Err(error) => tracing::warn!(
            installed_rank = installed_rank,
            error = %error,
            "failed to open the transaction withdrawing the in-progress migration entry"
        ),
    }
}

/// Builds the diagnostic of a non-transactional script whose applied row was lost.
fn history_not_recorded(migration: &Migration, error: &Error) -> Error {
    tracing::error!(
        script = %migration.script,
        error = %error,
        "failed to record the applied history entry of a non-transactional migration"
    );

    Error::HistoryNotRecorded {
        script: migration.script.clone(),
        source: error.to_string(),
    }
}

/// Records a failed migration attempt, best effort.
///
/// The failure is reported to the caller already, so a backend that cannot write
/// the row must not replace the original diagnostic with a second error.
fn record_failure<B: MigrationBackend>(
    guard: &mut LockGuard<'_, B>,
    migration: &Migration,
    installed_rank: i64,
    elapsed_ms: i64,
) {
    let entry =
        SchemaHistoryEntry::failed(installed_rank, migration, current_timestamp(), elapsed_ms);

    match guard.begin() {
        Ok(mut tx) => {
            if let Err(error) = tx.record_entry(&entry) {
                tracing::warn!(
                    script = %migration.script,
                    error = %error,
                    "failed to record failed migration entry"
                );
                let _ = tx.rollback();
            } else if let Err(error) = tx.commit() {
                tracing::warn!(
                    script = %migration.script,
                    error = %error,
                    "failed to commit failed migration history entry"
                );
            }
        }
        Err(error) => {
            tracing::warn!(
                script = %migration.script,
                error = %error,
                "failed to open transaction to record failed migration"
            );
        }
    }
}

/// Wraps `error` into the failure the caller sees for `migration`.
fn execution_failed(migration: &Migration, error: &Error) -> Error {
    Error::MigrationExecutionFailed {
        script: migration.script.clone(),
        source: error.to_string(),
    }
}
