//! The synchronous execution seam: the backend contract, its typed helpers,
//! and the lazily decoded row iterator.

use core::marker::PhantomData;

use crate::expr::SqlVal;
use crate::query::BuiltQuery;
use crate::row::FromRow;

use super::error::ExecutionError;
use super::ExecutableQuery;

/// A database driver capable of executing rendered queries.
///
/// An implementation translates the typed parameters of a [`BuiltQuery`] into
/// native driver values and reports the driver error unchanged. The three
/// methods of this trait are the bare statement path of the session hygiene
/// protocol of [`TransactionBackend`](crate::query::TransactionBackend): while
/// the bookkeeping of the adapter records a refused transaction-control
/// statement ([`TransactionStatus`](crate::query::TransactionStatus)), a
/// statement is answered with that recorded failure before it reaches the
/// driver, because the session may still hold the transaction the refused
/// statement could not close. The transaction-control path stays open through
/// [`TransactionBackend`](crate::query::TransactionBackend), whose statements
/// are the ones that end the state.
///
/// A backend that mediates checkouts of shared sessions — a connection pool —
/// owes the third obligation of the same protocol: it settles the disposition
/// of every checkout it hands out, by proving the session idle, by repairing
/// it, or by destroying the connection, and it never returns a session it
/// cannot vouch for to the next caller.
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot execute RURQE queries",
    label = "unsupported query backend",
    note = "driver adapters implement `QueryBackend` by translating `SqlVal` parameters into native driver values"
)]
pub trait QueryBackend {
    /// The error raised by the driver.
    type Error;

    /// Executes `query` and returns every produced row as ordered values.
    fn fetch_raw(&mut self, query: &BuiltQuery) -> Result<Vec<Vec<SqlVal>>, Self::Error>;

    /// Executes `query` and returns at most `limit` rows as ordered values.
    ///
    /// Adapters able to stop reading the cursor early override this method to
    /// avoid materialising the entire result set. The default implementation
    /// delegates to [`fetch_raw`](QueryBackend::fetch_raw) and truncates the
    /// collected rows, so third-party backends stay correct without any change.
    fn fetch_raw_limited(
        &mut self,
        query: &BuiltQuery,
        limit: Option<usize>,
    ) -> Result<Vec<Vec<SqlVal>>, Self::Error> {
        let mut rows = self.fetch_raw(query)?;
        if let Some(limit) = limit {
            rows.truncate(limit);
        }
        Ok(rows)
    }

    /// Executes `query` without decoding rows and returns the affected row count.
    fn execute_raw(&mut self, query: &BuiltQuery) -> Result<u64, Self::Error>;
}

/// Lazily decoded rows of a statement the backend has already executed.
///
/// The iterator owns the raw value buffer of a statement that has already
/// finished, so the connection is released before a single row is decoded, and
/// [`FromRow`] runs only when [`next`](Iterator::next) asks for the row.
pub struct QueryRowIter<Q, E> {
    rows: std::vec::IntoIter<Vec<SqlVal>>,
    decoded: usize,
    marker: PhantomData<fn() -> (Q, E)>,
}

impl<Q, E> QueryRowIter<Q, E> {
    /// Wraps the raw rows of an executed statement.
    fn new(rows: Vec<Vec<SqlVal>>) -> Self {
        QueryRowIter {
            rows: rows.into_iter(),
            decoded: 0,
            marker: PhantomData,
        }
    }

    /// The number of rows this iterator has decoded so far.
    pub fn decoded(&self) -> usize {
        self.decoded
    }
}

impl<Q, E> Iterator for QueryRowIter<Q, E>
where
    Q: ExecutableQuery,
    Q::Row: FromRow,
{
    type Item = Result<Q::Row, ExecutionError<E>>;

    fn next(&mut self) -> Option<Self::Item> {
        let values = self.rows.next()?;
        let row_index = self.decoded;
        self.decoded += 1;

        Some(
            Q::Row::try_from_row(&values)
                .map_err(|source| ExecutionError::RowDecode { row_index, source }),
        )
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.rows.size_hint()
    }
}

/// Execution helpers decoding query rows through [`FromRow`].
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a RURQE query backend",
    label = "missing backend implementation",
    note = "implement `QueryBackend` for the driver; `QueryBackendExt` is then available for free"
)]
pub trait QueryBackendExt: QueryBackend {
    /// Executes `query` and collects the decoded rows into `C`.
    ///
    /// The collection type is chosen by the caller, so `HashMap`, `HashSet`,
    /// `BTreeMap`, `VecDeque`, or any other `FromIterator<Q::Row>` receives the
    /// rows directly, without an intermediate `Vec` allocation.
    fn fetch_collect<C, Q>(&mut self, query: &Q) -> Result<C, ExecutionError<Self::Error>>
    where
        Q: ExecutableQuery,
        Q::Row: FromRow,
        C: FromIterator<Q::Row>,
    {
        let built = query.build_query().map_err(ExecutionError::Build)?;
        let rows = self.fetch_raw(&built).map_err(ExecutionError::Backend)?;

        QueryRowIter::<Q, Self::Error>::new(rows).collect()
    }

    /// Executes `query` and returns a lazy iterator over its decoded rows.
    ///
    /// The raw values are read from the backend before the iterator is
    /// returned, so the connection is released immediately and every row is
    /// decoded by [`FromRow`] only when it is consumed. A streaming fold, map,
    /// or filter therefore never materialises a collection of rows.
    fn fetch_iter<Q>(
        &mut self,
        query: &Q,
    ) -> Result<QueryRowIter<Q, Self::Error>, ExecutionError<Self::Error>>
    where
        Q: ExecutableQuery,
        Q::Row: FromRow,
    {
        let built = query.build_query().map_err(ExecutionError::Build)?;
        let rows = self.fetch_raw(&built).map_err(ExecutionError::Backend)?;

        Ok(QueryRowIter::new(rows))
    }

    /// Executes `query` and decodes every produced row into a `Vec`.
    fn fetch_all<Q>(&mut self, query: &Q) -> Result<Vec<Q::Row>, ExecutionError<Self::Error>>
    where
        Q: ExecutableQuery,
        Q::Row: FromRow,
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
        &mut self,
        query: &Q,
    ) -> Result<Option<Q::Row>, ExecutionError<Self::Error>>
    where
        Q: ExecutableQuery,
        Q::Row: FromRow,
    {
        let built = query.build_query().map_err(ExecutionError::Build)?;
        let rows = self
            .fetch_raw_limited(&built, Some(2))
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

    /// Executes `query` and decodes exactly one row.
    fn fetch_one<Q>(&mut self, query: &Q) -> Result<Q::Row, ExecutionError<Self::Error>>
    where
        Q: ExecutableQuery,
        Q::Row: FromRow,
    {
        self.fetch_optional(query)?.ok_or(ExecutionError::NoRows)
    }

    /// Executes `query` and returns the number of affected rows.
    fn execute<Q>(&mut self, query: &Q) -> Result<u64, ExecutionError<Self::Error>>
    where
        Q: ExecutableQuery,
    {
        let built = query.build_query().map_err(ExecutionError::Build)?;
        self.execute_raw(&built).map_err(ExecutionError::Backend)
    }
}

impl<B: QueryBackend + ?Sized> QueryBackendExt for B {}
