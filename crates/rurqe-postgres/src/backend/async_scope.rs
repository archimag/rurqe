//! Closure-scoped asynchronous transactions with savepoint nesting.
//!
//! The scope lends the connection to a closure, so the transaction is opened,
//! nested, and closed by the adapter instead of by a handle the caller has to
//! remember to commit. [`PgAsyncScope::transaction`] wraps a nested block in a
//! SQL `SAVEPOINT` and awaits the rollback of a block that fails, which is the
//! contract the synchronous
//! [`TransactionGuard`](rurqe_core::query::TransactionGuard) implements for the
//! owned and pooled connection adapters.
//!
//! The scope shares the poison record of the connection it borrows, so a
//! savepoint rollback the driver refuses marks that connection unusable instead
//! of leaving the work of the block pending inside the enclosing transaction.

use core::future::Future;
use core::pin::Pin;
use std::sync::Arc;

use rurqe_core::dialect::Dialect;
use rurqe_core::expr::SqlVal;
use rurqe_core::query::{AsyncQueryBackend, BuiltQuery, SavepointName, TransactionContext};
use tokio_postgres::Client;

use crate::dialect::{Pg16, Postgres};

use super::async_conn::{execute_raw_async, fetch_raw_async, fetch_raw_limited_async};
use super::poison::{control_failure, PoisonCell};
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

/// Renders a savepoint name as a quoted PostgreSQL identifier.
///
/// A savepoint name cannot be bound as a parameter, so it is rendered into the
/// statement text. [`SavepointName`] restricts the text to `[A-Za-z0-9_]`, but
/// that invariant belongs to `rurqe-core` and is invisible here, so the name is
/// quoted through the PostgreSQL identifier rule instead of being concatenated
/// raw: an embedded `"` is doubled and the whole name stays a single token.
/// PostgreSQL folds an unquoted identifier to lower case, and the names the
/// adapter generates are already lower case, so the quoted spelling addresses
/// the same savepoint the bare token would have created, and identifier
/// escaping is identical across every declared PostgreSQL version.
fn quoted_savepoint(name: &str) -> String {
    Postgres::<Pg16>::quote_identifier(name)
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
///
/// The scope shares the poison record of the connection it borrows: a savepoint
/// rollback the driver refuses is recorded there, and the scope then refuses
/// further statements and further savepoints with
/// [`PgError::ConnectionPoisoned`] instead of running further work on that
/// connection.
pub struct PgAsyncScope<'a> {
    client: &'a Client,
    context: TransactionContext,
    /// Poison record shared with the connection the scope borrows.
    poison: Arc<PoisonCell>,
}

impl<'a> PgAsyncScope<'a> {
    /// Wraps the connection of a transaction the adapter has opened.
    ///
    /// `poison` is the record of that connection, so a refusal observed at any
    /// nesting depth is reported by the connection itself and by every scope
    /// borrowing it.
    pub(crate) fn new(client: &'a Client, poison: Arc<PoisonCell>) -> Self {
        PgAsyncScope {
            client,
            context: TransactionContext::new(),
            poison,
        }
    }

