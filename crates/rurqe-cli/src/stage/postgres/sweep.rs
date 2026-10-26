//! Reaping of the scratch databases an interrupted verification left behind.
//!
//! A verification that dies before it drops its database — a killed process, a
//! hard timeout — leaves the scratch behind, so every name records the second it
//! was created, and the first sandbox connection of an interval sweeps away the
//! databases abandoned for longer than [`DEFAULT_SCRATCH_MAX_AGE`]. A sweep only
//! reaps databases this host tagged as its own, so a shared server never loses
//! another host's in-flight work.
//!
//! `stage cleanup` runs the very candidate selection the sweep runs, so the list
//! it asks the operator to confirm is exactly the list the sweep would drop.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use crate::cli::redact_database_url;

use super::naming::{current_owner_tag, epoch_seconds, is_scratch_database_candidate};
use super::sandbox::drop_scratch_database;

/// The age after which an abandoned scratch database is swept away.
pub(crate) const DEFAULT_SCRATCH_MAX_AGE: Duration = Duration::from_secs(3_600);

/// The scratch databases and their object descriptions the server currently holds.
///
/// The pattern narrows the catalog scan to the names this module creates, while
/// the prefix and owner comment are checked again on every row, so an underscore
/// standing for any single character in `LIKE` costs nothing.
const SCRATCH_DATABASES_SQL: &str = "SELECT d.datname, shobj_description(d.oid, 'pg_database') \
     FROM pg_database d \
     WHERE d.datname LIKE 'rurqe_scratch_%' AND d.datname <> current_database() \
       AND d.datdba = (SELECT oid FROM pg_roles WHERE rolname = current_user) \
     ORDER BY d.datname";

/// The minimum spacing between the opportunistic sweeps ahead of a connect.
///
/// A verification opens one sandbox connection per materialised side, so
/// sweeping on every connect would repeat the catalog scan three times for one
/// `stage verify`; the interval lets the first connect of a run sweep and the
/// rest go straight to their scratch database.
pub(super) const OPPORTUNISTIC_SWEEP_INTERVAL: Duration = Duration::from_secs(60);

/// The second of the epoch at which the last opportunistic sweep was attempted.
static LAST_OPPORTUNISTIC_SWEEP: AtomicU64 = AtomicU64::new(0);

/// Sweeps abandoned scratch databases ahead of a connect, at most once per
/// [`OPPORTUNISTIC_SWEEP_INTERVAL`].
///
/// The attempt is recorded before the sweep runs, so a sweep that failed waits
/// out the interval instead of being retried by the very next connect; the
/// connect itself is never blocked, because a refused sweep is only reported
/// through the `tracing` facade.
///
/// A clock that predates the epoch yields no attempt at all: without a reading
/// there is no way to tell an abandoned scratch database from one another host
/// created a moment ago, so nothing is scanned and nothing is recorded, and the
/// first connect that does read the clock starts the interval itself instead of
/// compensating for the connects that could not.
pub(super) fn sweep_opportunistically(admin: &mut postgres::Client, url: &str) {
    let Some(seconds) = epoch_seconds() else {
        tracing::debug!(
            "the clock predates the epoch; deferring the sweep of stale postgres scratch databases"
        );
        return;
    };

    let last = LAST_OPPORTUNISTIC_SWEEP.load(Ordering::Relaxed);

    if !opportunistic_sweep_due(last, seconds) {
        return;
    }

    if LAST_OPPORTUNISTIC_SWEEP
        .compare_exchange(last, seconds, Ordering::Relaxed, Ordering::Relaxed)
        .is_err()
    {
        // Another connection claimed the sweep of this instant, so this one
        // connects without repeating the catalog scan behind it.
        return;
    }

    match sweep_stale_scratch_databases(admin, DEFAULT_SCRATCH_MAX_AGE, url) {
        Ok(dropped) if dropped.is_empty() => {}
        Ok(dropped) => {
            tracing::debug!(
                count = dropped.len(),
                "swept stale postgres scratch databases ahead of a scratch connect"
            );
        }
        Err(error) => {
            tracing::warn!(
                %error,
                "failed to sweep stale postgres scratch databases ahead of a scratch connect"
            );
        }
    }
}

/// Whether an opportunistic sweep is due at `now` after the last attempt at `last`.
///
/// A clock that ran backwards never makes a sweep due, so a broken clock cannot
/// turn the throttle into a scan on every connect.
pub(super) fn opportunistic_sweep_due(last: u64, now: u64) -> bool {
    now.saturating_sub(last) >= OPPORTUNISTIC_SWEEP_INTERVAL.as_secs()
}

