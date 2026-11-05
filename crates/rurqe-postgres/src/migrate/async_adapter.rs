//! Blocking migration adapter over the asynchronous PostgreSQL clients.
//!
//! The migration protocol is synchronous while the asynchronous driver answers
//! through futures, so the adapter gives the protocol a thread of its own: the
//! thread owns a current-thread runtime and the connection, every operation
//! travels to it as one task, and the call that submitted it parks until the
//! answer arrives — the discipline `Migrator::migrate_async` establishes,
//! applied to a client the application already holds.
//!
//! Both sides block, so the caller belongs on a blocking thread of its own, and
//! the wait is reported through `warn_if_in_tokio_runtime`: a current-thread
//! runtime cannot finish under a parked caller, because that caller is the
//! thread that would have to poll the socket task of a [`PgAsyncConnection`] the
//! worker awaits. A panic inside the worker travels back as [`Error::Backend`],
//! and the session flag follows the proven outcome of the terminal statement.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use rurqe_migrate::catalog::SchemaCatalog;
use rurqe_migrate::checksum::Checksum;
use rurqe_migrate::error::Error;
use rurqe_migrate::history::{create_schema_history_table_sql, SchemaHistoryEntry};
use rurqe_migrate::introspect::SchemaSelection;
use rurqe_migrate::runner::{MigrationBackend, MigrationTransaction};
use rurqe_migrate::sleeper::{warn_if_in_tokio_runtime, ThreadSleeper};
use tokio::runtime::{Builder, Runtime};

use crate::backend::async_conn::PgAsyncConnection;
use crate::introspect::{decode_catalog, query_catalog_rows_async, DEFAULT_SCHEMA};

use super::history::{
    delete_entry_sql, insert_entry_sql, select_history_sql, update_checksum_sql,
    HISTORY_COLUMN_COUNT,
};
use super::{
    apply_unlock_answer, backend_error, backend_failure, poll_advisory_lock,
    ADVISORY_LOCK_RETRY_INTERVAL, DEFAULT_ADVISORY_LOCK_TIMEOUT, MIGRATION_LOCK_ID,
};

/// Reason reported when the worker thread no longer serves the protocol.
const WORKER_GONE: &str = "the PostgreSQL migration worker is no longer running";

/// Reason reported when a protocol operation panicked inside the worker thread.
const WORKER_PANIC: &str = "a PostgreSQL migration operation panicked inside the worker thread";

/// Reason reported when a reply carries a payload the caller did not expect.
const UNEXPECTED_REPLY: &str = "the PostgreSQL migration worker answered unexpectedly";

/// Reason reported when a step opens a boundary over an already open one.
const TRANSACTION_OPEN: &str = "the migration session already holds an open transaction";

/// The adapter named in the report of a caller that blocks a Tokio-managed thread.
const BLOCKING_OPERATION: &str = "PgAsyncMigrationBackend";

/// The connection the worker thread drives for one migration run.
enum Connection {
    /// A connection the caller opened and drives.
    Direct(PgAsyncConnection),
    /// A `deadpool-postgres` checkout held for the whole run.
    #[cfg(feature = "deadpool-postgres")]
    Pooled(deadpool_postgres::Object),
}

impl Connection {
    /// The client handle every protocol statement goes through.
    fn client(&self) -> &tokio_postgres::Client {
        match self {
            Connection::Direct(connection) => connection.client(),
            #[cfg(feature = "deadpool-postgres")]
            Connection::Pooled(object) => object,
        }
    }
}

/// The closure that prepares the connection of a run on the worker thread.
///
/// It runs inside the runtime of the worker thread, so a pooled checkout that
/// dials a connection has a runtime to schedule that work on, and it names the
/// shape both constructors share.
type Boot = Box<dyn FnOnce(&Runtime) -> Result<Connection, Error> + Send>;

/// The state of the session the worker thread owns.
struct Session {
    /// The connection the statements run on.
    connection: Connection,
    /// Whether the session is known to hold an open transaction.
    transaction: bool,
    /// Whether the session holds the migration advisory lock.
    lock_held: bool,
}

impl Session {
    /// Wraps `connection` in a session that holds no transaction and no lock.
    fn new(connection: Connection) -> Self {
        Session {
            connection,
            transaction: false,
            lock_held: false,
        }
    }

    /// The client handle the protocol statements run on.
    fn client(&self) -> &tokio_postgres::Client {
        self.connection.client()
    }
}

