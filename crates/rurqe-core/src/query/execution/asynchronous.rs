//! The asynchronous execution seam: the backend contract and its typed helpers.

use core::future::Future;

use crate::expr::SqlVal;
use crate::query::BuiltQuery;
use crate::row::FromRow;

use super::error::ExecutionError;
use super::ExecutableQuery;

/// An asynchronous database driver capable of executing rendered queries.
///
/// This is the asynchronous counterpart of [`QueryBackend`](super::QueryBackend): the adapter
/// translates the typed [`crate::expr::SqlVal`] parameters into native driver
/// values and awaits the statement. The trait deliberately exposes only the two
/// raw entry points; decoding lives in [`AsyncQueryBackendExt`], so every
/// adapter shares a single implementation of `fetch_all`, `fetch_optional`,
/// `fetch_one`, and `execute`.
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot execute RURQE queries asynchronously",
    label = "unsupported asynchronous query backend",
    note = "asynchronous driver adapters implement `AsyncQueryBackend`; `AsyncQueryBackendExt` is then available for free"
)]
pub trait AsyncQueryBackend: Sync {
    /// The error raised by the driver.
    ///
    /// The error crosses task boundaries: the returned futures are `Send`, so a
    /// spawned task must be able to hand the failure back to its caller.
    type Error: Send;

    /// Executes `query` and returns every produced row as ordered values.
    ///
    /// The returned future borrows the backend only for its own lifetime and is
    /// `Send`, so it can be moved into a spawned task while the backend stays
    /// shared through a [`Sync`] reference.
    fn fetch_raw(
        &self,
        query: &BuiltQuery,
    ) -> impl Future<Output = Result<Vec<Vec<SqlVal>>, Self::Error>> + Send;

    /// Executes `query` and returns at most `limit` rows as ordered values.
    ///
    /// Adapters able to stop reading the cursor early override this method to
    /// avoid materialising the entire result set. The default implementation
    /// delegates to [`fetch_raw`](AsyncQueryBackend::fetch_raw) and truncates
    /// the collected rows, so third-party backends stay correct without change.
    fn fetch_raw_limited(
        &self,
        query: &BuiltQuery,
        limit: Option<usize>,
    ) -> impl Future<Output = Result<Vec<Vec<SqlVal>>, Self::Error>> + Send {
        async move {
            let mut rows = self.fetch_raw(query).await?;
            if let Some(limit) = limit {
                rows.truncate(limit);
            }
            Ok(rows)
        }
    }

    /// Executes `query` without decoding rows and returns the affected row count.
    ///
    /// Like [`fetch_raw`](AsyncQueryBackend::fetch_raw), the future is `Send`, so
    /// a mutating statement runs inside a spawned task as well.
    fn execute_raw(
        &self,
        query: &BuiltQuery,
    ) -> impl Future<Output = Result<u64, Self::Error>> + Send;
}

/// Asynchronous execution helpers decoding query rows through [`FromRow`].
///
/// The helpers mirror [`QueryBackendExt`](super::QueryBackendExt) for
/// [`AsyncQueryBackend`] adapters, so an asynchronous connection, transaction,
/// or pool never re-implements the single-row policy. Every helper hands back an
/// explicit `Send` future, so awaiting it inside a task spawned on a
/// multithreaded runtime stays sound.
pub trait AsyncQueryBackendExt: AsyncQueryBackend {
    /// Executes `query` and collects the decoded rows into `C`.
    ///
    /// The collection type is chosen by the caller, so `HashMap`, `HashSet`,
    /// `BTreeSet`, `VecDeque`, or any other `FromIterator<Q::Row>` receives the
    /// rows directly, without an intermediate `Vec` allocation. The returned
    /// future is `Send`, so the whole read-decode-collect pipeline stays inside
    /// a task spawned on a multithreaded runtime.
    fn fetch_collect<C, Q>(
        &self,
        query: &Q,
    ) -> impl Future<Output = Result<C, ExecutionError<Self::Error>>> + Send
    where
        Q: ExecutableQuery + Sync,
        Q::Row: FromRow + Send,
        C: FromIterator<Q::Row>,
    {
        async move {
            let built = query.build_query().map_err(ExecutionError::Build)?;
            let rows = self
                .fetch_raw(&built)
                .await
                .map_err(ExecutionError::Backend)?;
            rows.iter()
                .enumerate()
                .map(|(row_index, values)| {
                    Q::Row::try_from_row(values)
                        .map_err(|source| ExecutionError::RowDecode { row_index, source })
                })
                .collect()
        }
    }

    /// Executes `query` and decodes every produced row into a `Vec`.
    fn fetch_all<Q>(
        &self,
        query: &Q,
    ) -> impl Future<Output = Result<Vec<Q::Row>, ExecutionError<Self::Error>>> + Send
    where
        Q: ExecutableQuery + Sync,
        Q::Row: FromRow + Send,
    {
        self.fetch_collect::<Vec<Q::Row>, Q>(query)
    }

    /// Executes `query` and decodes at most one row.
    ///
    /// The cursor is read for at most two rows, so a query producing an
    /// unbounded result set never materialises it. Exceeding the single-row
    /// contract fails with [`ExecutionError::MultipleRows`] before the row is
    /// decoded, and only the confirmed single row reaches [`FromRow`].
    fn fetch_optional<Q>(
        &self,
        query: &Q,
    ) -> impl Future<Output = Result<Option<Q::Row>, ExecutionError<Self::Error>>> + Send
    where
        Q: ExecutableQuery + Sync,
        Q::Row: FromRow + Send,
    {
        async move {
            let built = query.build_query().map_err(ExecutionError::Build)?;
            let rows = self
                .fetch_raw_limited(&built, Some(2))
                .await
                .map_err(ExecutionError::Backend)?;
            match rows.len() {
                0 => Ok(None),
                1 => {
                    let values = rows.into_iter().next().ok_or(ExecutionError::NoRows)?;
                    Q::Row::try_from_row(&values).map(Some).map_err(|source| {
                        ExecutionError::RowDecode {
                            row_index: 0,
                            source,
                        }
                    })
                }
                count => Err(ExecutionError::MultipleRows { count }),
            }
        }
    }

    /// Executes `query` and decodes exactly one row.
    fn fetch_one<Q>(
        &self,
        query: &Q,
    ) -> impl Future<Output = Result<Q::Row, ExecutionError<Self::Error>>> + Send
    where
        Q: ExecutableQuery + Sync,
        Q::Row: FromRow + Send,
    {
        async move {
            self.fetch_optional(query)
                .await?
                .ok_or(ExecutionError::NoRows)
        }
    }

    /// Executes `query` and returns the number of affected rows.
    fn execute<Q>(
        &self,
        query: &Q,
    ) -> impl Future<Output = Result<u64, ExecutionError<Self::Error>>> + Send
    where
        Q: ExecutableQuery + Sync,
    {
        async move {
            let built = query.build_query().map_err(ExecutionError::Build)?;
            self.execute_raw(&built)
                .await
                .map_err(ExecutionError::Backend)
        }
    }
}

impl<B: AsyncQueryBackend + ?Sized> AsyncQueryBackendExt for B {}