/// One scratch database the server catalog reports, with its owner comment.
#[derive(Clone)]
pub(crate) struct ScratchCatalogEntry {
    /// The name of the scratch database.
    pub(crate) name: String,
    /// The owner marker its database comment carries, if it carries one.
    pub(crate) description: Option<String>,
}

/// The admin session a scratch database sweep runs through.
///
/// Keeping the session behind a small trait lets `stage cleanup` drop exactly
/// the candidate list it read for its confirmation — one catalog scan per
/// command — and lets tests count the scans and drops a cleanup performs.
pub(crate) trait ScratchAdminSession {
    /// Lists the scratch databases of the connected role with their comments.
    fn scratch_catalog(&mut self) -> Result<Vec<ScratchCatalogEntry>, String>;

    /// Drops the stale scratch database `name`, reporting whether it is gone.
    fn drop_scratch(&mut self, name: &str) -> bool;
}

impl ScratchAdminSession for postgres::Client {
    fn scratch_catalog(&mut self) -> Result<Vec<ScratchCatalogEntry>, String> {
        let rows = self
            .query(SCRATCH_DATABASES_SQL, &[])
            .map_err(|error| format!("failed to list the scratch databases: {error}"))?;

        rows.into_iter()
            .map(|row| {
                let name: String = row
                    .try_get(0)
                    .map_err(|error| format!("failed to read a scratch database name: {error}"))?;
                let description: Option<String> = row.try_get(1).map_err(|error| {
                    format!("failed to read a scratch database description: {error}")
                })?;

                Ok(ScratchCatalogEntry { name, description })
            })
            .collect()
    }

    fn drop_scratch(&mut self, name: &str) -> bool {
        // The sweep never forces a drop: a scratch database the server refuses
        // to release is simply retried on a later run, so a verification can
        // never terminate sessions on a database it did not abandon itself.
        drop_scratch_database(self, name, false)
    }
}

/// Lists the scratch databases matching the owner marker and abandoned for longer than `max_age`.
pub(crate) fn find_stale_scratch_databases<S: ScratchAdminSession + ?Sized>(
    session: &mut S,
    max_age: Duration,
    expected_owner: &str,
) -> Result<Vec<String>, String> {
    let now = SystemTime::now();
    let mut candidates = Vec::new();

    for entry in session.scratch_catalog()? {
        if !is_scratch_database_candidate(
            &entry.name,
            entry.description.as_deref(),
            expected_owner,
            max_age,
            now,
        ) {
            continue;
        }

        candidates.push(entry.name);
    }

    Ok(candidates)
}

/// Drops the stale scratch databases the candidate list names, once each.
///
/// Returns the names of the databases that are gone afterwards. A database the
/// server refuses to drop is reported and skipped, because a sweep running
/// ahead of a verification must never block it.
pub(crate) fn drop_stale_scratch_databases<S: ScratchAdminSession + ?Sized>(
    session: &mut S,
    candidates: &[String],
    source: &str,
) -> Vec<String> {
    let redacted_source = redact_database_url(source);
    let mut dropped = Vec::new();

    for name in candidates {
        if session.drop_scratch(name) {
            tracing::debug!(
                url = %redacted_source,
                database = %name,
                "dropped a stale postgres scratch database"
            );
            dropped.push(name.clone());
        } else {
            tracing::warn!(
                url = %redacted_source,
                database = %name,
                "failed to drop a stale postgres scratch database"
            );
        }
    }

    dropped
}

/// Drops the scratch databases abandoned for longer than `max_age`.
///
/// Returns the names of the databases that are gone afterwards. A database the
/// server refuses to drop is reported and skipped, because a sweep running
/// ahead of a verification must never block it.
fn sweep_stale_scratch_databases<S: ScratchAdminSession + ?Sized>(
    session: &mut S,
    max_age: Duration,
    source: &str,
) -> Result<Vec<String>, String> {
    let owner_tag = current_owner_tag();
    let candidates = find_stale_scratch_databases(session, max_age, &owner_tag)?;

    if candidates.is_empty() {
        return Ok(Vec::new());
    }

    tracing::debug!(
        url = %redact_database_url(source),
        count = candidates.len(),
        databases = ?candidates,
        "sweeping stale postgres scratch databases"
    );

    Ok(drop_stale_scratch_databases(session, &candidates, source))
}
