//! [`MigrationBackend`] over a `postgres` client.

use std::sync::Arc;
use std::time::{Duration, Instant};

use rurqe_migrate::catalog::SchemaCatalog;
use rurqe_migrate::checksum::Checksum;
use rurqe_migrate::error::Error;
use rurqe_migrate::history::{create_schema_history_table_sql, SchemaHistoryEntry};
use rurqe_migrate::runner::{MigrationBackend, MigrationTransaction};
use rurqe_migrate::sleeper::{warn_if_in_tokio_runtime, Sleeper, ThreadSleeper};

use crate::introspect::{introspect_postgres_schema, DEFAULT_SCHEMA};

use super::history_sql::{
    backend_error, delete_entry_sql, insert_entry_sql, select_history_sql, update_checksum_sql,
    RawHistoryRow, HISTORY_COLUMN_COUNT,
};

/// Advisory lock key guarding a PostgreSQL migration run.
pub(super) const MIGRATION_LOCK_ID: i64 = 0x5255_5251_455f_4d49;

/// Time a migration run waits for the PostgreSQL advisory lock.
const DEFAULT_ADVISORY_LOCK_TIMEOUT: Duration = Duration::from_secs(30);

/// Delay between two attempts to take the PostgreSQL advisory lock.
const ADVISORY_LOCK_RETRY_INTERVAL: Duration = Duration::from_millis(250);

/// Polls the advisory lock `lock_id` until one round takes it or the wait expires.
///
/// The loop carries no driver call of its own: `attempt` reports whether one
/// round took the lock, so its behaviour — an immediate acquisition, a lock that
/// another runner frees later, and a wait that runs out — is verified without a
/// PostgreSQL daemon. A failing round is reported unchanged, because a lost
/// connection cannot be healed by waiting.
///
/// The pause between two rounds is delegated to `sleeper`, so a caller picks
/// whether the wait blocks the thread ([`ThreadSleeper`]) or performs no pause at
/// all (`rurqe_migrate::sleeper::NoopSleeper`) in tests and dry runs.
pub(super) fn poll_advisory_lock<F>(
    lock_id: i64,
    timeout: Duration,
    interval: Duration,
    sleeper: &dyn Sleeper,
    mut attempt: F,
) -> Result<usize, Error>
where
    F: FnMut() -> Result<bool, Error>,
{
    let start = Instant::now();
    let mut attempts = 0usize;

    loop {
        attempts += 1;
        if attempt()? {
            return Ok(attempts);
        }

        let elapsed = start.elapsed();
        if elapsed >= timeout {
            let lock_hex = format!("0x{lock_id:016x}");
            return Err(Error::Backend(format!(
                "could not acquire the PostgreSQL advisory lock {lock_id} ({lock_hex}) \
                 within {timeout:?} ({attempts} attempt(s), waited {elapsed:?}): another \
                 migration runner may be holding it; retry once that run finishes"
            )));
        }

        sleeper.sleep(interval.min(timeout.saturating_sub(elapsed)));
    }
}

/// [`MigrationBackend`] over a `postgres` client.
///
/// The adapter is synchronous and blocking: it owns a `postgres::Client` whose
/// calls run on the calling thread, and
/// [`acquire_lock`](MigrationBackend::acquire_lock) polls the advisory lock by
/// pausing between attempts. Invoke it from a blocking context —
/// `tokio::task::spawn_blocking`, a dedicated thread, or plain synchronous code —
/// never directly on an asynchronous worker thread. [`Self::with_sleeper`]
/// replaces the default [`ThreadSleeper`] when a caller supplies its own pause
/// strategy.
pub struct PostgresMigrationBackend {
    client: postgres::Client,
    schema: String,
    lock_held: bool,
    lock_timeout: Duration,
    sleeper: Arc<dyn Sleeper>,
}

impl PostgresMigrationBackend {
    /// Wraps an open PostgreSQL client.
    pub fn new(client: postgres::Client) -> Self {
        Self {
            client,
            schema: DEFAULT_SCHEMA.to_owned(),
            lock_held: false,
            lock_timeout: DEFAULT_ADVISORY_LOCK_TIMEOUT,
            sleeper: Arc::new(ThreadSleeper),
        }
    }

    /// Selects the PostgreSQL schema the recorded catalog digest describes.
    ///
    /// `public` suits a deployment keeping its tables in the default schema,
    /// while a project owning a schema of its own names it here, so the digest a
    /// migration records describes the very tables the declarative stage
    /// declares instead of an unrelated `public` catalog.
    pub fn with_schema(mut self, schema: impl Into<String>) -> Self {
        self.schema = schema.into();
        self
    }