/// One protocol operation executed on the worker thread.
enum Operation {
    /// Creates the `__rurqe_schema_history` table when it is absent.
    EnsureSchemaHistoryTable,
    /// Reads every recorded history entry in installation order.
    ReadHistory,
    /// Reports the digest of the selected schemas.
    CatalogChecksum(SchemaSelection),
    /// Takes the session-scoped migration advisory lock.
    AcquireLock(Duration),
    /// Releases the session-scoped migration advisory lock.
    ReleaseLock,
    /// Opens a transactional boundary.
    Begin,
    /// Executes one SQL script.
    Execute(String),
    /// Records one history entry.
    RecordEntry(Box<SchemaHistoryEntry>),
    /// Drops the history entry of one installed rank.
    DeleteEntry(i64),
    /// Rewrites the checksum recorded for one script.
    UpdateChecksum { script: String, checksum: Checksum },
    /// Commits the open transactional boundary.
    Commit,
    /// Rolls the open transactional boundary back.
    Rollback,
}

/// One queued operation together with the channel its answer is sent through.
struct Command {
    /// The operation to execute.
    operation: Operation,
    /// The channel the answer of the operation is sent through.
    reply: Sender<Result<Response, Error>>,
}

/// The payload a completed operation reports.
enum Response {
    /// The operation produced no payload.
    Unit,
    /// The recorded history of the session.
    History(Vec<SchemaHistoryEntry>),
    /// The digest of the selected schemas.
    Checksum(Option<Checksum>),
}

impl Response {
    /// Reports the empty payload of an operation that returns nothing.
    fn unit(self) -> Result<(), Error> {
        match self {
            Response::Unit => Ok(()),
            _ => Err(unexpected_reply()),
        }
    }

    /// Reports the recorded history of the session.
    fn history(self) -> Result<Vec<SchemaHistoryEntry>, Error> {
        match self {
            Response::History(entries) => Ok(entries),
            _ => Err(unexpected_reply()),
        }
    }

    /// Reports the digest of the selected schemas.
    fn checksum(self) -> Result<Option<Checksum>, Error> {
        match self {
            Response::Checksum(checksum) => Ok(checksum),
            _ => Err(unexpected_reply()),
        }
    }
}

/// Describes a worker that no longer serves the protocol.
fn worker_gone() -> Error {
    Error::Backend(String::from(WORKER_GONE))
}

/// Describes a reply that carries a payload the caller never asked for.
fn unexpected_reply() -> Error {
    Error::Backend(String::from(UNEXPECTED_REPLY))
}

/// Handle to the thread executing the protocol of one migration run.
///
/// Starting the worker and submitting an operation both park the calling thread,
/// so both report a caller that blocks a Tokio-managed thread.
struct Worker {
    /// The queue the operations of the run are submitted through.
    commands: Option<Sender<Command>>,
    /// The thread executing the queued operations.
    thread: Option<JoinHandle<()>>,
}

impl Worker {
    /// Starts the worker thread and reports the outcome of its first action.
    ///
    /// `boot` runs on the worker thread before the first operation, so the
    /// runtime and a pooled checkout are prepared where the protocol waits for
    /// them; a failure there reaches the caller.
    fn start(boot: Boot) -> Result<Self, Error> {
        // Starting the worker parks the caller until the connection of the run is
        // ready, so the caller's discipline is reported before the wait begins.
        warn_if_in_tokio_runtime(BLOCKING_OPERATION);
        let (commands, queue) = mpsc::channel();
        let (started, boot_outcome) = mpsc::channel();

        let thread = thread::Builder::new()
            .name(String::from("rurqe-pg-migrate"))
            .spawn(move || {
                let runtime = match Builder::new_current_thread().enable_all().build() {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        let _ = started.send(Err(backend_failure(error)));
                        return;
                    }
                };

                let connection = match boot(&runtime) {
                    Ok(connection) => connection,
                    Err(error) => {
                        let _ = started.send(Err(error));
                        return;
                    }
                };

                let _ = started.send(Ok(()));
                run_worker(runtime, Some(Session::new(connection)), queue);
            })
            .map_err(backend_failure)?;

