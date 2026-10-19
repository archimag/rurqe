//! PostgreSQL dialect implementation and runtime adapters for RURQE.

use core::fmt::Write;
use core::marker::PhantomData;

pub use rurqe_core;
use rurqe_core::dialect::{
    Dialect, SupportsDistinctOn, SupportsLateralJoin, SupportsOnConflict, SupportsReturning,
};

/// Marker trait representing a PostgreSQL engine version.
pub trait PgVersion: 'static + Send + Sync {}

/// PostgreSQL 14 marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Pg14;
impl PgVersion for Pg14 {}

/// PostgreSQL 15 marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Pg15;
impl PgVersion for Pg15 {}

/// PostgreSQL 16 marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Pg16;
impl PgVersion for Pg16 {}

/// PostgreSQL dialect parameterised by engine version (defaulting to [`Pg16`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Postgres<V: PgVersion = Pg16>(PhantomData<fn() -> V>);

impl<V: PgVersion> Dialect for Postgres<V> {
    fn push_quoted_identifier(out: &mut String, ident: &str) {
        out.push('"');
        for ch in ident.chars() {
            if ch == '"' {
                out.push('"');
            }
            out.push(ch);
        }
        out.push('"');
    }

    fn push_placeholder(out: &mut String, index: usize) {
        let _ = write!(out, "${index}");
    }
}

impl<V: PgVersion> SupportsReturning for Postgres<V> {}
impl<V: PgVersion> SupportsDistinctOn for Postgres<V> {}
impl<V: PgVersion> SupportsOnConflict for Postgres<V> {}
impl<V: PgVersion> SupportsLateralJoin for Postgres<V> {}

/// Shared parameter binding and row decoding for the PostgreSQL driver adapters.
#[cfg(feature = "tokio-postgres")]
mod pg_support {
    use std::error::Error as StdError;
    use std::fmt;

    use bytes::BytesMut;
    use rurqe_core::expr::SqlVal;
    use rurqe_core::query::{BuiltQuery, ExecutableQuery, ExecutionError};
    use rurqe_core::row::FromRow;
    use tokio_postgres::types::{IsNull, ToSql, Type};
    use tokio_postgres::Row;

    /// Error raised while executing a query through a PostgreSQL driver adapter.
    #[derive(Debug)]
    pub enum PgError {
        /// The PostgreSQL driver failed to execute the statement.
        Driver(tokio_postgres::Error),
        /// A result column holds a type that cannot be decoded into a [`SqlVal`].
        UnsupportedType(String),
        /// The connection could not be checked out of a `deadpool-postgres` pool.
        #[cfg(feature = "deadpool-postgres")]
        Pool(deadpool_postgres::PoolError),
    }

