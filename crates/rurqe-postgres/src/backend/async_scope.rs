//! Closure-scoped asynchronous transactions with savepoint nesting.
//!
//! The scope lends the connection to a closure, so the transaction is opened,
//! nested, and closed by the adapter instead of by a handle the caller has to
//! remember to commit. [`PgAsyncScope::transaction`] wraps a nested block in a
//! SQL `SAVEPOINT` and awaits the rollback of a block that fails, which is the
//! contract the synchronous
//! [`TransactionGuard`](rurqe_core::query::TransactionGuard) implements for the
//! owned and pooled connection adapters.

use core::future::Future;
use core::pin::Pin;

use rurqe_core::expr::SqlVal;
use rurqe_core::query::{AsyncQueryBackend, BuiltQuery, SavepointName, TransactionContext};
use tokio_postgres::Client;

use super::async_conn::{execute_raw_async, fetch_raw_async, fetch_raw_limited_async};
use super::types::PgError;

/// The erased future a transaction closure hands back to the adapter.
///
/// The asynchronous terminal helpers of a query return opaque `impl Future`
/// types, which stable Rust cannot name in the return type of a closure
/// argument, so a closure erases its own future into a box. The erasure costs
/// one allocation per block and leaves query construction, the rendered
/// statement, the borrowed scope, and the decoded rows concrete.
pub type ScopeFuture<'s, T, E> = Pin<Box<dyn Future<Output = Result<T, E>> + Send + 's>>;

/// Runs a transaction-control statement the adapter rendered itself.
///
/// The statement is assembled by the caller (the `BEGIN` text from the
/// isolation level, the savepoint statements from a validated savepoint name)
/// and carries no bound parameters, so it travels through the same
/// extended-query path as every typed statement instead of a separate
/// simple-query API.
pub(super) async fn execute_control(client: &Client, sql: String) -> Result<(), PgError> {
    let statement = BuiltQuery {
        sql,
        params: Vec::new(),
    };
    execute_raw_async(client, &statement).await?;
    Ok(())
}

/// An open asynchronous transaction, or a savepoint inside one, that statements
/// execute against.
///
/// The adapter hands the scope to the closure of
/// [`PgAsyncConnection::transaction_with_isolation`](crate::PgAsyncConnection::transaction_with_isolation),
/// which has opened the transaction and closes it once the closure returns.
/// Every statement executed through the scope therefore shares that
/// transaction: the block commits as a whole, and a block that returns `Err` is
/// rolled back before the error reaches the caller.
///
/// The scope implements the asynchronous query contract, so the ordinary
/// terminal calls accept it exactly like a connection or a pool:
///
/// ```text
/// connection
///     .transaction_with_isolation(IsolationLevel::Serializable, |mut scope| {
///         Box::pin(async move {
///             insert_into::<Postgres, User>()
///                 .values((UserId::set(1i64),))
///                 .execute(&scope)
///                 .await?;
///
///             // A nested block is a savepoint: its rollback undoes only its
///             // own statements.
///             scope
///                 .transaction(|nested| {
///                     Box::pin(async move {
///                         insert_into::<Postgres, User>()
///                             .values((UserId::set(2i64),))
///                             .execute(&nested)
///                             .await?;
///                         Ok(())
///                     })
///                 })
///                 .await?;
///
///             Ok(())
///         })
///     })
///     .await?;
/// ```
///
/// The closure returns the dialect error alias, so a terminal call propagates
/// with `?` unchanged inside the block.
pub struct PgAsyncScope<'a> {
    client: &'a Client,
    context: TransactionContext,
}

impl<'a> PgAsyncScope<'a> {
    /// Wraps the connection of a transaction the adapter has opened.
    pub(crate) fn new(client: &'a Client) -> Self {
        PgAsyncScope {
            client,
            context: TransactionContext::new(),
        }
    }

    /// Runs `f` inside a nested transaction, releasing it on `Ok` and rolling it
    /// back on `Err`.
    ///
    /// The block is a SQL `SAVEPOINT` of the transaction the scope belongs to,
    /// so a failing block undoes only its own statements: the enclosing
    /// transaction stays open, and the work it did before and after the nested
    /// block can still commit. Nesting works the same way, one savepoint deeper.
    ///
    /// Both endings are awaited: `RELEASE SAVEPOINT` keeps the work of a block
    /// that returned `Ok`, and `ROLLBACK TO SAVEPOINT` undoes the work of a
    /// block that returned `Err` before that error reaches the caller. A
    /// rollback the driver refuses is traced instead of replacing the error, and
    /// the connection must then be discarded rather than reused, because the
    /// statements of the block stay pending inside the enclosing transaction.
    ///
    /// `E` has to absorb the driver error, which the dialect error alias over
    /// `PgError` does, so a terminal call propagates with `?` unchanged inside
    /// the nested block as well.
    pub async fn transaction<T, E, F>(&mut self, f: F) -> Result<T, E>
    where
        E: From<PgError>,
        F: for<'s> FnOnce(PgAsyncScope<'s>) -> ScopeFuture<'s, T, E>,
    {
        let savepoint = self.context.open_savepoint();
        let opened = execute_control(self.client, format!("SAVEPOINT {savepoint}")).await;
        if let Err(error) = opened {
            self.context.close_savepoint();
            return Err(E::from(error));
        }

        let nested = PgAsyncScope {
            client: self.client,
            context: self.context.clone(),
        };

        match f(nested).await {
            Ok(value) => {
                let name = savepoint.as_str();
                match execute_control(self.client, format!("RELEASE SAVEPOINT {name}")).await {
                    Ok(()) => {
                        tracing::debug!(savepoint = %name, "released savepoint");
                        self.context.close_savepoint();
                        Ok(value)
                    }
                    Err(error) => {
                        self.context.close_savepoint();
                        Err(E::from(error))
                    }
                }
            }
            Err(error) => {
                self.roll_back(&savepoint).await;
                self.context.close_savepoint();
                Err(error)
            }
        }
    }

    /// Rolls a nested block back to `savepoint`.
    ///
    /// A rollback the driver refuses is traced instead of returned, because the
    /// failure of the block is the error the caller has to act on; the
    /// connection must then be discarded rather than reused, because the
    /// statements of the block stay pending inside the enclosing transaction.
    async fn roll_back(&self, savepoint: &SavepointName) {
        let name = savepoint.as_str();
        match execute_control(self.client, format!("ROLLBACK TO SAVEPOINT {name}")).await {
            Ok(()) => tracing::warn!(savepoint = %name, "rolled back to savepoint"),
            Err(failure) => tracing::error!(
                savepoint = %name,
                failure = %failure,
                "rolling back to the savepoint failed; the connection must be discarded"
            ),
        }
    }
}

impl AsyncQueryBackend for PgAsyncScope<'_> {
    type Error = PgError;

    fn fetch_raw(
        &self,
        query: &BuiltQuery,
    ) -> impl Future<Output = Result<Vec<Vec<SqlVal>>, PgError>> + Send {
        fetch_raw_async(self.client, query)
    }

    fn fetch_raw_limited(
        &self,
        query: &BuiltQuery,
        limit: Option<usize>,
    ) -> impl Future<Output = Result<Vec<Vec<SqlVal>>, PgError>> + Send {
        fetch_raw_limited_async(self.client, query, limit)
    }

    fn execute_raw(&self, query: &BuiltQuery) -> impl Future<Output = Result<u64, PgError>> + Send {
        execute_raw_async(self.client, query)
    }
}