        match boot_outcome.recv() {
            Ok(Ok(())) => Ok(Worker {
                commands: Some(commands),
                thread: Some(thread),
            }),
            Ok(Err(error)) => {
                drop(commands);
                let _ = thread.join();
                Err(error)
            }
            Err(_) => {
                drop(commands);
                let _ = thread.join();
                Err(worker_gone())
            }
        }
    }

    /// Sends one operation to the worker and parks until it answers.
    ///
    /// The answer arrives from the worker thread, so the wait blocks the caller
    /// and is reported before it starts.
    fn submit(&self, operation: Operation) -> Result<Response, Error> {
        warn_if_in_tokio_runtime(BLOCKING_OPERATION);
        let commands = self.commands.as_ref().ok_or_else(worker_gone)?;
        let (reply, answer) = mpsc::channel();
        commands
            .send(Command { operation, reply })
            .map_err(|_| worker_gone())?;
        answer.recv().map_err(|_| worker_gone())?
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        // Dropping the queue ends the receive loop once the operations already
        // submitted have been executed, so the thread finishes its work, closes
        // the session inside the runtime, and ends.
        drop(self.commands.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Serves the queued operations until the queue is closed.
fn run_worker(runtime: Runtime, mut session: Option<Session>, queue: Receiver<Command>) {
    while let Ok(Command { operation, reply }) = queue.recv() {
        let outcome = match session.as_mut() {
            Some(active) => guarded(&runtime, active, operation),
            None => Err(worker_gone()),
        };
        let _ = reply.send(outcome);
    }

    if let Some(session) = session.take() {
        // The session is dropped inside the runtime context, so a pooled
        // checkout reaches the driver call that returns it to its pool from a
        // runtime that can schedule it.
        runtime.block_on(async move {
            drop(session);
        });
    }
}

/// Runs one queued operation, reporting a panic inside it as a typed failure.
///
/// A panic is caught at the boundary of the operation and travels back as
/// [`Error::Backend`], so the thread keeps serving the protocol.
fn guarded(
    runtime: &Runtime,
    session: &mut Session,
    operation: Operation,
) -> Result<Response, Error> {
    catch_unwind(AssertUnwindSafe(|| perform(runtime, session, operation)))
        .unwrap_or_else(|_| Err(Error::Backend(String::from(WORKER_PANIC))))
}

/// Executes one operation against the session the worker thread owns.
fn perform(
    runtime: &Runtime,
    session: &mut Session,
    operation: Operation,
) -> Result<Response, Error> {
    match operation {
        Operation::EnsureSchemaHistoryTable => {
            let statement = create_schema_history_table_sql();
            batch(runtime, session, statement)?;
            Ok(Response::Unit)
        }
        Operation::ReadHistory => {
            let entries = runtime.block_on(read_history(session.client()))?;
            Ok(Response::History(entries))
        }
        Operation::CatalogChecksum(selection) => {
            let client = session.client();
            let catalog = runtime.block_on(read_catalog(client, &selection))?;
            Ok(Response::Checksum(Some(catalog.checksum())))
        }
        Operation::AcquireLock(timeout) => {
            let lock_id = MIGRATION_LOCK_ID;
            let outcome = {
                let client = session.client();
                poll_advisory_lock(
                    lock_id,
                    timeout,
                    ADVISORY_LOCK_RETRY_INTERVAL,
                    &ThreadSleeper,
                    || {
                        runtime.block_on(async {
                            client
                                .query_one("SELECT pg_try_advisory_lock($1)", &[&lock_id])
                                .await
                                .map_err(backend_error)
                                .and_then(|row| row.try_get(0).map_err(backend_error))
                        })
                    },
                )
            };

            match outcome {
                Ok(attempts) => {
                    session.lock_held = true;
                    tracing::debug!(
                        lock_id,
                        attempts = attempts as u64,
                        "acquired the PostgreSQL advisory lock"
                    );
                    Ok(Response::Unit)
                }
                Err(error) => {
                    tracing::warn!(
                        lock_id,
                        error = %error,
                        "could not acquire the PostgreSQL advisory lock"
                    );
                    Err(error)
                }
            }
        }
        Operation::ReleaseLock => {
            if !session.lock_held {
                return Ok(Response::Unit);
            }

            let answer = {
                let client = session.client();
                runtime.block_on(async {
                    client
                        .query_one("SELECT pg_advisory_unlock($1)", &[&MIGRATION_LOCK_ID])
                        .await
                        .map_err(backend_error)
                        .and_then(|row| row.try_get(0).map_err(backend_error))
                })
            };

            apply_unlock_answer(MIGRATION_LOCK_ID, &mut session.lock_held, answer)?;
            Ok(Response::Unit)
        }
        Operation::Begin => {
            if session.transaction {
                return Err(Error::Backend(String::from(TRANSACTION_OPEN)));
            }
            batch(runtime, session, "BEGIN")?;
            session.transaction = true;
            Ok(Response::Unit)
        }
        Operation::Execute(statement) => {
            batch(runtime, session, &statement)?;
            Ok(Response::Unit)
        }
        Operation::RecordEntry(entry) => {
            let client = session.client();
            runtime.block_on(record_entry(client, &entry))?;
            Ok(Response::Unit)
        }
        Operation::DeleteEntry(installed_rank) => {
            let client = session.client();
            runtime.block_on(remove_entry(client, installed_rank))?;
            Ok(Response::Unit)
        }
        Operation::UpdateChecksum { script, checksum } => {
            let client = session.client();
            runtime.block_on(rewrite_checksum(client, &script, &checksum))?;
            Ok(Response::Unit)
        }
        Operation::Commit => {
            let outcome = batch(runtime, session, "COMMIT");
            match &outcome {
                Ok(()) => session.transaction = false,
                Err(error) => {
                    // A refused commit is compensated by a rollback, and the flag
                    // follows that rollback's proven outcome: an accepted one
                    // closes the session, a refused one keeps the mark.
                    match batch(runtime, session, "ROLLBACK") {
                        Ok(()) => {
                            session.transaction = false;
                            tracing::warn!(
                                error = %error,
                                "committing the migration step failed; the session was rolled back"
                            );
                        }
                        Err(rollback) => {
                            tracing::warn!(
                                error = %error,
                                rollback_error = %rollback,
                                "committing the migration step failed and the compensating rollback \
                                 was refused; the session stays marked as holding a transaction"
                            );
                        }
                    }
                }
            }
            outcome.map(|()| Response::Unit)
        }
        Operation::Rollback => {
            let outcome = batch(runtime, session, "ROLLBACK");
            // Only a rollback the server accepted proves the session idle; a
            // refused one keeps the flag, so the next `Begin` is refused.
            if outcome.is_ok() {
                session.transaction = false;
            }
            outcome.map(|()| Response::Unit)
        }
    }
}

/// Executes one statement on the session the worker thread owns.
fn batch(runtime: &Runtime, session: &Session, statement: &str) -> Result<(), Error> {
    runtime.block_on(batch_execute(session.client(), statement))
}

/// Executes `statement` through the simple query protocol of the driver.
async fn batch_execute(client: &tokio_postgres::Client, statement: &str) -> Result<(), Error> {
    client.batch_execute(statement).await.map_err(backend_error)
}

/// Reads and decodes the recorded history of `client`.
async fn read_history(client: &tokio_postgres::Client) -> Result<Vec<SchemaHistoryEntry>, Error> {
    let rows = client
        .query(&select_history_sql(), &[])
        .await
        .map_err(backend_error)?;
    super::decode_history_rows(&rows)
}

/// Records one history entry through the asynchronous client.
async fn record_entry(
    client: &tokio_postgres::Client,
    entry: &SchemaHistoryEntry,
) -> Result<(), Error> {
    let sql = insert_entry_sql();
    let version = entry.version.as_ref().map(ToString::to_string);
    let checksum = entry.checksum.map(|checksum| checksum.to_hex());
    let kind = entry.kind.as_str();

    let params: [&(dyn tokio_postgres::types::ToSql + Sync); HISTORY_COLUMN_COUNT] = [
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

    client
        .execute(&sql, &params)
        .await
        .map(|_| ())
        .map_err(backend_error)
}

/// Drops the history entry recorded for `installed_rank`.
async fn remove_entry(client: &tokio_postgres::Client, installed_rank: i64) -> Result<(), Error> {
    let sql = delete_entry_sql();
    client
        .execute(&sql, &[&installed_rank])
        .await
        .map(|_| ())
        .map_err(backend_error)
}

/// Rewrites the checksum recorded for `script`.
async fn rewrite_checksum(
    client: &tokio_postgres::Client,
    script: &str,
    checksum: &Checksum,
) -> Result<(), Error> {
    let sql = update_checksum_sql();
    let hex = checksum.to_hex();
    client
        .execute(&sql, &[&hex, &script])
        .await
        .map(|_| ())
        .map_err(backend_error)
}

/// Introspects the selected schemas through the asynchronous catalog queries.
async fn read_catalog(
    client: &tokio_postgres::Client,
    selection: &SchemaSelection,
) -> Result<SchemaCatalog, Error> {
    let schemas = selection.all();
    let rows = query_catalog_rows_async(client, &schemas)
        .await
        .map_err(backend_failure)?;
    decode_catalog(selection, rows).map_err(backend_failure)
}

/// [`MigrationBackend`] over the asynchronous PostgreSQL clients.
///
/// The adapter owns a thread that drives the asynchronous driver on a
/// current-thread runtime of its own, so the synchronous lifecycle of
/// [`rurqe_migrate::Migrator`] runs against a client the application already holds.
/// The connection is session-scoped: the advisory lock the runner takes is held
/// by the session the adapter owns for the whole run and released before it ends.
///
/// Every operation parks the calling thread until the worker answers, so the
/// caller belongs on a blocking thread of its own and never on a Tokio worker.
pub struct PgAsyncMigrationBackend {
    /// The thread serving the protocol of this run.
    worker: Worker,
    /// The schemas the recorded catalog digest describes.
    selection: SchemaSelection,
    /// Whether the session holds the migration advisory lock.
    lock_held: bool,
    /// How long a run waits for the advisory lock before giving up.
    lock_timeout: Duration,
}

impl PgAsyncMigrationBackend {
    /// Wraps a connection the caller has opened and drives.
    ///
    /// The socket of an asynchronous client is driven by the runtime that spawned
    /// its connection task, so the caller keeps that task alive for the whole run;
    /// the run itself belongs on a blocking thread, as the type documents.
    pub fn new(connection: PgAsyncConnection) -> Result<Self, Error> {
        let boot = move |_runtime: &Runtime| -> Result<Connection, Error> {
            Ok(Connection::Direct(connection))
        };
        Self::bootstrap(Box::new(boot))
    }

    /// Checks a connection out of a pool and keeps it for the whole run.
    ///
    /// The checkout happens on the worker thread, so the caller does not have to
    /// be inside a runtime to start the adapter. The connection stays checked
    /// out until the backend is dropped, which is what keeps the session that
    /// took the advisory lock identical to the session serving the protocol.
    #[cfg(feature = "deadpool-postgres")]
    pub fn from_pool(pool: crate::PgAsyncPool) -> Result<Self, Error> {
        let pooled = pool.pool().clone();
        Self::bootstrap(Box::new(
            move |runtime: &Runtime| -> Result<Connection, Error> {
                let object = runtime.block_on(pooled.get()).map_err(backend_failure)?;
                Ok(Connection::Pooled(object))
            },
        ))
    }

    /// Starts the worker thread and wraps it in the backend.
    fn bootstrap(boot: Boot) -> Result<Self, Error> {
        Ok(PgAsyncMigrationBackend {
            worker: Worker::start(boot)?,
            selection: SchemaSelection::single(DEFAULT_SCHEMA),
            lock_held: false,
            lock_timeout: DEFAULT_ADVISORY_LOCK_TIMEOUT,
        })
    }

    /// Selects the PostgreSQL schemas the recorded catalog digest describes.
    pub fn with_schemas(mut self, selection: SchemaSelection) -> Self {
        self.selection = selection;
        self
    }

    /// Selects the one PostgreSQL schema the recorded catalog digest describes.
    pub fn with_schema(mut self, schema: impl Into<String>) -> Self {
        self.selection = SchemaSelection::single(schema);
        self
    }

    /// The primary PostgreSQL schema the recorded catalog digest describes.
    pub fn schema(&self) -> &str {
        self.selection.primary()
    }

    /// Overrides how long a run waits for the advisory lock before giving up.
    pub fn with_lock_timeout(mut self, timeout: Duration) -> Self {
        self.lock_timeout = timeout;
        self
    }

    /// Sends one operation to the worker thread and waits for its answer.
    fn submit(&self, operation: Operation) -> Result<Response, Error> {
        self.worker.submit(operation)
    }
}

impl MigrationBackend for PgAsyncMigrationBackend {
    type Transaction<'a> = PgAsyncMigrationTransaction<'a>;

    fn ensure_schema_history_table(&mut self) -> Result<(), Error> {
        self.submit(Operation::EnsureSchemaHistoryTable)?.unit()
    }

    fn read_history(&mut self) -> Result<Vec<SchemaHistoryEntry>, Error> {
        self.submit(Operation::ReadHistory)?.history()
    }

    fn catalog_checksum(&mut self) -> Result<Option<Checksum>, Error> {
        let selection = self.selection.clone();
        self.submit(Operation::CatalogChecksum(selection))?
            .checksum()
    }

    fn begin(&mut self) -> Result<Self::Transaction<'_>, Error> {
        self.submit(Operation::Begin)?.unit()?;
        Ok(PgAsyncMigrationTransaction {
            backend: self,
            finished: false,
        })
    }

    fn execute_non_transactional(&mut self, sql: &str) -> Result<(), Error> {
        self.submit(Operation::Execute(sql.to_owned()))?.unit()
    }

    fn acquire_lock(&mut self) -> Result<(), Error> {
        tracing::debug!(
            lock_id = MIGRATION_LOCK_ID,
            timeout = ?self.lock_timeout,
            "acquiring the PostgreSQL advisory lock through the asynchronous backend"
        );
        self.submit(Operation::AcquireLock(self.lock_timeout))?
            .unit()?;
        self.lock_held = true;
        Ok(())
    }

    fn release_lock(&mut self) -> Result<(), Error> {
        if !self.lock_held {
            return Ok(());
        }

        tracing::debug!(
            lock_id = MIGRATION_LOCK_ID,
            "releasing the PostgreSQL advisory lock"
        );
        self.submit(Operation::ReleaseLock)?.unit()?;
        self.lock_held = false;
        Ok(())
    }
}

impl Drop for PgAsyncMigrationBackend {
    fn drop(&mut self) {
        // The session outlives the run, so a lock the run still holds is released
        // here instead of being left to a connection that returns to its pool.
        if !self.lock_held {
            return;
        }
        if let Err(error) = self.release_lock() {
            tracing::warn!(error = %error, "failed to release the migration advisory lock");
        }
    }
}

/// Transactional boundary of a [`PgAsyncMigrationBackend`] step.
///
/// The boundary issues `BEGIN` on the session the worker thread owns, and the
/// session is recorded idle only once the server proved the terminal statement:
/// a commit, a rollback, or the compensating rollback of a refused commit. A
/// refused one keeps the session marked, so the next boundary is rejected
/// instead of nesting inside the abandoned transaction.
pub struct PgAsyncMigrationTransaction<'a> {
    /// The backend the step belongs to.
    backend: &'a mut PgAsyncMigrationBackend,
    /// Whether the step reached a terminal call.
    finished: bool,
}

impl PgAsyncMigrationTransaction<'_> {
    /// Sends one operation to the worker thread and waits for its answer.
    fn submit(&self, operation: Operation) -> Result<Response, Error> {
        self.backend.submit(operation)
    }
}