    impl fmt::Display for PgError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                PgError::Driver(error) => write!(f, "PostgreSQL driver error: {error}"),
                PgError::UnsupportedType(name) => write!(
                    f,
                    "unsupported PostgreSQL column type `{name}`: cannot decode into a SQL value"
                ),
                #[cfg(feature = "deadpool-postgres")]
                PgError::Pool(error) => write!(f, "connection pool error: {error}"),
            }
        }
    }

    impl StdError for PgError {
        fn source(&self) -> Option<&(dyn StdError + 'static)> {
            match self {
                PgError::Driver(error) => Some(error),
                PgError::UnsupportedType(_) => None,
                #[cfg(feature = "deadpool-postgres")]
                PgError::Pool(_) => None,
            }
        }
    }

    impl From<tokio_postgres::Error> for PgError {
        fn from(error: tokio_postgres::Error) -> Self {
            PgError::Driver(error)
        }
    }

    #[cfg(feature = "deadpool-postgres")]
    impl From<deadpool_postgres::PoolError> for PgError {
        fn from(error: deadpool_postgres::PoolError) -> Self {
            PgError::Pool(error)
        }
    }

    /// Executes `query` through an asynchronous client without decoding rows.
    ///
    /// The helper accepts any [`tokio_postgres::GenericClient`] — a plain
    /// client, a pooled client, or an open transaction — so the parameter
    /// binding logic is shared by every asynchronous adapter.
    pub(crate) async fn fetch_raw_async<C>(
        client: &C,
        query: &BuiltQuery,
    ) -> Result<Vec<Vec<SqlVal>>, PgError>
    where
        C: tokio_postgres::GenericClient,
    {
        let params: Vec<SqlValParam<'_>> = query.params.iter().map(SqlValParam).collect();
        let refs: Vec<&(dyn ToSql + Sync)> = params
            .iter()
            .map(|param| param as &(dyn ToSql + Sync))
            .collect();
        let rows = client.query(query.sql.as_str(), &refs).await?;
        rows.iter().map(row_to_sql_vals).collect()
    }

    /// Executes `query` through an asynchronous client and returns affected rows.
    pub(crate) async fn execute_raw_async<C>(
        client: &C,
        query: &BuiltQuery,
    ) -> Result<u64, PgError>
    where
        C: tokio_postgres::GenericClient,
    {
        let params: Vec<SqlValParam<'_>> = query.params.iter().map(SqlValParam).collect();
        let refs: Vec<&(dyn ToSql + Sync)> = params
            .iter()
            .map(|param| param as &(dyn ToSql + Sync))
            .collect();
        Ok(client.execute(query.sql.as_str(), &refs).await?)
    }

    /// Executes `query` through an asynchronous client and decodes every row.
    pub(crate) async fn fetch_all_async<Q, C>(
        client: &C,
        query: &Q,
    ) -> Result<Vec<Q::Row>, ExecutionError<PgError>>
    where
        C: tokio_postgres::GenericClient,
        Q: ExecutableQuery,
        Q::Row: FromRow,
    {
        let built = query.build_query();
        let rows = fetch_raw_async(client, &built)
            .await
            .map_err(ExecutionError::Backend)?;
        rows.iter()
            .enumerate()
            .map(|(row_index, values)| {
                Q::Row::from_row(values).ok_or(ExecutionError::RowDecode { row_index })
            })
            .collect()
    }

    /// Executes `query` through a synchronous client without decoding rows.
    #[cfg(feature = "postgres")]
    pub(crate) fn fetch_raw_sync(
        client: &mut postgres::Client,
        query: &BuiltQuery,
    ) -> Result<Vec<Vec<SqlVal>>, PgError> {
        let params: Vec<SqlValParam<'_>> = query.params.iter().map(SqlValParam).collect();
        let refs: Vec<&(dyn ToSql + Sync)> = params
            .iter()
            .map(|param| param as &(dyn ToSql + Sync))
            .collect();
        let rows = client.query(query.sql.as_str(), &refs)?;
        rows.iter().map(row_to_sql_vals).collect()
    }

    /// Executes `query` through a synchronous client and returns affected rows.
    #[cfg(feature = "postgres")]
    pub(crate) fn execute_raw_sync(
        client: &mut postgres::Client,
        query: &BuiltQuery,
    ) -> Result<u64, PgError> {
        let params: Vec<SqlValParam<'_>> = query.params.iter().map(SqlValParam).collect();
        let refs: Vec<&(dyn ToSql + Sync)> = params
            .iter()
            .map(|param| param as &(dyn ToSql + Sync))
            .collect();
        Ok(client.execute(query.sql.as_str(), &refs)?)
    }

    /// A [`SqlVal`] bound to the parameter type expected by the PostgreSQL driver.
    #[derive(Debug)]
    pub(crate) struct SqlValParam<'a>(pub(crate) &'a SqlVal);

    impl ToSql for SqlValParam<'_> {
        fn to_sql(
            &self,
            ty: &Type,
            out: &mut BytesMut,
        ) -> Result<IsNull, Box<dyn StdError + Sync + Send>> {
            match self.0 {
                SqlVal::Null => Ok(IsNull::Yes),
                SqlVal::Bool(value) => value.to_sql(ty, out),
                SqlVal::I32(value) => value.to_sql(ty, out),
                SqlVal::I64(value) => value.to_sql(ty, out),
                SqlVal::F64(value) => value.to_sql(ty, out),
                SqlVal::Text(value) => value.as_str().to_sql(ty, out),
                SqlVal::Bytes(value) => value.as_slice().to_sql(ty, out),
            }
        }

        fn accepts(_ty: &Type) -> bool {
            true
        }

        fn to_sql_checked(
            &self,
            ty: &Type,
            out: &mut BytesMut,
        ) -> Result<IsNull, Box<dyn StdError + Sync + Send>> {
            self.to_sql(ty, out)
        }
    }

    /// Decodes a driver row into the ordered values RURQE rows are built from.
    pub(crate) fn row_to_sql_vals(row: &Row) -> Result<Vec<SqlVal>, PgError> {
        let mut values = Vec::with_capacity(row.columns().len());
        for (index, column) in row.columns().iter().enumerate() {
            let ty = column.type_();
            let value = if ty == &Type::BOOL {
                row.try_get::<usize, Option<bool>>(index)?
                    .map_or(SqlVal::Null, SqlVal::Bool)
            } else if ty == &Type::INT2 {
                row.try_get::<usize, Option<i16>>(index)?
                    .map_or(SqlVal::Null, |value| SqlVal::I32(i32::from(value)))
            } else if ty == &Type::INT4 {
                row.try_get::<usize, Option<i32>>(index)?
                    .map_or(SqlVal::Null, SqlVal::I32)
            } else if ty == &Type::INT8 {
                row.try_get::<usize, Option<i64>>(index)?
                    .map_or(SqlVal::Null, SqlVal::I64)
            } else if ty == &Type::FLOAT4 {
                row.try_get::<usize, Option<f32>>(index)?
                    .map_or(SqlVal::Null, |value| SqlVal::F64(f64::from(value)))
            } else if ty == &Type::FLOAT8 {
                row.try_get::<usize, Option<f64>>(index)?
                    .map_or(SqlVal::Null, SqlVal::F64)
            } else if ty == &Type::TEXT
                || ty == &Type::VARCHAR
                || ty == &Type::BPCHAR
                || ty == &Type::NAME
            {
                row.try_get::<usize, Option<String>>(index)?
                    .map_or(SqlVal::Null, SqlVal::Text)
            } else if ty == &Type::BYTEA {
                row.try_get::<usize, Option<Vec<u8>>>(index)?
                    .map_or(SqlVal::Null, SqlVal::Bytes)
            } else {
                return Err(PgError::UnsupportedType(ty.name().to_owned()));
            };
            values.push(value);
        }
        Ok(values)
    }

    #[cfg(test)]
    mod tests {
        use bytes::BytesMut;
        use rurqe_core::expr::SqlVal;
        use tokio_postgres::types::{IsNull, ToSql, Type};

        use super::SqlValParam;

        #[test]
        fn scalar_values_map_to_driver_parameters() {
            let mut buffer = BytesMut::new();
            assert!(matches!(
                SqlValParam(&SqlVal::I64(7))
                    .to_sql(&Type::INT8, &mut buffer)
                    .unwrap(),
                IsNull::No
            ));
            assert!(matches!(
                SqlValParam(&SqlVal::Bool(true))
                    .to_sql(&Type::BOOL, &mut buffer)
                    .unwrap(),
                IsNull::No
            ));
            assert!(matches!(
                SqlValParam(&SqlVal::Text("root".to_owned()))
                    .to_sql(&Type::TEXT, &mut buffer)
                    .unwrap(),
                IsNull::No
            ));
        }

        #[test]
        fn null_binds_for_any_declared_type() {
            let mut buffer = BytesMut::new();
            assert!(matches!(
                SqlValParam(&SqlVal::Null)
                    .to_sql(&Type::INT8, &mut buffer)
                    .unwrap(),
                IsNull::Yes
            ));
            assert!(matches!(
                SqlValParam(&SqlVal::Null)
                    .to_sql(&Type::UUID, &mut buffer)
                    .unwrap(),
                IsNull::Yes
            ));
        }
    }
}