    /// The PostgreSQL schema the recorded catalog digest describes.
    pub fn schema(&self) -> &str {
        &self.schema
    }

    /// Overrides how long a run waits for the advisory lock before giving up.
    ///
    /// A migration that cannot take the lock within the timeout is rejected with
    /// a diagnostic naming the lock, so a runner blocked by a stuck peer fails
    /// fast instead of hanging forever.
    pub fn with_lock_timeout(mut self, timeout: Duration) -> Self {
        self.lock_timeout = timeout;
        self
    }

    /// Replaces the strategy pausing execution between two advisory lock rounds.
    ///
    /// The default [`ThreadSleeper`] blocks the calling thread, which suits a
    /// synchronous runner. A caller that already drives its own scheduler — or a
    /// test asserting how often the wait happened — supplies its own
    /// implementation instead.
    pub fn with_sleeper(mut self, sleeper: impl Sleeper + 'static) -> Self {
        self.sleeper = Arc::new(sleeper);
        self
    }

    /// Introspects the schema catalog of the selected schema.
    pub fn introspect_catalog(&mut self) -> Result<SchemaCatalog, Error> {
        let schema = self.schema.clone();
        introspect_postgres_schema(&mut self.client, &schema).map_err(backend_error)
    }
}

impl MigrationBackend for PostgresMigrationBackend {
    type Transaction<'a> = PostgresMigrationTransaction<'a>;

    fn ensure_schema_history_table(&mut self) -> Result<(), Error> {
        self.client
            .batch_execute(create_schema_history_table_sql())
            .map_err(backend_error)
    }

    fn read_history(&mut self) -> Result<Vec<SchemaHistoryEntry>, Error> {
        let rows = self
            .client
            .query(&select_history_sql(), &[])
            .map_err(backend_error)?;

        let mut history = Vec::with_capacity(rows.len());
        for row in &rows {
            let raw = RawHistoryRow {
                installed_rank: row.try_get(0).map_err(backend_error)?,
                version: row.try_get(1).map_err(backend_error)?,
                description: row.try_get(2).map_err(backend_error)?,
                kind: row.try_get(3).map_err(backend_error)?,
                script: row.try_get(4).map_err(backend_error)?,
                checksum: row.try_get(5).map_err(backend_error)?,
                installed_on: row.try_get(6).map_err(backend_error)?,
                execution_time_ms: row.try_get(7).map_err(backend_error)?,
                success: row.try_get(8).map_err(backend_error)?,
            };
            history.push(raw.decode()?);
        }

        Ok(history)
    }

    fn catalog_checksum(&mut self) -> Result<Option<Checksum>, Error> {
        self.introspect_catalog()
            .map(|catalog| Some(catalog.checksum()))
    }

    fn begin(&mut self) -> Result<Self::Transaction<'_>, Error> {
        let schema = self.schema.clone();
        let tx = self.client.transaction().map_err(backend_error)?;
        Ok(PostgresMigrationTransaction {
            tx: Some(tx),
            schema,
        })
    }

    fn execute_non_transactional(&mut self, sql: &str) -> Result<(), Error> {
        self.client.batch_execute(sql).map_err(backend_error)
    }

    fn acquire_lock(&mut self) -> Result<(), Error> {
        warn_if_in_tokio_runtime("PostgresMigrationBackend::acquire_lock");
        let lock_id = MIGRATION_LOCK_ID;
        let timeout = self.lock_timeout;

        tracing::debug!(
            lock_id,
            timeout = ?timeout,
            "acquiring the PostgreSQL advisory lock"
        );

        let sleeper = Arc::clone(&self.sleeper);
        let client = &mut self.client;
        let outcome = poll_advisory_lock(
            lock_id,
            timeout,
            ADVISORY_LOCK_RETRY_INTERVAL,
            sleeper.as_ref(),
            || {
                let row = client
                    .query_one("SELECT pg_try_advisory_lock($1)", &[&lock_id])
                    .map_err(backend_error)?;
                row.try_get(0).map_err(backend_error)
            },
        );

        match outcome {
            Ok(attempts) => {
                self.lock_held = true;
                tracing::debug!(
                    lock_id,
                    attempts = attempts as u64,
                    "acquired the PostgreSQL advisory lock"
                );
                Ok(())
            }
            Err(err) => {
                tracing::warn!(
                    lock_id,
                    error = %err,
                    "could not acquire the PostgreSQL advisory lock"
                );
                Err(err)
            }
        }
    }

    fn release_lock(&mut self) -> Result<(), Error> {
        // The flag is cleared before the query runs: a connection that already
        // dropped releases the lock on the server side by itself, and retrying
        // the unlock on the same closed connection from `Drop` cannot succeed.
        self.lock_held = false;

        tracing::debug!(
            lock_id = MIGRATION_LOCK_ID,
            "releasing the PostgreSQL advisory lock"
        );

        self.client
            .query("SELECT pg_advisory_unlock($1)", &[&MIGRATION_LOCK_ID])
            .map(|_| ())
            .map_err(|err| {
                tracing::warn!(
                    lock_id = MIGRATION_LOCK_ID,
                    error = %err,
                    "failed to release the PostgreSQL advisory lock"
                );
                backend_error(err)
            })
    }
}