impl MigrationTransaction for PgAsyncMigrationTransaction<'_> {
    fn execute(&mut self, sql: &str) -> Result<(), Error> {
        self.submit(Operation::Execute(sql.to_owned()))?.unit()
    }

    fn catalog_checksum(&mut self) -> Result<Option<Checksum>, Error> {
        let selection = self.backend.selection.clone();
        self.submit(Operation::CatalogChecksum(selection))?
            .checksum()
    }

    fn record_entry(&mut self, entry: &SchemaHistoryEntry) -> Result<(), Error> {
        self.submit(Operation::RecordEntry(Box::new(entry.clone())))?
            .unit()
    }

    fn delete_entry(&mut self, installed_rank: i64) -> Result<(), Error> {
        self.submit(Operation::DeleteEntry(installed_rank))?.unit()
    }

    fn update_checksum(&mut self, script: &str, checksum: &Checksum) -> Result<(), Error> {
        self.submit(Operation::UpdateChecksum {
            script: script.to_owned(),
            checksum: *checksum,
        })?
        .unit()
    }

    fn commit(mut self) -> Result<(), Error> {
        let outcome = self.submit(Operation::Commit)?.unit();
        self.finished = true;
        outcome
    }

    fn rollback(mut self) -> Result<(), Error> {
        let outcome = self.submit(Operation::Rollback)?.unit();
        self.finished = true;
        outcome
    }
}

impl Drop for PgAsyncMigrationTransaction<'_> {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        if let Err(error) = self
            .backend
            .submit(Operation::Rollback)
            .and_then(Response::unit)
        {
            tracing::warn!(error = %error, "failed to roll back an abandoned migration step");
        }
    }
}