/// Asynchronous execution adapter over [`tokio_postgres::Client`].
#[cfg(feature = "tokio-postgres")]
mod async_backend {
    use rurqe_core::expr::SqlVal;
    use rurqe_core::query::{BuiltQuery, ExecutableQuery, ExecutionError};
    use rurqe_core::row::FromRow;

    use crate::pg_support::{execute_raw_async, fetch_all_async, fetch_raw_async, PgError};

    /// A [`tokio_postgres::Client`] executing RURQE queries asynchronously.
    ///
    /// The adapter mirrors the synchronous execution helpers of
    /// [`QueryBackendExt`](rurqe_core::query::QueryBackendExt) with `async`
    /// methods: [`fetch_all`](PgAsyncConnection::fetch_all),
    /// [`fetch_optional`](PgAsyncConnection::fetch_optional),
    /// [`fetch_one`](PgAsyncConnection::fetch_one), and
    /// [`execute`](PgAsyncConnection::execute).
    pub struct PgAsyncConnection {
        client: tokio_postgres::Client,
    }

    impl PgAsyncConnection {
        /// Wraps an existing asynchronous PostgreSQL client.
        pub fn new(client: tokio_postgres::Client) -> Self {
            Self { client }
        }

        /// Returns a shared reference to the wrapped client.
        pub fn client(&self) -> &tokio_postgres::Client {
            &self.client
        }

