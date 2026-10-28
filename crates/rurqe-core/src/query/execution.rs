//! Query execution contracts, typed errors, and synchronous transaction control.

use core::fmt;
use core::future::Future;
use core::marker::PhantomData;

use crate::dialect::Dialect;
use crate::dml::insert::InsertSource;
use crate::dml::{AssignmentsPresent, DeleteQuery, ExecutableMutation, InsertQuery, UpdateQuery};
use crate::expr::SqlVal;
use crate::row::FromRow;
use crate::schema::Table;

use super::group_by::GroupedQuery;
use super::savepoint::SavepointName;
use super::scope::TableScope;
use super::select::{BuiltQuery, HasProjection, SelectQuery};

/// A query whose statement can be rendered and whose rows decode into `Row`.
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot be executed as a query",
    label = "unsupported query type",
    note = "executable queries are `SelectQuery`, `InsertQuery`, `UpdateQuery`, and `DeleteQuery`"
)]
pub trait ExecutableQuery {
    /// The Rust type produced for a single row of the query.
    type Row;

    /// Renders the statement into SQL text and its ordered parameter list.
    fn build_query(&self) -> Result<BuiltQuery, crate::query::BuildError>;
}

impl<D: Dialect, Tables: TableScope, Output, Lock> ExecutableQuery
    for SelectQuery<D, Tables, Output, HasProjection, Lock>
{
    type Row = Output;

    fn build_query(&self) -> Result<BuiltQuery, crate::query::BuildError> {
        self.build()
    }
}

impl<D: Dialect, T: Table, Values: InsertSource<T>, Returning> ExecutableQuery
    for InsertQuery<D, T, Values, Returning>
{
    type Row = Returning;

    fn build_query(&self) -> Result<BuiltQuery, crate::query::BuildError> {
        self.build()
    }
}

impl<D: Dialect, T: Table, State: ExecutableMutation, Returning, Scope, Assignments> ExecutableQuery
    for UpdateQuery<D, T, State, Returning, Scope, Assignments>
where
    Assignments: AssignmentsPresent,
{
    type Row = Returning;

    fn build_query(&self) -> Result<BuiltQuery, crate::query::BuildError> {
        self.build()
    }
}

impl<D: Dialect, T: Table, State: ExecutableMutation, Returning, Scope> ExecutableQuery
    for DeleteQuery<D, T, State, Returning, Scope>
{
    type Row = Returning;

    fn build_query(&self) -> Result<BuiltQuery, crate::query::BuildError> {
        self.build()
    }
}

impl<D: Dialect, Tables: TableScope, Output, GroupByCols> ExecutableQuery
    for GroupedQuery<D, Tables, Output, GroupByCols>
{
    type Row = Output;

    fn build_query(&self) -> Result<BuiltQuery, crate::query::BuildError> {
        self.build()
    }
}

/// A database driver capable of executing rendered queries.
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

/// Error raised while executing a query through [`QueryBackendExt`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutionError<E> {
    /// Pre-flight query validation detected invalid SQL semantics.
    Build(crate::query::BuildError),
    /// The driver failed to execute the statement.
    Backend(E),
    /// A produced row did not match the row type of the query.
    RowDecode {
        /// The zero-based index of the first row that failed to decode.
        row_index: usize,
        /// The exact reason why row decoding failed.
        source: crate::row::RowDecodeError,
    },
    /// A single-row query produced no rows.
    NoRows,
    /// A single-row query produced more than one row.
    MultipleRows {
        /// The number of rows the query produced.
        count: usize,
    },
}

impl<E> ExecutionError<E> {
    /// Returns `true` when pre-flight query validation failed.
    pub fn is_build(&self) -> bool {
        matches!(self, ExecutionError::Build(_))
    }

    /// Borrows the validation error details when pre-flight query validation failed.
    pub fn as_build(&self) -> Option<&crate::query::BuildError> {
        match self {
            ExecutionError::Build(error) => Some(error),
            _ => None,
        }
    }