impl Drop for PostgresMigrationBackend {
    fn drop(&mut self) {
        if !self.lock_held {
            return;
        }
        if let Err(err) = self.release_lock() {
            tracing::warn!(error = %err, "failed to release migration advisory lock");
        }
    }
}

/// Transactional boundary of a [`PostgresMigrationBackend`] step.
pub struct PostgresMigrationTransaction<'a> {
    tx: Option<postgres::Transaction<'a>>,
    schema: String,
}

impl<'a> PostgresMigrationTransaction<'a> {
    fn transaction(&mut self) -> Result<&mut postgres::Transaction<'a>, Error> {
        self.tx.as_mut().ok_or_else(|| {
            Error::Backend("postgres migration transaction already finished".to_owned())
        })
    }

    /// Introspects the selected schema as the open transaction sees it.
    ///
    /// The catalog statements run over the transaction itself, so a step reads
    /// the changes it has applied so far and nobody else observes them until the
    /// transaction commits.
    pub fn introspect_catalog(&mut self) -> Result<SchemaCatalog, Error> {
        let schema = self.schema.clone();
        introspect_postgres_schema(self.transaction()?, &schema).map_err(backend_error)
    }
}

impl MigrationTransaction for PostgresMigrationTransaction<'_> {
    fn execute(&mut self, sql: &str) -> Result<(), Error> {
        self.transaction()?
            .batch_execute(sql)
            .map_err(backend_error)
    }

    fn catalog_checksum(&mut self) -> Result<Option<Checksum>, Error> {
        self.introspect_catalog()
            .map(|catalog| Some(catalog.checksum()))
    }

    fn record_entry(&mut self, entry: &SchemaHistoryEntry) -> Result<(), Error> {
        let sql = insert_entry_sql('$');
        let version = entry.version.as_ref().map(ToString::to_string);
        let checksum = entry.checksum.map(|checksum| checksum.to_hex());
        let kind = entry.kind.as_str();

        let params: [&(dyn postgres::types::ToSql + Sync); HISTORY_COLUMN_COUNT] = [
            &entry.installed_rank,
            &version,
            &entry.description,
            &kind,
            &entry.script,
            &checksum,
            &entry.installed_on,
            &entry.execution_time_ms,
            &entry.success,
        ];

        self.transaction()?
            .execute(&sql, &params)
            .map(|_| ())
            .map_err(backend_error)
    }

    fn delete_entry(&mut self, installed_rank: i64) -> Result<(), Error> {
        let sql = delete_entry_sql('$');
        self.transaction()?
            .execute(&sql, &[&installed_rank])
            .map(|_| ())
            .map_err(backend_error)
    }

    fn update_checksum(&mut self, script: &str, checksum: &Checksum) -> Result<(), Error> {
        let sql = update_checksum_sql('$');
        let hex = checksum.to_hex();
        self.transaction()?
            .execute(&sql, &[&hex, &script])
            .map(|_| ())
            .map_err(backend_error)
    }

    fn commit(mut self) -> Result<(), Error> {
        let tx = self.tx.take().ok_or_else(|| {
            Error::Backend("postgres migration transaction already finished".to_owned())
        })?;
        tx.commit().map_err(backend_error)
    }

    fn rollback(mut self) -> Result<(), Error> {
        let tx = self.tx.take().ok_or_else(|| {
            Error::Backend("postgres migration transaction already finished".to_owned())
        })?;
        tx.rollback().map_err(backend_error)
    }
}

impl Drop for PostgresMigrationTransaction<'_> {
    fn drop(&mut self) {
        if let Some(tx) = self.tx.take() {
            if let Err(err) = tx.rollback() {
                tracing::warn!(error = %err, "failed to rollback postgres migration transaction");
            }
        }
    }
}