        /// Executes `query` and returns every produced row as ordered values.
        pub async fn fetch_raw(&self, query: &BuiltQuery) -> Result<Vec<Vec<SqlVal>>, PgError> {
            fetch_raw_async(&self.client, query).await
        }

        /// Executes `query` without decoding rows and returns the affected rows.
        pub async fn execute_raw(&self, query: &BuiltQuery) -> Result<u64, PgError> {
            execute_raw_async(&self.client, query).await
        }

        /// Executes `query` and decodes every produced row.
        pub async fn fetch_all<Q>(&self, query: &Q) -> Result<Vec<Q::Row>, ExecutionError<PgError>>
        where
            Q: ExecutableQuery,
            Q::Row: FromRow,
        {
            fetch_all_async(&self.client, query).await
        }

        /// Executes `query` and decodes at most one row.
        pub async fn fetch_optional<Q>(
            &self,
            query: &Q,
        ) -> Result<Option<Q::Row>, ExecutionError<PgError>>
        where
            Q: ExecutableQuery,
            Q::Row: FromRow,
        {
            let rows = self.fetch_all(query).await?;
            match rows.len() {
                0 => Ok(None),
                1 => Ok(rows.into_iter().next()),
                count => Err(ExecutionError::MultipleRows { count }),
            }
        }

        /// Executes `query` and decodes exactly one row.
        pub async fn fetch_one<Q>(&self, query: &Q) -> Result<Q::Row, ExecutionError<PgError>>
        where
            Q: ExecutableQuery,
            Q::Row: FromRow,
        {
            self.fetch_optional(query)
                .await?
                .ok_or(ExecutionError::NoRows)
        }

        /// Executes `query` and returns the number of affected rows.
        pub async fn execute<Q>(&self, query: &Q) -> Result<u64, ExecutionError<PgError>>
        where
            Q: ExecutableQuery,
        {
            let built = query.build_query();
            self.execute_raw(&built)
                .await
                .map_err(ExecutionError::Backend)
        }