    /// Returns `true` when the query produced no rows at all.
    pub fn is_not_found(&self) -> bool {
        matches!(self, ExecutionError::NoRows)
    }

    /// Returns `true` when a single-row query produced more than one row.
    pub fn is_multiple_rows(&self) -> bool {
        matches!(self, ExecutionError::MultipleRows { .. })
    }

    /// Returns `true` when a produced row did not match the query row type.
    pub fn is_row_decode(&self) -> bool {
        matches!(self, ExecutionError::RowDecode { .. })
    }

    /// Borrows the decode error details when the failure came from row decoding.
    pub fn as_row_decode(&self) -> Option<(usize, &crate::row::RowDecodeError)> {
        match self {
            ExecutionError::RowDecode { row_index, source } => Some((*row_index, source)),
            _ => None,
        }
    }

    /// Borrows the driver error when the failure came from the backend.
    pub fn as_backend(&self) -> Option<&E> {
        match self {
            ExecutionError::Backend(error) => Some(error),
            ExecutionError::Build(_)
            | ExecutionError::RowDecode { .. }
            | ExecutionError::NoRows
            | ExecutionError::MultipleRows { .. } => None,
        }
    }

    /// Takes the driver error when the failure came from the backend.
    pub fn into_backend(self) -> Option<E> {
        match self {
            ExecutionError::Backend(error) => Some(error),
            ExecutionError::Build(_)
            | ExecutionError::RowDecode { .. }
            | ExecutionError::NoRows
            | ExecutionError::MultipleRows { .. } => None,
        }
    }
}

impl<E> From<E> for ExecutionError<E> {
    fn from(error: E) -> Self {
        ExecutionError::Backend(error)
    }
}

impl<E: fmt::Display> fmt::Display for ExecutionError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExecutionError::Build(error) => write!(f, "invalid query: {error}"),
            ExecutionError::Backend(error) => write!(f, "database backend error: {error}"),
            ExecutionError::RowDecode { row_index, source } => {
                write!(f, "failed to decode row {row_index}: {source}")
            }
            ExecutionError::NoRows => {
                f.write_str("expected exactly one row, but the query returned none")
            }
            ExecutionError::MultipleRows { count } => {
                write!(
                    f,
                    "expected at most one row, but the query returned {count}"
                )
            }
        }
    }
}

impl<E: std::error::Error + 'static> std::error::Error for ExecutionError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ExecutionError::Build(error) => Some(error),
            ExecutionError::Backend(error) => Some(error),
            ExecutionError::RowDecode { source, .. } => Some(source),
            ExecutionError::NoRows | ExecutionError::MultipleRows { .. } => None,
        }
    }
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

/// An asynchronous database driver capable of executing rendered queries.
///
/// This is the asynchronous counterpart of [`QueryBackend`]: the adapter
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
/// The helpers mirror [`QueryBackendExt`] for [`AsyncQueryBackend`] adapters,
/// so an asynchronous connection, transaction, or pool never re-implements the
/// single-row policy. Every helper hands back an explicit `Send` future, so
/// awaiting it inside a task spawned on a multithreaded runtime stays sound.
pub trait AsyncQueryBackendExt: AsyncQueryBackend {
    /// Executes `query` and decodes every produced row.
    fn fetch_all<Q>(
        &self,
        query: &Q,
    ) -> impl Future<Output = Result<Vec<Q::Row>, ExecutionError<Self::Error>>> + Send
    where
        Q: ExecutableQuery + Sync,
        Q::Row: FromRow + Send,
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

/// The isolation level a transaction runs under.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IsolationLevel {
    /// Reads only rows committed before the statement started.
    ReadCommitted,
    /// Reads a stable snapshot, so repeated reads observe the same rows.
    RepeatableRead,
    /// Behaves as if transactions ran one after another.
    Serializable,
}

