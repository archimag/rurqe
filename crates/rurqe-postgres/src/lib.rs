//! PostgreSQL dialect implementation and runtime adapters for RURQE.

use core::fmt::Write;
use core::marker::PhantomData;

pub use rurqe_core;
use rurqe_core::dialect::{Dialect, SupportsDistinctOn, SupportsOnConflict, SupportsReturning};

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

/// Shared parameter binding and row decoding for the PostgreSQL driver adapters.
#[cfg(feature = "tokio-postgres")]
mod pg_support {
    use std::error::Error as StdError;
    use std::fmt;

    use bytes::BytesMut;
    use rurqe_core::expr::SqlVal;
    use tokio_postgres::types::{IsNull, ToSql, Type};
    use tokio_postgres::Row;

    /// Error raised while executing a query through a PostgreSQL driver adapter.
    #[derive(Debug)]
    pub enum PgError {
        /// The PostgreSQL driver failed to execute the statement.
        Driver(tokio_postgres::Error),
        /// A result column holds a type that cannot be decoded into a [`SqlVal`].
        UnsupportedType(String),
    }

    impl fmt::Display for PgError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                PgError::Driver(error) => write!(f, "PostgreSQL driver error: {error}"),
                PgError::UnsupportedType(name) => write!(
                    f,
                    "unsupported PostgreSQL column type `{name}`: cannot decode into a SQL value"
                ),
            }
        }
    }

    impl StdError for PgError {
        fn source(&self) -> Option<&(dyn StdError + 'static)> {
            match self {
                PgError::Driver(error) => Some(error),
                PgError::UnsupportedType(_) => None,
            }
        }
    }

    impl From<tokio_postgres::Error> for PgError {
        fn from(error: tokio_postgres::Error) -> Self {
            PgError::Driver(error)
        }
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
            assert_eq!(
                SqlValParam(&SqlVal::I64(7))
                    .to_sql(&Type::INT8, &mut buffer)
                    .unwrap(),
                IsNull::No
            );
            assert_eq!(
                SqlValParam(&SqlVal::Bool(true))
                    .to_sql(&Type::BOOL, &mut buffer)
                    .unwrap(),
                IsNull::No
            );
            assert_eq!(
                SqlValParam(&SqlVal::Text("root".to_owned()))
                    .to_sql(&Type::TEXT, &mut buffer)
                    .unwrap(),
                IsNull::No
            );
        }

        #[test]
        fn null_binds_for_any_declared_type() {
            let mut buffer = BytesMut::new();
            assert_eq!(
                SqlValParam(&SqlVal::Null)
                    .to_sql(&Type::INT8, &mut buffer)
                    .unwrap(),
                IsNull::Yes
            );
            assert_eq!(
                SqlValParam(&SqlVal::Null)
                    .to_sql(&Type::UUID, &mut buffer)
                    .unwrap(),
                IsNull::Yes
            );
        }
    }
}

/// Asynchronous execution adapter over [`tokio_postgres::Client`].
#[cfg(feature = "tokio-postgres")]
mod async_backend {
    use rurqe_core::expr::SqlVal;
    use rurqe_core::query::{BuiltQuery, ExecutableQuery, ExecutionError};
    use rurqe_core::row::FromRow;
    use tokio_postgres::types::ToSql;

    use crate::pg_support::{row_to_sql_vals, PgError, SqlValParam};

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
            let params: Vec<SqlValParam<'_>> = query.params.iter().map(SqlValParam).collect();
            let refs: Vec<&(dyn ToSql + Sync)> = params
                .iter()
                .map(|param| param as &(dyn ToSql + Sync))
                .collect();
            let rows = self.client.query(query.sql.as_str(), &refs).await?;
            rows.iter().map(row_to_sql_vals).collect()
        }

        /// Executes `query` without decoding rows and returns the affected rows.
        pub async fn execute_raw(&self, query: &BuiltQuery) -> Result<u64, PgError> {
            let params: Vec<SqlValParam<'_>> = query.params.iter().map(SqlValParam).collect();
            let refs: Vec<&(dyn ToSql + Sync)> = params
                .iter()
                .map(|param| param as &(dyn ToSql + Sync))
                .collect();
            Ok(self.client.execute(query.sql.as_str(), &refs).await?)
        }

        /// Executes `query` and decodes every produced row.
        pub async fn fetch_all<Q>(&self, query: &Q) -> Result<Vec<Q::Row>, ExecutionError<PgError>>
        where
            Q: ExecutableQuery,
            Q::Row: FromRow,
        {
            let built = query.build_query();
            let rows = self
                .fetch_raw(&built)
                .await
                .map_err(ExecutionError::Backend)?;
            rows.iter()
                .enumerate()
                .map(|(row_index, values)| {
                    Q::Row::from_row(values).ok_or(ExecutionError::RowDecode { row_index })
                })
                .collect()
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

/// Synchronous execution adapter over [`postgres::Client`].
#[cfg(feature = "postgres")]
mod sync_backend {
    use postgres::types::ToSql;
    use rurqe_core::expr::SqlVal;
    use rurqe_core::query::{BuiltQuery, QueryBackend};

    use crate::pg_support::{row_to_sql_vals, PgError, SqlValParam};

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
    }

    impl QueryBackend for PgConnection {
        type Error = PgError;

        fn fetch_raw(&mut self, query: &BuiltQuery) -> Result<Vec<Vec<SqlVal>>, PgError> {
            let params: Vec<SqlValParam<'_>> = query.params.iter().map(SqlValParam).collect();
            let refs: Vec<&(dyn ToSql + Sync)> = params
                .iter()
                .map(|param| param as &(dyn ToSql + Sync))
                .collect();
            let rows = self.client.query(query.sql.as_str(), &refs)?;
            rows.iter().map(row_to_sql_vals).collect()
        }

        fn execute_raw(&mut self, query: &BuiltQuery) -> Result<u64, PgError> {
            let params: Vec<SqlValParam<'_>> = query.params.iter().map(SqlValParam).collect();
            let refs: Vec<&(dyn ToSql + Sync)> = params
                .iter()
                .map(|param| param as &(dyn ToSql + Sync))
                .collect();
            Ok(self.client.execute(query.sql.as_str(), &refs)?)
        }
    }
}

#[cfg(feature = "tokio-postgres")]
pub use async_backend::PgAsyncConnection;
#[cfg(feature = "tokio-postgres")]
pub use pg_support::PgError;
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
        fn assert_capabilities<D: Dialect + SupportsReturning + SupportsDistinctOn + SupportsOnConflict>() {}

        assert_capabilities::<Postgres<Pg14>>();
        assert_capabilities::<Postgres<Pg15>>();
        assert_capabilities::<Postgres<Pg16>>();
        assert_capabilities::<Postgres>();
    }
}