        /// Begins a transaction over the connection.
        ///
        /// The returned [`PgAsyncTransaction`] issues `COMMIT` only when
        /// [`commit`](PgAsyncTransaction::commit) is called and rolls the
        /// transaction back when it is dropped, so a returned error, a panic, and
        /// a cancelled future all leave the connection without an open
        /// transaction.
        pub async fn transaction(&mut self) -> Result<PgAsyncTransaction<'_>, PgError> {
            let inner = self.client.transaction().await?;
            Ok(PgAsyncTransaction { inner })
        }
    }

    /// An open transaction started by [`PgAsyncConnection::transaction`].
    ///
    /// The wrapper runs the query helpers on the transactional connection and
    /// keeps the transaction alive across `await` points. Committing is explicit:
    /// dropping the handle without calling [`commit`](PgAsyncTransaction::commit)
    /// rolls the transaction back, so a cancelled future or a panicking block
    /// never leaves partial work behind — the driver issues the `ROLLBACK` from
    /// `Drop` without awaiting anything.
    pub struct PgAsyncTransaction<'a> {
        inner: tokio_postgres::Transaction<'a>,
    }

    impl PgAsyncTransaction<'_> {
        /// Executes `query` and returns every produced row as ordered values.
        pub async fn fetch_raw(&self, query: &BuiltQuery) -> Result<Vec<Vec<SqlVal>>, PgError> {
            fetch_raw_async(&self.inner, query).await
        }

        /// Executes `query` without decoding rows and returns the affected rows.
        pub async fn execute_raw(&self, query: &BuiltQuery) -> Result<u64, PgError> {
            execute_raw_async(&self.inner, query).await
        }

        /// Executes `query` and decodes every produced row.
        pub async fn fetch_all<Q>(&self, query: &Q) -> Result<Vec<Q::Row>, ExecutionError<PgError>>
        where
            Q: ExecutableQuery,
            Q::Row: FromRow,
        {
            fetch_all_async(&self.inner, query).await
        }

        /// Executes `query` and decodes at most one row.
        pub async fn fetch_optional<Q>(
            &self,
            query: &Q,
        ) -> Result<Option<Q::Row>, ExecutionError<PgError>>
        where
            Q: ExecutableQuery,
            Q::Row: FromRow,
        {
            let rows = self.fetch_all(query).await?;
            match rows.len() {
                0 => Ok(None),
                1 => Ok(rows.into_iter().next()),
                count => Err(ExecutionError::MultipleRows { count }),
            }
        }

        /// Executes `query` and decodes exactly one row.
        pub async fn fetch_one<Q>(&self, query: &Q) -> Result<Q::Row, ExecutionError<PgError>>
        where
            Q: ExecutableQuery,
            Q::Row: FromRow,
        {
            self.fetch_optional(query)
                .await?
                .ok_or(ExecutionError::NoRows)
        }

        /// Executes `query` and returns the number of affected rows.
        pub async fn execute<Q>(&self, query: &Q) -> Result<u64, ExecutionError<PgError>>
        where
            Q: ExecutableQuery,
        {
            let built = query.build_query();
            self.execute_raw(&built)
                .await
                .map_err(ExecutionError::Backend)
        }

        /// Commits the transaction, persisting all its work.
        pub async fn commit(self) -> Result<(), PgError> {
            Ok(self.inner.commit().await?)
        }

        /// Rolls the transaction back, discarding all its work.
        pub async fn rollback(self) -> Result<(), PgError> {
            Ok(self.inner.rollback().await?)
        }
    }
}

/// Synchronous execution adapter over [`postgres::Client`].
#[cfg(feature = "postgres")]
mod sync_backend {
    use rurqe_core::expr::SqlVal;
    use rurqe_core::query::{BuiltQuery, QueryBackend, TransactionBackend};

    use crate::pg_support::{execute_raw_sync, fetch_raw_sync, PgError};

    /// A [`postgres::Client`] executing RURQE queries through the synchronous driver.
    ///
    /// The adapter implements the RURQE
    /// [`QueryBackend`](rurqe_core::query::QueryBackend) contract, so the
    /// `fetch_all`, `fetch_one`, `fetch_optional`, and `execute` helpers of
    /// [`QueryBackendExt`](rurqe_core::query::QueryBackendExt) become available.
    pub struct PgConnection {
        client: postgres::Client,
    }

    impl PgConnection {
        /// Wraps an existing synchronous PostgreSQL client.
        pub fn new(client: postgres::Client) -> Self {
            Self { client }
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
        /// The block executes between `BEGIN` and `COMMIT`; any error returned by
        /// `f` rolls the transaction back, so a failed block never leaves partial
        /// work behind. A panicking block is rolled back before the panic is
        /// resumed, so unwinding never leaves an open transaction on the
        /// connection either.
        pub fn transaction<T, E, F>(&mut self, f: F) -> Result<T, E>
        where
            F: FnOnce(&mut PgConnection) -> Result<T, E>,
            E: From<PgError>,
        {
            self.begin(None).map_err(E::from)?;
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(self)));
            match outcome {
                Ok(Ok(value)) => {
                    self.commit().map_err(E::from)?;
                    Ok(value)
                }
                Ok(Err(error)) => {
                    let _ = self.rollback();
                    Err(error)
                }
                Err(payload) => {
                    let _ = self.rollback();
                    std::panic::resume_unwind(payload)
                }
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
}

/// Asynchronous execution adapter over a [`deadpool_postgres::Pool`].
#[cfg(feature = "deadpool-postgres")]
mod deadpool_backend {
    use rurqe_core::expr::SqlVal;
    use rurqe_core::query::{BuiltQuery, ExecutableQuery, ExecutionError};
    use rurqe_core::row::FromRow;

