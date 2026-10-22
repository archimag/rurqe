//! Synchronous execution adapter over [`postgres::Client`].

use rurqe_core::expr::SqlVal;
use rurqe_core::query::{
    BuiltQuery, QueryBackend, TransactionBackend, TransactionGuard, TransactionStatus,
    TransactionalConnection,
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
    status: TransactionStatus,
}

impl PgConnection {
    /// Wraps an existing synchronous PostgreSQL client.
    pub fn new(client: postgres::Client) -> Self {
        Self {
            client,
            status: TransactionStatus::new(),
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
        let mut guard = TransactionGuard::begin(self).map_err(E::from)?;
        let value = f(&mut guard)?;
        guard.commit().map_err(E::from)?;
        Ok(value)
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

impl TransactionalConnection for PgConnection {
    fn transaction_status(&mut self) -> &mut TransactionStatus {
        &mut self.status
    }
}
