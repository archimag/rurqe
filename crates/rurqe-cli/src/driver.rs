//! Migration lifecycle driver adapters.
//!
//! [`SqliteMigrationBackend`] and [`PostgresMigrationBackend`] translate the
//! engine-neutral [`MigrationBackend`] contract into driver calls. Every step
//! runs inside a driver transaction, so a migration script and its
//! `__rurqe_schema_history` row are always applied or discarded together.
//!
//! Both adapters also report the canonical digest of the catalog they hold, so a
//! cold start anchors the very digest `schema.lock` and `checkpoint.toml` carry
//! whatever engine executed the declarative stage.

use std::sync::Arc;
use std::time::{Duration, Instant};

use rurqe_migrate::catalog::SchemaCatalog;
use rurqe_migrate::checksum::Checksum;
use rurqe_migrate::error::Error;
use rurqe_migrate::history::{
    create_schema_history_table_sql, SchemaHistoryEntry, SCHEMA_HISTORY_TABLE,
};
use rurqe_migrate::migration::MigrationKind;
use rurqe_migrate::runner::{MigrationBackend, MigrationTransaction};
use rurqe_migrate::sleeper::{warn_if_in_tokio_runtime, Sleeper, ThreadSleeper};
use rurqe_migrate::version::MigrationVersion;

use crate::introspect::{introspect_postgres_schema, introspect_sqlite, DEFAULT_SCHEMA};

/// Number of columns written by [`SchemaHistoryEntry`].
const HISTORY_COLUMN_COUNT: usize = 9;

/// Advisory lock key guarding a PostgreSQL migration run.
const MIGRATION_LOCK_ID: i64 = 0x5255_5251_455f_4d49;

/// Time a migration run waits for the PostgreSQL advisory lock.
const DEFAULT_ADVISORY_LOCK_TIMEOUT: Duration = Duration::from_secs(30);

/// Delay between two attempts to take the PostgreSQL advisory lock.
const ADVISORY_LOCK_RETRY_INTERVAL: Duration = Duration::from_millis(250);

/// Time a migration run waits for the SQLite write lock.
const DEFAULT_SQLITE_LOCK_TIMEOUT: Duration = Duration::from_secs(30);

/// Savepoint wrapping one migration step inside the SQLite run lock.
const SQLITE_STEP_SAVEPOINT: &str = "rurqe_migration_step";

/// A raw `__rurqe_schema_history` row before decoding into domain types.
struct RawHistoryRow {
    installed_rank: i64,
    version: Option<String>,
    description: String,
    kind: String,
    script: String,
    checksum: Option<String>,
    installed_on: String,
    execution_time_ms: i64,
    success: bool,
}

impl RawHistoryRow {
    fn decode(self) -> Result<SchemaHistoryEntry, Error> {
        Ok(SchemaHistoryEntry {
            installed_rank: self.installed_rank,
            version: match self.version {
                Some(raw) => Some(raw.parse::<MigrationVersion>()?),
                None => None,
            },
            description: self.description,
            kind: decode_kind(&self.kind)?,
            script: self.script,
            checksum: match self.checksum {
                Some(hex) => Some(Checksum::from_hex(&hex)?),
                None => None,
            },
            installed_on: self.installed_on,
            execution_time_ms: self.execution_time_ms,
            success: self.success,
        })
    }
}

fn decode_kind(raw: &str) -> Result<MigrationKind, Error> {
    [
        MigrationKind::Versioned,
        MigrationKind::Repeatable,
        MigrationKind::Baseline,
    ]
    .into_iter()
    .find(|kind| kind.as_str() == raw)
    .ok_or_else(|| {
        Error::Backend(format!(
            "unknown migration kind `{raw}` recorded in {SCHEMA_HISTORY_TABLE}"
        ))
    })
}

fn select_history_sql() -> String {
    format!(
        "SELECT installed_rank, version, description, \"type\", script, checksum, installed_on, \
         execution_time, success FROM {SCHEMA_HISTORY_TABLE} ORDER BY installed_rank"
    )
}