    use crate::pg_support::{execute_raw_async, fetch_all_async, fetch_raw_async, PgError};

    /// A `deadpool-postgres` pool executing RURQE queries asynchronously.
    ///
    /// Each method checks a connection out of the pool, awaits the statement, and
    /// returns the connection, so the helpers mirror the asynchronous methods of
    /// [`PgAsyncConnection`](crate::PgAsyncConnection) without manual extraction.
    #[derive(Clone)]
    pub struct PgAsyncPool {
        pool: deadpool_postgres::Pool,
    }

    impl PgAsyncPool {
        /// Wraps an existing `deadpool-postgres` pool.
        pub fn new(pool: deadpool_postgres::Pool) -> Self {
            Self { pool }
        }

        /// Returns a shared reference to the wrapped pool.
        pub fn pool(&self) -> &deadpool_postgres::Pool {
            &self.pool
        }

        /// Executes `query` and returns every produced row as ordered values.
        pub async fn fetch_raw(&self, query: &BuiltQuery) -> Result<Vec<Vec<SqlVal>>, PgError> {
            let client = self.pool.get().await.map_err(PgError::Pool)?;
            fetch_raw_async(&*client, query).await
        }

        /// Executes `query` without decoding rows and returns the affected rows.
        pub async fn execute_raw(&self, query: &BuiltQuery) -> Result<u64, PgError> {
            let client = self.pool.get().await.map_err(PgError::Pool)?;
            execute_raw_async(&*client, query).await
        }

        /// Executes `query` and decodes every produced row.
        pub async fn fetch_all<Q>(&self, query: &Q) -> Result<Vec<Q::Row>, ExecutionError<PgError>>
        where
            Q: ExecutableQuery,
            Q::Row: FromRow,
        {
            let client = self
                .pool
                .get()
                .await
                .map_err(|error| ExecutionError::Backend(PgError::Pool(error)))?;
            fetch_all_async(&*client, query).await
        }

        /// Executes `query` and decodes at most one row.
        pub async fn fetch_optional<Q>(
            &self,
            query: &Q,
        ) -> Result<Option<Q::Row>, ExecutionError<PgError>>
        where
            Q: ExecutableQuery,
            Q::Row: FromRow,
        {
            let rows = self.fetch_all(query).await?;
            match rows.len() {
                0 => Ok(None),
                1 => Ok(rows.into_iter().next()),
                count => Err(ExecutionError::MultipleRows { count }),
            }
        }

        /// Executes `query` and decodes exactly one row.
        pub async fn fetch_one<Q>(&self, query: &Q) -> Result<Q::Row, ExecutionError<PgError>>
        where
            Q: ExecutableQuery,
            Q::Row: FromRow,
        {
            self.fetch_optional(query)
                .await?
                .ok_or(ExecutionError::NoRows)
        }

        /// Executes `query` and returns the number of affected rows.
        pub async fn execute<Q>(&self, query: &Q) -> Result<u64, ExecutionError<PgError>>
        where
            Q: ExecutableQuery,
        {
            let built = query.build_query();
            self.execute_raw(&built)
                .await
                .map_err(ExecutionError::Backend)
        }
    }
}

/// Synchronous execution adapter over an [`r2d2::Pool`].
#[cfg(feature = "r2d2")]
mod r2d2_backend {
    use std::error::Error as StdError;
    use std::fmt;

    use r2d2::ManageConnection;
    use rurqe_core::expr::SqlVal;
    use rurqe_core::query::{BuiltQuery, QueryBackend};