impl IsolationLevel {
    /// The SQL text of the corresponding `ISOLATION LEVEL` clause.
    pub const fn as_str(self) -> &'static str {
        match self {
            IsolationLevel::ReadCommitted => "READ COMMITTED",
            IsolationLevel::RepeatableRead => "REPEATABLE READ",
            IsolationLevel::Serializable => "SERIALIZABLE",
        }
    }
}

/// Tracks the nesting depth and savepoint names of an open transaction.
#[derive(Debug, Clone, Default)]
pub struct TransactionContext {
    next_savepoint: u32,
    savepoints: Vec<SavepointName>,
}

impl TransactionContext {
    /// Creates a context for the outermost transaction.
    pub fn new() -> Self {
        TransactionContext::default()
    }

    /// The number of nested savepoints currently open.
    pub fn depth(&self) -> usize {
        self.savepoints.len()
    }

    /// Whether the context currently sits inside a nested transaction.
    pub fn is_nested(&self) -> bool {
        !self.savepoints.is_empty()
    }

    /// The name of the innermost open savepoint, if any.
    pub fn current_savepoint(&self) -> Option<&str> {
        self.savepoints.last().map(SavepointName::as_str)
    }

    /// Allocates a fresh savepoint name and makes it the innermost block.
    pub fn open_savepoint(&mut self) -> SavepointName {
        self.next_savepoint += 1;
        let name = SavepointName::generated(self.next_savepoint);
        self.savepoints.push(name.clone());
        name
    }

    /// Ends the innermost block and returns the savepoint name it used.
    pub fn close_savepoint(&mut self) -> Option<SavepointName> {
        self.savepoints.pop()
    }
}

/// Runs a parameterless statement through a [`QueryBackend`].
fn execute_statement<B: QueryBackend + ?Sized>(
    backend: &mut B,
    sql: String,
) -> Result<(), B::Error> {
    backend
        .execute_raw(&BuiltQuery {
            sql,
            params: Vec::new(),
        })
        .map(|_| ())
}

/// A synchronous connection able to run transactions and nested savepoints.
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot run RURQE transactions",
    label = "unsupported transaction backend",
    note = "synchronous connections implement `TransactionBackend` on top of `QueryBackend`"
)]
pub trait TransactionBackend: QueryBackend {
    /// Opens the outermost transaction with the given isolation level.
    fn begin(&mut self, isolation: Option<IsolationLevel>) -> Result<(), Self::Error> {
        let mut sql = String::from("BEGIN");
        if let Some(level) = isolation {
            sql.push_str(" ISOLATION LEVEL ");
            sql.push_str(level.as_str());
        }
        execute_statement(self, sql)
    }

    /// Commits the current transaction, persisting all its work.
    fn commit(&mut self) -> Result<(), Self::Error> {
        execute_statement(self, String::from("COMMIT"))
    }

    /// Rolls the current transaction back, discarding all its work.
    fn rollback(&mut self) -> Result<(), Self::Error> {
        execute_statement(self, String::from("ROLLBACK"))
    }

    /// Opens the nested transaction `name` as a savepoint.
    ///
    /// The name is a [`SavepointName`], so it cannot carry characters that
    /// would escape the rendered statement.
    fn savepoint(&mut self, name: &SavepointName) -> Result<(), Self::Error> {
        execute_statement(self, format!("SAVEPOINT {}", name.as_str()))
    }

    /// Releases the savepoint `name`, keeping its work in the transaction.
    fn release_savepoint(&mut self, name: &SavepointName) -> Result<(), Self::Error> {
        execute_statement(self, format!("RELEASE SAVEPOINT {}", name.as_str()))
    }

    /// Rolls the transaction back to the savepoint `name`, discarding only the
    /// work done inside the nested block.
    fn rollback_to_savepoint(&mut self, name: &SavepointName) -> Result<(), Self::Error> {
        execute_statement(self, format!("ROLLBACK TO SAVEPOINT {}", name.as_str()))
    }
}