fn insert_entry_sql(sigil: char) -> String {
    format!(
        "INSERT INTO {SCHEMA_HISTORY_TABLE} (installed_rank, version, description, \"type\", \
         script, checksum, installed_on, execution_time, success) VALUES ({})",
        placeholders(sigil, HISTORY_COLUMN_COUNT)
    )
}

fn delete_entry_sql(sigil: char) -> String {
    format!("DELETE FROM {SCHEMA_HISTORY_TABLE} WHERE installed_rank = {sigil}1")
}

fn update_checksum_sql(sigil: char) -> String {
    format!("UPDATE {SCHEMA_HISTORY_TABLE} SET checksum = {sigil}1 WHERE script = {sigil}2")
}

fn placeholders(sigil: char, count: usize) -> String {
    (1..=count)
        .map(|index| format!("{sigil}{index}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn backend_error(err: impl std::fmt::Display) -> Error {
    Error::Backend(err.to_string())
}

fn release_savepoint_sql() -> String {
    format!("RELEASE SAVEPOINT {SQLITE_STEP_SAVEPOINT}")
}

fn rollback_savepoint_sql() -> String {
    format!(
        "ROLLBACK TO SAVEPOINT {SQLITE_STEP_SAVEPOINT}; RELEASE SAVEPOINT {SQLITE_STEP_SAVEPOINT}"
    )
}

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
fn poll_advisory_lock<F>(
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

/// [`MigrationBackend`] over a `rusqlite` connection.
///
/// The adapter is synchronous and blocking: every statement runs on the calling
/// thread, and [`acquire_lock`](MigrationBackend::acquire_lock) waits for the
/// SQLite write lock through the driver busy handler. Invoke it from a blocking
/// context — `tokio::task::spawn_blocking`, a dedicated thread, or plain
/// synchronous code — never directly on an asynchronous worker thread.
pub struct SqliteMigrationBackend {
    conn: rusqlite::Connection,
    lock_held: bool,
    lock_timeout: Duration,
}

impl SqliteMigrationBackend {
    /// Wraps an open SQLite connection.
    pub fn new(conn: rusqlite::Connection) -> Self {
        Self {
            conn,
            lock_held: false,
            lock_timeout: DEFAULT_SQLITE_LOCK_TIMEOUT,
        }
    }

    /// Overrides how long a run waits for the write lock before giving up.
    ///
    /// SQLite serialises writers through a single lock on the database file. A
    /// run that cannot take it within the timeout is rejected with a diagnostic
    /// naming the wait, so a runner blocked by a stuck peer fails fast instead
    /// of hanging forever.
    pub fn with_lock_timeout(mut self, timeout: Duration) -> Self {
        self.lock_timeout = timeout;
        self
    }

    /// Introspects the schema catalog of the wrapped connection.
    ///
    /// The catalog is read through the very introspection the rest of the
    /// toolchain uses, so the history table stays out of it and the reported
    /// types follow the SQLite affinity rules.
    pub fn introspect_catalog(&self) -> Result<SchemaCatalog, Error> {
        introspect_sqlite(&self.conn).map_err(backend_error)
    }
}

impl MigrationBackend for SqliteMigrationBackend {
    type Transaction<'a> = SqliteMigrationTransaction<'a>;

    fn ensure_schema_history_table(&mut self) -> Result<(), Error> {
        self.conn
            .execute_batch(create_schema_history_table_sql())
            .map_err(backend_error)
    }

    fn read_history(&mut self) -> Result<Vec<SchemaHistoryEntry>, Error> {
        let mut statement = self
            .conn
            .prepare(&select_history_sql())
            .map_err(backend_error)?;

        let rows = statement
            .query_map([], |row| {
                Ok(RawHistoryRow {
                    installed_rank: row.get(0)?,
                    version: row.get(1)?,
                    description: row.get(2)?,
                    kind: row.get(3)?,
                    script: row.get(4)?,
                    checksum: row.get(5)?,
                    installed_on: row.get(6)?,
                    execution_time_ms: row.get(7)?,
                    success: row.get::<_, i64>(8)? != 0,
                })
            })
            .map_err(backend_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(backend_error)?;

        rows.into_iter().map(RawHistoryRow::decode).collect()
    }

    fn catalog_checksum(&mut self) -> Result<Option<Checksum>, Error> {
        self.introspect_catalog()
            .map(|catalog| Some(catalog.checksum()))
    }

    fn begin(&mut self) -> Result<Self::Transaction<'_>, Error> {
        if self.lock_held {
            self.conn
                .execute_batch(&format!("SAVEPOINT {SQLITE_STEP_SAVEPOINT}"))
                .map_err(backend_error)?;
            return Ok(SqliteMigrationTransaction {
                step: Some(SqliteStep::Savepoint(&mut self.conn)),
            });
        }

        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(backend_error)?;
        Ok(SqliteMigrationTransaction {
            step: Some(SqliteStep::Transaction(tx)),
        })
    }

    fn execute_non_transactional(&mut self, sql: &str) -> Result<(), Error> {
        self.conn.execute_batch(sql).map_err(backend_error)
    }

    fn acquire_lock(&mut self) -> Result<(), Error> {
        warn_if_in_tokio_runtime("SqliteMigrationBackend::acquire_lock");
        let timeout = self.lock_timeout;
        self.conn.busy_timeout(timeout).map_err(backend_error)?;
        self.conn.execute_batch("BEGIN IMMEDIATE").map_err(|err| {
            Error::Backend(format!(
                "could not acquire the SQLite write lock within {timeout:?}: {err}; another \
                 migration runner may be holding it, retry once that run finishes"
            ))
        })?;
        self.lock_held = true;
        Ok(())
    }

    fn release_lock(&mut self) -> Result<(), Error> {
        // The flag is cleared before the statement runs: a connection that is
        // already unusable cannot commit, and the file lock is released by the
        // driver once the connection closes.
        self.lock_held = false;
        self.conn.execute_batch("COMMIT").map_err(backend_error)
    }
}

impl Drop for SqliteMigrationBackend {
    fn drop(&mut self) {
        if !self.lock_held {
            return;
        }
        if let Err(err) = self.release_lock() {
            tracing::warn!(error = %err, "failed to release the SQLite migration lock");
        }
    }
}

/// One step of a SQLite migration run.
enum SqliteStep<'a> {
    /// A step that owns its native transaction.
    Transaction(rusqlite::Transaction<'a>),
    /// A step nested in the transaction that holds the run lock.
    Savepoint(&'a mut rusqlite::Connection),
}

/// Transactional boundary of a [`SqliteMigrationBackend`] step.
pub struct SqliteMigrationTransaction<'a> {
    step: Option<SqliteStep<'a>>,
}

impl<'a> SqliteMigrationTransaction<'a> {
    fn current(&mut self) -> Result<&mut SqliteStep<'a>, Error> {
        self.step.as_mut().ok_or_else(|| {
            Error::Backend("sqlite migration transaction already finished".to_owned())
        })
    }

    fn connection(&mut self) -> Result<&rusqlite::Connection, Error> {
        match self.current()? {
            SqliteStep::Transaction(tx) => Ok(&**tx),
            SqliteStep::Savepoint(conn) => Ok(&**conn),
        }
    }

    /// Introspects the schema catalog the step sees.
    ///
    /// The catalog is read over the connection of the step, so a migration
    /// observes the changes it has already applied without committing them.
    pub fn introspect_catalog(&mut self) -> Result<SchemaCatalog, Error> {
        introspect_sqlite(self.connection()?).map_err(backend_error)
    }

    fn run(&mut self, sql: &str, params: impl rusqlite::Params) -> Result<(), Error> {
        self.connection()?
            .execute(sql, params)
            .map(|_| ())
            .map_err(backend_error)
    }
}

impl MigrationTransaction for SqliteMigrationTransaction<'_> {
    fn execute(&mut self, sql: &str) -> Result<(), Error> {
        self.connection()?.execute_batch(sql).map_err(backend_error)
    }

    fn catalog_checksum(&mut self) -> Result<Option<Checksum>, Error> {
        self.introspect_catalog()
            .map(|catalog| Some(catalog.checksum()))
    }

    fn record_entry(&mut self, entry: &SchemaHistoryEntry) -> Result<(), Error> {
        let sql = insert_entry_sql('?');
        let version = entry.version.as_ref().map(ToString::to_string);
        let checksum = entry.checksum.map(|checksum| checksum.to_hex());

        self.run(
            &sql,
            rusqlite::params![
                entry.installed_rank,
                version,
                entry.description,
                entry.kind.as_str(),
                entry.script,
                checksum,
                entry.installed_on,
                entry.execution_time_ms,
                entry.success,
            ],
        )
    }

    fn delete_entry(&mut self, installed_rank: i64) -> Result<(), Error> {
        let sql = delete_entry_sql('?');
        self.run(&sql, rusqlite::params![installed_rank])
    }

    fn update_checksum(&mut self, script: &str, checksum: &Checksum) -> Result<(), Error> {
        let sql = update_checksum_sql('?');
        self.run(&sql, rusqlite::params![checksum.to_hex(), script])
    }

    fn commit(mut self) -> Result<(), Error> {
        let step = self.step.take().ok_or_else(|| {
            Error::Backend("sqlite migration transaction already finished".to_owned())
        })?;

        match step {
            SqliteStep::Transaction(tx) => tx.commit().map_err(backend_error),
            SqliteStep::Savepoint(conn) => conn
                .execute_batch(&release_savepoint_sql())
                .map_err(backend_error),
        }
    }

    fn rollback(mut self) -> Result<(), Error> {
        let step = self.step.take().ok_or_else(|| {
            Error::Backend("sqlite migration transaction already finished".to_owned())
        })?;

        match step {
            SqliteStep::Transaction(tx) => tx.rollback().map_err(backend_error),
            SqliteStep::Savepoint(conn) => conn
                .execute_batch(&rollback_savepoint_sql())
                .map_err(backend_error),
        }
    }
}

impl Drop for SqliteMigrationTransaction<'_> {
    fn drop(&mut self) {
        let Some(step) = self.step.take() else {
            return;
        };

        let outcome = match step {
            SqliteStep::Transaction(tx) => tx.rollback(),
            SqliteStep::Savepoint(conn) => conn.execute_batch(&rollback_savepoint_sql()),
        };

        if let Err(err) = outcome {
            tracing::warn!(error = %err, "failed to rollback sqlite migration transaction");
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use rurqe_migrate::migration::Migration;
    use rurqe_migrate::runner::Migrator;
    use rurqe_migrate::sleeper::NoopSleeper;
    use std::sync::Mutex;
    use std::time::Duration;

    /// A [`Sleeper`] recording the pause of every waiting round.
    #[derive(Default)]
    struct RecordingSleeper {
        pauses: Mutex<Vec<Duration>>,
    }

    impl RecordingSleeper {
        fn new() -> Self {
            Self::default()
        }

        fn pauses(&self) -> Vec<Duration> {
            self.pauses
                .lock()
                .map(|guard| guard.to_vec())
                .unwrap_or_default()
        }
    }

    impl Sleeper for RecordingSleeper {
        fn sleep(&self, duration: Duration) {
            if let Ok(mut guard) = self.pauses.lock() {
                guard.push(duration);
            }
        }
    }

    fn backend() -> SqliteMigrationBackend {
        SqliteMigrationBackend::new(rusqlite::Connection::open_in_memory().unwrap())
    }

    fn migrations(script: &str, sql: &str) -> Vec<Migration> {
        vec![Migration::parse(script, sql).unwrap()]
    }

    #[test]
    fn sqlite_backend_applies_versioned_migrations_once() {
        let mut backend = backend();
        let scripts = migrations("V1__init.sql", "CREATE TABLE users (id INTEGER);");

        let report = Migrator::new().migrate(&mut backend, &scripts).unwrap();
        assert_eq!(report.applied_count, 1);

        let report = Migrator::new().migrate(&mut backend, &scripts).unwrap();
        assert_eq!(report.applied_count, 0);

        let history = backend.read_history().unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].script, "V1__init.sql");
        assert_eq!(history[0].kind, MigrationKind::Versioned);
        assert!(history[0].success);
    }

    #[test]
    fn sqlite_backend_rolls_back_failed_migrations() {
        let mut backend = backend();
        let broken = migrations("V1__broken.sql", "CREATE TABLE oops (;");

        let error = Migrator::new()
            .migrate(&mut backend, &broken)
            .expect_err("the broken script fails to execute");
        assert!(matches!(
            error,
            rurqe_migrate::error::Error::MigrationExecutionFailed { .. }
        ));

        let history = backend.read_history().expect("history is readable");
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].script, "V1__broken.sql");
        assert!(!history[0].success);

        let blocked = Migrator::new()
            .migrate(&mut backend, &broken)
            .expect_err("the failed entry blocks the next run");
        assert!(matches!(
            blocked,
            rurqe_migrate::error::Error::FailedMigrationAlreadyApplied { .. }
        ));

        let migrator = Migrator::new();
        let fixed = migrations("V1__fixed.sql", "CREATE TABLE ok (id INTEGER);");
        let repair = migrator
            .repair(&mut backend, &fixed)
            .expect("repair purges the failed entry");
        assert_eq!(repair.removed_failed_entries, 1);

        let report = migrator
            .migrate(&mut backend, &fixed)
            .expect("the fixed script applies");
        assert_eq!(report.applied_count, 1);
    }

    #[test]
    fn sqlite_backend_reports_the_digest_of_its_catalog() {
        let mut backend = backend();
        let scripts = migrations(
            "V1__users.sql",
            "CREATE TABLE users (id INTEGER NOT NULL PRIMARY KEY, email TEXT NOT NULL);",
        );
        Migrator::new()
            .migrate(&mut backend, &scripts)
            .expect("the script applies");

        let catalog = backend
            .introspect_catalog()
            .expect("the catalog of the connection is introspected");
        assert_eq!(catalog.tables.len(), 1);
        assert_eq!(catalog.tables[0].name, "users");

        // The history table stays outside the catalog the digest describes.
        assert_eq!(
            backend
                .catalog_checksum()
                .expect("the digest of the connection"),
            Some(catalog.checksum())
        );
    }

    #[test]
    fn sqlite_transaction_introspects_the_catalog_it_sees() {
        let mut backend = backend();
        let scripts = migrations("V1__users.sql", "CREATE TABLE users (id INTEGER NOT NULL);");
        Migrator::new()
            .migrate(&mut backend, &scripts)
            .expect("the script applies");

        let mut tx = backend.begin().expect("the migration transaction opens");
        let catalog = tx
            .introspect_catalog()
            .expect("the catalog of the transaction is introspected");
        assert_eq!(catalog.tables.len(), 1);
        assert_eq!(catalog.tables[0].name, "users");

        // A change applied by the very transaction is visible to its own catalog.
        tx.execute("CREATE TABLE orders (id INTEGER NOT NULL);")
            .expect("the transaction applies the step");
        assert_eq!(
            tx.introspect_catalog()
                .expect("the catalog of the transaction is introspected")
                .tables
                .len(),
            2
        );

        tx.rollback().expect("the transaction is rolled back");
    }

    #[test]
    fn the_run_lock_nests_every_step_in_a_savepoint() {
        let mut backend = backend();
        backend.acquire_lock().expect("the write lock is taken");

        {
            let mut step = backend.begin().expect("a step opens inside the lock");
            step.execute("CREATE TABLE users (id INTEGER NOT NULL);")
                .expect("the step applies");
            step.commit().expect("the step is released");
        }

        {
            let mut step = backend
                .begin()
                .expect("a second step opens inside the lock");
            step.execute("CREATE TABLE oops (;")
                .expect_err("the broken statement fails");
            step.rollback().expect("the step is rolled back");
        }

        backend.release_lock().expect("the lock commits the run");

        let users: i64 = backend
            .conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name = 'users'",
                [],
                |row| row.get(0),
            )
            .expect("the committed table is visible");
        assert_eq!(users, 1);

        let oops: i64 = backend
            .conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name = 'oops'",
                [],
                |row| row.get(0),
            )
            .expect("the rolled back table stays absent");
        assert_eq!(oops, 0);
    }

    /// A fresh database file no concurrent test can collide with.
    fn scratch_database(label: &str) -> std::path::PathBuf {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let sequence = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut path = std::env::temp_dir();
        path.push(format!(
            "rurqe-driver-{label}-{}-{sequence}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        path
    }

    #[test]
    fn the_write_lock_excludes_a_second_runner_on_the_same_database() {
        let path = scratch_database("lock");

        let mut first = SqliteMigrationBackend::new(
            rusqlite::Connection::open(&path).expect("the first connection opens"),
        );
        let mut second = SqliteMigrationBackend::new(
            rusqlite::Connection::open(&path).expect("the second connection opens"),
        )
        .with_lock_timeout(Duration::ZERO);

        first
            .acquire_lock()
            .expect("the first runner takes the write lock");

        let error = second
            .acquire_lock()
            .expect_err("the second runner cannot take a held write lock");
        match &error {
            Error::Backend(message) => assert!(message.contains("write lock"), "{message}"),
            other => panic!("expected a backend failure, got {other:?}"),
        }

        first
            .release_lock()
            .expect("the first runner publishes and releases the lock");
        second
            .acquire_lock()
            .expect("the released lock is handed to the second runner");
        second
            .release_lock()
            .expect("the second runner releases the lock");

        drop(first);
        drop(second);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn the_advisory_lock_is_taken_on_the_first_attempt() {
        let mut calls = 0u32;
        let attempts = poll_advisory_lock(
            MIGRATION_LOCK_ID,
            Duration::from_secs(1),
            Duration::ZERO,
            &NoopSleeper,
            || {
                calls += 1;
                Ok(true)
            },
        )
        .expect("the free lock is taken at once");

        assert_eq!(attempts, 1);
        assert_eq!(calls, 1);
    }

    #[test]
    fn the_advisory_lock_is_polled_until_it_becomes_free() {
        let mut pending = 2u32;
        let attempts = poll_advisory_lock(
            MIGRATION_LOCK_ID,
            Duration::from_secs(1),
            Duration::ZERO,
            &NoopSleeper,
            || {
                if pending == 0 {
                    Ok(true)
                } else {
                    pending -= 1;
                    Ok(false)
                }
            },
        )
        .expect("the lock becomes free after the earlier rounds fail");

        assert_eq!(attempts, 3);
    }

    #[test]
    fn every_waiting_round_pauses_through_the_configured_sleeper() {
        let sleeper = RecordingSleeper::new();
        let interval = Duration::from_millis(5);
        let mut pending = 2u32;

        let attempts = poll_advisory_lock(
            MIGRATION_LOCK_ID,
            Duration::from_secs(1),
            interval,
            &sleeper,
            || {
                if pending == 0 {
                    Ok(true)
                } else {
                    pending -= 1;
                    Ok(false)
                }
            },
        )
        .expect("the lock becomes free after two waiting rounds");

        // The round that takes the lock never pauses, so only the two rejected
        // rounds ask the sleeper to wait.
        assert_eq!(attempts, 3);
        assert_eq!(sleeper.pauses(), vec![interval, interval]);
    }

    #[test]
    fn a_held_advisory_lock_times_out_with_its_identifier() {
        let error = poll_advisory_lock(
            MIGRATION_LOCK_ID,
            Duration::ZERO,
            Duration::ZERO,
            &NoopSleeper,
            || Ok(false),
        )
        .expect_err("a lock nobody releases times out");

        let message = match error {
            Error::Backend(message) => message,
            other => panic!("unexpected error: {other:?}"),
        };

        assert!(
            message.contains(&MIGRATION_LOCK_ID.to_string()),
            "{message}"
        );
        assert!(
            message.contains(&format!("0x{MIGRATION_LOCK_ID:016x}")),
            "{message}"
        );
    }

    #[test]
    fn a_backend_failure_stops_the_polling() {
        let error = poll_advisory_lock(
            MIGRATION_LOCK_ID,
            Duration::from_secs(1),
            Duration::ZERO,
            &NoopSleeper,
            || Err(Error::Backend("connection lost".to_owned())),
        )
        .expect_err("a failing backend is reported unchanged");

        assert_eq!(error, Error::Backend("connection lost".to_owned()));
    }
}
