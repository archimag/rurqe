//! Synchronous execution adapter over [`postgres::Client`].

use rurqe_core::expr::SqlVal;
use rurqe_core::query::{
    BuiltQuery, QueryBackend, SavepointName, TransactionBackend, TransactionContext,
};

use super::types::{execute_raw_sync, fetch_raw_sync, PgError};

/// A [`postgres::Client`] executing RURQE queries through the synchronous driver.
///
/// The adapter implements the RURQE
/// [`QueryBackend`](rurqe_core::query::QueryBackend) contract, so the
/// `fetch_all`, `fetch_one`, `fetch_optional`, and `execute` helpers of
/// [`QueryBackendExt`](rurqe_core::query::QueryBackendExt) become available.
pub struct PgConnection {
    client: postgres::Client,
    tx_context: TransactionContext,
    in_transaction: bool,
}

impl PgConnection {
    /// Wraps an existing synchronous PostgreSQL client.
    pub fn new(client: postgres::Client) -> Self {
        Self {
            client,
            tx_context: TransactionContext::new(),
            in_transaction: false,
        }
    }

    /// Returns a shared reference to the wrapped client.
    pub fn client(&self) -> &postgres::Client {
        &self.client
    }

    /// Returns a mutable reference to the wrapped client.
    pub fn client_mut(&mut self) -> &mut postgres::Client {
        &mut self.client
    }

    /// Consumes the adapter, returning the wrapped client.
    pub fn into_inner(self) -> postgres::Client {
        self.client
    }

    /// Runs `f` inside a transaction, committing on `Ok` and rolling back on `Err`.
    ///
    /// The outermost call executes its block between `BEGIN` and `COMMIT`; any
    /// error returned by `f` rolls the transaction back, so a failed block never
    /// leaves partial work behind. Calling `transaction` again while a
    /// transaction is already open nests the block inside a `SAVEPOINT`, so an
    /// inner failure rolls back only its own statements while the enclosing
    /// transaction stays intact. A panicking block is rolled back as well, so
    /// unwinding never leaves an open transaction or savepoint behind.
    pub fn transaction<T, E, F>(&mut self, f: F) -> Result<T, E>
    where
        F: FnOnce(&mut PgConnection) -> Result<T, E>,
        E: From<PgError>,
    {
        let outer = !self.in_transaction;
        let savepoint = if outer {
            tracing::info!("beginning transaction");
            self.begin(None).map_err(E::from)?;
            self.in_transaction = true;
            None
        } else {
            let name = self.tx_context.open_savepoint();
            tracing::debug!(savepoint = %name, "creating savepoint");
            self.savepoint(&name).map_err(E::from)?;
            Some(name)
        };

        let mut guard = PgTransactionGuard {
            connection: self,
            outer,
            savepoint,
            finished: false,
        };

        match f(&mut *guard.connection) {
            Ok(value) => {
                if outer {
                    guard.connection.commit().map_err(E::from)?;
                    guard.connection.in_transaction = false;
                    tracing::info!("committed transaction");
                } else if let Some(name) = guard.savepoint.as_ref() {
                    guard.connection.release_savepoint(name).map_err(E::from)?;
                    guard.connection.tx_context.close_savepoint();
                    tracing::debug!(savepoint = %name, "released savepoint");
                }
                guard.finished = true;
                Ok(value)
            }
            Err(error) => Err(error),
        }
    }
}

/// RAII guard that rolls the surrounding block back unless it finished cleanly.
///
/// The guard owns the mutable borrow of the connection for the duration of the
/// closure, so the closure still receives a reborrow while an unwind discards
/// the block through [`Drop`].
struct PgTransactionGuard<'a> {
    connection: &'a mut PgConnection,
    outer: bool,
    savepoint: Option<SavepointName>,
    finished: bool,
}

impl Drop for PgTransactionGuard<'_> {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        if self.outer {
            let _ = self.connection.rollback();
            self.connection.in_transaction = false;
            tracing::warn!("rolled back transaction");
        } else if let Some(savepoint) = self.savepoint.as_ref() {
            let _ = self.connection.rollback_to_savepoint(savepoint);
            let _ = self.connection.release_savepoint(savepoint);
            self.connection.tx_context.close_savepoint();
            tracing::warn!(savepoint = %savepoint, "rolled back to savepoint");
        }
    }
}

impl QueryBackend for PgConnection {
    type Error = PgError;

    fn fetch_raw(&mut self, query: &BuiltQuery) -> Result<Vec<Vec<SqlVal>>, PgError> {
        fetch_raw_sync(&mut self.client, query)
    }

    fn execute_raw(&mut self, query: &BuiltQuery) -> Result<u64, PgError> {
        execute_raw_sync(&mut self.client, query)
    }
}

impl TransactionBackend for PgConnection {}