    /// Refuses to run anything on a poisoned connection.
    ///
    /// The refusal carries the reason recorded on the shared cell, so a caller
    /// learns which statement could not be completed instead of only that the
    /// connection is unusable.
    fn refuse_if_poisoned(&self) -> Result<(), PgError> {
        if !self.poison.is_poisoned() {
            return Ok(());
        }

        let reason = self
            .poison
            .poison_reason()
            .unwrap_or_else(|| String::from("the connection recorded no reason"));
        Err(PgError::ConnectionPoisoned { reason })
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
    /// rollback the driver refuses poisons the connection the scope borrows,
    /// because the refused statement leaves the work of the block pending inside
    /// the enclosing transaction: the failure is traced, the error of the block
    /// still reaches the caller unchanged, and every further statement and
    /// savepoint is refused with [`PgError::ConnectionPoisoned`], so the
    /// connection has to be discarded rather than reused.
    ///
    /// A block that panics needs no scope-local catch. The unwind leaves the
    /// nested scope and reaches the exit guard the adapter installed around the
    /// outer block — the unwind guard of a connection, or the disposition guard
    /// of a pool — which marks the shared cell poisoned before the panic
    /// re-raises; the savepoint the panic left open is closed by the rollback of
    /// the enclosing block or by the destruction of the connection. The scope
    /// itself holds no guard, because only the adapter that opened the outermost
    /// block learns that the transaction was abandoned.
    ///
    /// `E` has to absorb the driver error, which the dialect error alias over
    /// `PgError` does, so a terminal call propagates with `?` unchanged inside
    /// the nested block as well.
    pub async fn transaction<T, E, F>(&mut self, f: F) -> Result<T, E>
    where
        E: From<PgError>,
        F: for<'s> FnOnce(PgAsyncScope<'s>) -> ScopeFuture<'s, T, E>,
    {
        self.refuse_if_poisoned().map_err(E::from)?;

        let savepoint = self.context.open_savepoint();
        let statement = format!("SAVEPOINT {}", quoted_savepoint(savepoint.as_str()));
        let opened = execute_control(self.client, statement).await;
        if let Err(error) = opened {
            self.context.close_savepoint();
            return Err(E::from(error));
        }

        let nested = PgAsyncScope {
            client: self.client,
            context: self.context.clone(),
            poison: Arc::clone(&self.poison),
        };

        match f(nested).await {
            Ok(value) => {
                let name = savepoint.as_str();
                let statement = format!("RELEASE SAVEPOINT {}", quoted_savepoint(name));
                match execute_control(self.client, statement).await {
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
    /// A rollback the driver refuses is traced and recorded on the shared poison
    /// cell instead of being returned, because the failure of the block is the
    /// error the caller has to act on; the connection must then be discarded
    /// rather than reused, because the statements of the block stay pending
    /// inside the enclosing transaction.
    async fn roll_back(&self, savepoint: &SavepointName) {
        let name = savepoint.as_str();
        let quoted = quoted_savepoint(name);
        let statement = format!("ROLLBACK TO SAVEPOINT {quoted}");
        match execute_control(self.client, statement).await {
            Ok(()) => tracing::warn!(savepoint = %name, "rolled back to savepoint"),
            Err(failure) => {
                let refused = format!("ROLLBACK TO SAVEPOINT {quoted}");
                self.poison.mark(control_failure(&refused, &failure));
                tracing::error!(
                    savepoint = %name,
                    failure = %failure,
                    "rolling back to the savepoint failed; the connection must be discarded"
                );
            }
        }
    }
}

impl AsyncQueryBackend for PgAsyncScope<'_> {
    type Error = PgError;

    async fn fetch_raw(&self, query: &BuiltQuery) -> Result<Vec<Vec<SqlVal>>, PgError> {
        self.refuse_if_poisoned()?;
        fetch_raw_async(self.client, query).await
    }

    async fn fetch_raw_limited(
        &self,
        query: &BuiltQuery,
        limit: Option<usize>,
    ) -> Result<Vec<Vec<SqlVal>>, PgError> {
        self.refuse_if_poisoned()?;
        fetch_raw_limited_async(self.client, query, limit).await
    }

    async fn execute_raw(&self, query: &BuiltQuery) -> Result<u64, PgError> {
        self.refuse_if_poisoned()?;
        execute_raw_async(self.client, query).await
    }
}

#[cfg(test)]
mod tests {
    use super::quoted_savepoint;

    #[test]
    fn a_generated_name_renders_as_a_quoted_identifier() {
        assert_eq!(quoted_savepoint("rurqe_sp_7"), "\"rurqe_sp_7\"");
    }

    #[test]
    fn metacharacters_stay_inside_the_quoted_identifier() {
        // The validation of `SavepointName` rejects these characters, and the
        // statements are still rendered so that a name which ever did carry one
        // reaches PostgreSQL as a single escaped token instead of as SQL.
        assert_eq!(
            quoted_savepoint("sp\"; DROP TABLE users; --"),
            "\"sp\"\"; DROP TABLE users; --\""
        );
        assert_eq!(quoted_savepoint("sp'or"), "\"sp'or\"");
    }
}