    use crate::pg_support::{execute_raw_sync, fetch_raw_sync, PgError};

    /// An `r2d2` pool of synchronous PostgreSQL connections.
    ///
    /// The pool implements the RURQE
    /// [`QueryBackend`](rurqe_core::query::QueryBackend) contract, so the
    /// `fetch_all`, `fetch_one`, `fetch_optional`, and `execute` helpers of
    /// [`QueryBackendExt`](rurqe_core::query::QueryBackendExt) become available
    /// directly on the pool: every call checks a connection out, runs the
    /// statement, and returns the connection to the pool.
    pub struct PgPool<M: ManageConnection> {
        pool: r2d2::Pool<M>,
    }

    impl<M: ManageConnection<Connection = postgres::Client>> PgPool<M> {
        /// Wraps an existing `r2d2` pool managing `postgres` connections.
        pub fn new(pool: r2d2::Pool<M>) -> Self {
            Self { pool }
        }

        /// Checks a connection out of the pool.
        pub fn get(&self) -> Result<r2d2::PooledConnection<M>, r2d2::Error> {
            self.pool.get()
        }
    }

    /// Error raised while executing a query through an `r2d2` PostgreSQL pool.
    #[derive(Debug)]
    pub enum PgPoolError {
        /// The connection could not be checked out of the pool.
        Pool(r2d2::Error),
        /// The statement failed to execute.
        Query(PgError),
    }

    impl fmt::Display for PgPoolError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                PgPoolError::Pool(error) => write!(f, "connection pool error: {error}"),
                PgPoolError::Query(error) => write!(f, "{error}"),
            }
        }
    }

    impl StdError for PgPoolError {
        fn source(&self) -> Option<&(dyn StdError + 'static)> {
            match self {
                PgPoolError::Pool(error) => Some(error),
                PgPoolError::Query(error) => Some(error),
            }
        }
    }

    impl<M: ManageConnection<Connection = postgres::Client>> QueryBackend for PgPool<M> {
        type Error = PgPoolError;

        fn fetch_raw(&mut self, query: &BuiltQuery) -> Result<Vec<Vec<SqlVal>>, Self::Error> {
            let mut connection = self.pool.get().map_err(PgPoolError::Pool)?;
            fetch_raw_sync(&mut *connection, query).map_err(PgPoolError::Query)
        }

        fn execute_raw(&mut self, query: &BuiltQuery) -> Result<u64, Self::Error> {
            let mut connection = self.pool.get().map_err(PgPoolError::Pool)?;
            execute_raw_sync(&mut *connection, query).map_err(PgPoolError::Query)
        }
    }
}

#[cfg(feature = "tokio-postgres")]
pub use async_backend::{PgAsyncConnection, PgAsyncTransaction};
#[cfg(feature = "deadpool-postgres")]
pub use deadpool_backend::PgAsyncPool;
#[cfg(feature = "tokio-postgres")]
pub use pg_support::PgError;
#[cfg(feature = "r2d2")]
pub use r2d2_backend::{PgPool, PgPoolError};
#[cfg(feature = "postgres")]
pub use sync_backend::PgConnection;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_standard_and_escaped_identifiers() {
        assert_eq!(Postgres::<Pg16>::quote_identifier("users"), "\"users\"");
        assert_eq!(
            Postgres::<Pg16>::quote_identifier("user\"name"),
            "\"user\"\"name\""
        );
    }

    #[test]
    fn formats_numbered_placeholders() {
        assert_eq!(Postgres::<Pg16>::placeholder(1), "$1");
        assert_eq!(Postgres::<Pg16>::placeholder(42), "$42");
    }

    #[test]
    fn capabilities_are_satisfied() {
        fn assert_capabilities<
            D: Dialect + SupportsReturning + SupportsDistinctOn + SupportsOnConflict + SupportsLateralJoin,
        >() {}

        assert_capabilities::<Postgres<Pg14>>();
        assert_capabilities::<Postgres<Pg15>>();
        assert_capabilities::<Postgres<Pg16>>();
        assert_capabilities::<Postgres>();
    }
}
