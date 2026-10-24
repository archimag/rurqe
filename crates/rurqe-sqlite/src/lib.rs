//! SQLite dialect implementation and runtime adapters for RURQE.
//!
//! The dialect is parameterised by a type-level engine version, so SQL features
//! introduced by newer SQLite releases exist only for versions that actually
//! support them:
//!
//! - [`Sqlite3_0`] — baseline dialect without `ON CONFLICT` or `RETURNING`.
//! - [`Sqlite3_24`] — adds `ON CONFLICT` upsert syntax.
//! - [`Sqlite3_35`] — adds the `RETURNING` clause and `UPDATE ... FROM`.
//!
//! ```
//! use rurqe_core::dialect::Dialect;
//! use rurqe_sqlite::{Sqlite, Sqlite3_35};
//!
//! assert_eq!(Sqlite::<Sqlite3_35>::quote_identifier("users"), "\"users\"");
//! assert_eq!(Sqlite::<Sqlite3_35>::placeholder(1), "?");
//! ```

use core::marker::PhantomData;

pub use rurqe_core;
use rurqe_core::dialect::{
    Dialect, SupportsAggregateFilter, SupportsJsonAggregation, SupportsNullsOrdering,
    SupportsOnConflict, SupportsReturning, SupportsUpdateFrom, SupportsWindowFunctions,
};

/// Marker trait representing a SQLite engine version.
pub trait SqliteVersion: 'static + Send + Sync {}

/// Threshold trait fulfilled by versions at least as recent as `MAJOR.MINOR`.
///
/// Capability traits are implemented only for versions satisfying the threshold
/// that introduced them, which keeps unsupported SQL out of reach at compile time.
pub trait SqliteVersionAtLeast<const MAJOR: u32, const MINOR: u32>: SqliteVersion {}

/// SQLite 3.0 marker: the baseline dialect without upserts or returning clauses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Sqlite3_0;
impl SqliteVersion for Sqlite3_0 {}
impl SqliteVersionAtLeast<3, 0> for Sqlite3_0 {}

/// SQLite 3.24 marker: introduces `ON CONFLICT` upsert syntax.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Sqlite3_24;
impl SqliteVersion for Sqlite3_24 {}
impl SqliteVersionAtLeast<3, 0> for Sqlite3_24 {}
impl SqliteVersionAtLeast<3, 24> for Sqlite3_24 {}

/// SQLite 3.25 marker: introduces window functions (`OVER (...)`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Sqlite3_25;
impl SqliteVersion for Sqlite3_25 {}
impl SqliteVersionAtLeast<3, 0> for Sqlite3_25 {}
impl SqliteVersionAtLeast<3, 24> for Sqlite3_25 {}
impl SqliteVersionAtLeast<3, 25> for Sqlite3_25 {}

/// SQLite 3.30 marker: introduces `FILTER (WHERE ...)` clauses on aggregate functions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Sqlite3_30;
impl SqliteVersion for Sqlite3_30 {}
impl SqliteVersionAtLeast<3, 0> for Sqlite3_30 {}
impl SqliteVersionAtLeast<3, 24> for Sqlite3_30 {}
impl SqliteVersionAtLeast<3, 25> for Sqlite3_30 {}
impl SqliteVersionAtLeast<3, 30> for Sqlite3_30 {}

/// SQLite 3.35 marker: introduces the `RETURNING` clause on DML statements.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Sqlite3_35;
impl SqliteVersion for Sqlite3_35 {}
impl SqliteVersionAtLeast<3, 0> for Sqlite3_35 {}
impl SqliteVersionAtLeast<3, 24> for Sqlite3_35 {}
impl SqliteVersionAtLeast<3, 25> for Sqlite3_35 {}
impl SqliteVersionAtLeast<3, 30> for Sqlite3_35 {}
impl SqliteVersionAtLeast<3, 35> for Sqlite3_35 {}

/// SQLite dialect parameterised by engine version (defaulting to [`Sqlite3_35`]).
///
/// Identifiers are double-quoted and escaped by doubling embedded quotes, while
/// bind parameters use anonymous `?` placeholders.
///
/// Versions below 3.24 do not implement [`SupportsOnConflict`]:
///
/// ```compile_fail
/// use rurqe_core::dialect::SupportsOnConflict;
/// use rurqe_sqlite::{Sqlite, Sqlite3_0};
///
/// fn assert_on_conflict<D: SupportsOnConflict>() {}
/// assert_on_conflict::<Sqlite<Sqlite3_0>>();
/// ```
///
/// Versions below 3.35 do not implement [`SupportsReturning`]:
///
/// ```compile_fail
/// use rurqe_core::dialect::SupportsReturning;
/// use rurqe_sqlite::{Sqlite, Sqlite3_24};
///
/// fn assert_returning<D: SupportsReturning>() {}
/// assert_returning::<Sqlite<Sqlite3_24>>();
/// ```
///
/// SQLite has no `DELETE ... USING` clause, so the `SupportsDeleteUsing`
/// capability is never implemented and the clause stays unreachable:
///
/// ```compile_fail
/// use rurqe_core::dialect::SupportsDeleteUsing;
/// use rurqe_sqlite::{Sqlite, Sqlite3_35};
///
/// fn assert_delete_using<D: SupportsDeleteUsing>() {}
/// assert_delete_using::<Sqlite<Sqlite3_35>>();
/// ```
///
/// SQLite gained `UPDATE ... FROM` in 3.33.0, so versions older than the newest
/// pinned marker do not implement [`SupportsUpdateFrom`]:
///
/// ```compile_fail
/// use rurqe_core::dialect::SupportsUpdateFrom;
/// use rurqe_sqlite::{Sqlite, Sqlite3_24};
///
/// fn assert_update_from<D: SupportsUpdateFrom>() {}
/// assert_update_from::<Sqlite<Sqlite3_24>>();
/// ```
///
/// SQLite has no `LATERAL` joins, so the `SupportsLateralJoin` capability is
/// never implemented either:
///
/// ```compile_fail
/// use rurqe_core::dialect::SupportsLateralJoin;
/// use rurqe_sqlite::{Sqlite, Sqlite3_35};
///
/// fn assert_lateral<D: SupportsLateralJoin>() {}
/// assert_lateral::<Sqlite<Sqlite3_35>>();
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Sqlite<V: SqliteVersion = Sqlite3_35>(PhantomData<fn() -> V>);

impl<V: SqliteVersion> Dialect for Sqlite<V> {
    // SQLite has no row-level JSON aggregation, so a nested collection assembles
    // every object field by field and collects the objects with
    // `json_group_array`.
    const SUPPORTS_JSONB_ROW_AGG: bool = false;

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

    fn push_placeholder(out: &mut String, _index: usize) {
        out.push('?');
    }
}

impl<V: SqliteVersion + SqliteVersionAtLeast<3, 35>> SupportsReturning for Sqlite<V> {}

impl<V: SqliteVersion + SqliteVersionAtLeast<3, 24>> SupportsOnConflict for Sqlite<V> {}

// `UPDATE ... FROM` belongs to SQLite since 3.33.0, so the newest pinned marker is
// the first one that satisfies it. SQLite has no `DELETE ... USING` clause at all,
// so `SupportsDeleteUsing` stays unimplemented for every version.
impl<V: SqliteVersion + SqliteVersionAtLeast<3, 35>> SupportsUpdateFrom for Sqlite<V> {}

impl<V: SqliteVersion + SqliteVersionAtLeast<3, 30>> SupportsAggregateFilter for Sqlite<V> {}

// Window functions belong to SQLite since 3.25.0, and explicit `NULLS FIRST` /
// `NULLS LAST` since 3.30.0, so each capability follows the threshold that
// introduced it. `GREATEST` and `LEAST` were never added to the engine (its
// `max`/`min` scalar functions serve the same purpose), and `IS DISTINCT FROM`
// arrived only in 3.39.0, past the newest pinned marker, so neither capability is
// implemented for any declared version.
impl<V: SqliteVersion + SqliteVersionAtLeast<3, 25>> SupportsWindowFunctions for Sqlite<V> {}

impl<V: SqliteVersion + SqliteVersionAtLeast<3, 30>> SupportsNullsOrdering for Sqlite<V> {}

impl<V: SqliteVersion + SqliteVersionAtLeast<3, 24>> SupportsJsonAggregation for Sqlite<V> {}

/// Re-export of the underlying `rusqlite` crate for adapter users.
#[cfg(feature = "rusqlite")]
pub use ::rusqlite;

/// A `rusqlite` connection extended with the RURQE execution contract.
#[cfg(feature = "rusqlite")]
pub use rusqlite_backend::SqliteConnection;

#[cfg(feature = "rusqlite")]
mod rusqlite_backend {
    use core::ops::{Deref, DerefMut};

    use ::rusqlite::types::{ToSql, ToSqlOutput, ValueRef};
    use ::rusqlite::Connection;

    use rurqe_core::expr::SqlVal;
    use rurqe_core::query::{
        BuiltQuery, IsolationLevel, QueryBackend, TransactionBackend, TransactionGuard,
        TransactionStatus, TransactionalConnection,
    };

    /// A `rusqlite` connection that executes RURQE-built queries.
    ///
    /// The adapter implements the RURQE
    /// [`QueryBackend`](rurqe_core::query::QueryBackend) contract, so the
    /// `fetch_all`, `fetch_one`, `fetch_optional`, and `execute` helpers of
    /// [`QueryBackendExt`](rurqe_core::query::QueryBackendExt) become available.
    /// It also dereferences to the wrapped [`Connection`], so raw SQL can still be
    /// executed directly.
    pub struct SqliteConnection {
        connection: Connection,
        status: TransactionStatus,
    }

    impl SqliteConnection {
        /// Wraps an existing `rusqlite` connection.
        pub fn new(connection: Connection) -> Self {
            Self {
                connection,
                status: TransactionStatus::new(),
            }
        }

        /// Opens a new in-memory SQLite database.
        pub fn open_in_memory() -> Result<Self, ::rusqlite::Error> {
            Connection::open_in_memory().map(Self::new)
        }

        /// Returns a shared reference to the wrapped connection.
        pub fn connection(&self) -> &Connection {
            &self.connection
        }

        /// Returns a mutable reference to the wrapped connection.
        pub fn connection_mut(&mut self) -> &mut Connection {
            &mut self.connection
        }

        /// Consumes the adapter, returning the wrapped connection.
        pub fn into_inner(self) -> Connection {
            self.connection
        }

        /// Runs `f` inside a transaction, committing on `Ok` and rolling back on `Err`.
        ///
        /// When no transaction is open the block runs between `BEGIN` and `COMMIT`.
        /// Calling `transaction` again while a transaction is already open nests the
        /// block inside a `SAVEPOINT`, so an inner failure rolls back only its own
        /// statements while the enclosing transaction stays intact. A panic raised by
        /// `f` is rolled back as well, so the transaction can never be left
        /// half-finished.
        pub fn transaction<T, E, F>(&mut self, f: F) -> Result<T, E>
        where
            F: FnOnce(&mut SqliteConnection) -> Result<T, E>,
            E: From<::rusqlite::Error>,
        {
            let mut guard = TransactionGuard::begin(self).map_err(E::from)?;
            let value = f(&mut guard)?;
            guard.commit().map_err(E::from)?;
            Ok(value)
        }
    }

    impl TransactionalConnection for SqliteConnection {
        fn transaction_status(&mut self) -> &mut TransactionStatus {
            &mut self.status
        }
    }

    impl TransactionBackend for SqliteConnection {
        fn begin(&mut self, _isolation: Option<IsolationLevel>) -> Result<(), Self::Error> {
            // SQLite serialises access through database-level locking, so the
            // requested isolation level does not map onto a `BEGIN` clause.
            self.connection.execute_batch("BEGIN")
        }
    }

    impl Deref for SqliteConnection {
        type Target = Connection;

        fn deref(&self) -> &Self::Target {
            &self.connection
        }
    }

    impl DerefMut for SqliteConnection {
        fn deref_mut(&mut self) -> &mut Self::Target {
            &mut self.connection
        }
    }

    /// Bridges a [`SqlVal`] into the `rusqlite` parameter interface.
    struct SqlValParam<'a>(&'a SqlVal);

    impl ToSql for SqlValParam<'_> {
        fn to_sql(&self) -> ::rusqlite::Result<ToSqlOutput<'_>> {
            match self.0 {
                SqlVal::Null => Ok(ToSqlOutput::Borrowed(ValueRef::Null)),
                SqlVal::I32(value) => {
                    Ok(ToSqlOutput::Borrowed(ValueRef::Integer(i64::from(*value))))
                }
                SqlVal::I64(value) => Ok(ToSqlOutput::Borrowed(ValueRef::Integer(*value))),
                SqlVal::F64(value) => Ok(ToSqlOutput::Borrowed(ValueRef::Real(*value))),
                SqlVal::Bool(value) => Ok(ToSqlOutput::Borrowed(ValueRef::Integer(*value as i64))),
                SqlVal::Text(value) => Ok(ToSqlOutput::Borrowed(ValueRef::Text(value.as_bytes()))),
                SqlVal::Bytes(value) => Ok(ToSqlOutput::Borrowed(ValueRef::Blob(value.as_slice()))),
                // SQLite has no array column type, so an array binds as the JSON
                // document of its elements: that is the form
                // `Vec<T>::from_sql_value` parses back into the elements of the
                // array, and the form an aggregated row set already arrives in.
                // Rendering the value through its `Display` form instead would
                // bind PostgreSQL's `{...}` spelling, which SQLite neither stores
                // as a document nor decodes back into a collection.
                SqlVal::Array(values) => Ok(ToSqlOutput::Owned(::rusqlite::types::Value::Text(
                    json_array(values)?,
                ))),
                SqlVal::EncodingError(message) => Err(::rusqlite::Error::ToSqlConversionFailure(
                    Box::new(::std::io::Error::other(message.clone())),
                )),
                // The variants left are the feature-gated scalar families
                // (`decimal`, `uuid`, `chrono`, `serde_json`), whose textual form
                // is the value SQLite stores for the column they belong to.
                #[allow(unreachable_patterns)]
                other => Ok(ToSqlOutput::Owned(::rusqlite::types::Value::Text(
                    other.to_string(),
                ))),
            }
        }
    }

    /// Renders an array value as the JSON document bound for its parameter.
    fn json_array(values: &[SqlVal]) -> ::rusqlite::Result<String> {
        let mut document = String::from("[");
        for (index, value) in values.iter().enumerate() {
            if index > 0 {
                document.push(',');
            }
            push_json_value(&mut document, value)?;
        }
        document.push(']');

        Ok(document)
    }

    /// Appends one element of an array document in its JSON form.
    ///
    /// A value that cannot be encoded aborts the whole parameter instead of
    /// entering the document as a substitute, so the driver reports the element
    /// that failed rather than binding a document of the wrong shape.
    fn push_json_value(out: &mut String, value: &SqlVal) -> ::rusqlite::Result<()> {
        match value {
            SqlVal::Null => out.push_str("null"),
            SqlVal::I32(value) => out.push_str(&value.to_string()),
            SqlVal::I64(value) => out.push_str(&value.to_string()),
            SqlVal::F64(value) if value.is_finite() => out.push_str(&value.to_string()),
            // JSON carries no notation for a non-finite number.
            SqlVal::F64(_) => out.push_str("null"),
            SqlVal::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
            SqlVal::Text(value) => push_json_string(out, value),
            // A blob has no JSON counterpart, so its octets render as a list of
            // numbers, which keeps the document parseable.
            SqlVal::Bytes(octets) => {
                out.push('[');
                for (index, octet) in octets.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    out.push_str(&octet.to_string());
                }
                out.push(']');
            }
            SqlVal::Array(values) => {
                out.push('[');
                for (index, value) in values.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    push_json_value(out, value)?;
                }
                out.push(']');
            }
            SqlVal::EncodingError(message) => {
                return Err(::rusqlite::Error::ToSqlConversionFailure(Box::new(
                    ::std::io::Error::other(message.clone()),
                )));
            }
            // A feature-gated scalar family renders in its textual form, which is
            // the value the crate parses back out of the element.
            #[allow(unreachable_patterns)]
            other => push_json_string(out, &other.to_string()),
        }

        Ok(())
    }

    /// Appends `value` as a quoted JSON string, escaping what JSON requires.
    fn push_json_string(out: &mut String, value: &str) {
        out.push('"');
        for ch in value.chars() {
            match ch {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                // JSON requires every other control character to be escaped.
                ch if (ch as u32) < 0x20 => {
                    out.push_str(&format!("\\u{:04x}", ch as u32));
                }
                ch => out.push(ch),
            }
        }
        out.push('"');
    }

    /// Decodes a raw SQLite value into the matching [`SqlVal`] variant.
    ///
    /// Text columns must hold valid UTF-8: a value that is not valid UTF-8 is
    /// reported as a conversion failure for the offending column instead of being
    /// silently rewritten with replacement characters.
    fn value_ref_to_sql_val(
        index: usize,
        value: ValueRef<'_>,
    ) -> Result<SqlVal, ::rusqlite::Error> {
        Ok(match value {
            ValueRef::Null => SqlVal::Null,
            ValueRef::Integer(value) => SqlVal::I64(value),
            ValueRef::Real(value) => SqlVal::F64(value),
            ValueRef::Text(bytes) => {
                SqlVal::Text(String::from_utf8(bytes.to_vec()).map_err(|error| {
                    ::rusqlite::Error::FromSqlConversionFailure(
                        index,
                        ::rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?)
            }
            ValueRef::Blob(bytes) => SqlVal::Bytes(bytes.to_vec()),
        })
    }

    pub(crate) fn fetch_raw_with(
        connection: &mut Connection,
        query: &BuiltQuery,
    ) -> Result<Vec<Vec<SqlVal>>, ::rusqlite::Error> {
        let mut statement = connection.prepare(&query.sql)?;
        let column_count = statement.column_count();
        let params = query.params.iter().map(SqlValParam);
        let mut rows = statement.query(::rusqlite::params_from_iter(params))?;

        let mut result = Vec::new();
        while let Some(row) = rows.next()? {
            let mut values = Vec::with_capacity(column_count);
            for index in 0..column_count {
                values.push(value_ref_to_sql_val(index, row.get_ref(index)?)?);
            }
            result.push(values);
        }
        Ok(result)
    }

    pub(crate) fn execute_raw_with(
        connection: &mut Connection,
        query: &BuiltQuery,
    ) -> Result<u64, ::rusqlite::Error> {
        let params = query.params.iter().map(SqlValParam);
        let affected = connection.execute(&query.sql, ::rusqlite::params_from_iter(params))?;
        Ok(affected as u64)
    }

    impl QueryBackend for SqliteConnection {
        type Error = ::rusqlite::Error;

        fn fetch_raw(&mut self, query: &BuiltQuery) -> Result<Vec<Vec<SqlVal>>, Self::Error> {
            fetch_raw_with(&mut self.connection, query)
        }

        fn execute_raw(&mut self, query: &BuiltQuery) -> Result<u64, Self::Error> {
            execute_raw_with(&mut self.connection, query)
        }
    }
}

#[cfg(feature = "r2d2")]
mod r2d2_pool {
    use std::error::Error as StdError;
    use std::fmt;

    use r2d2::ManageConnection;
    use rurqe_core::expr::SqlVal;
    use rurqe_core::query::{BuiltQuery, QueryBackend};

    use crate::rusqlite_backend::{execute_raw_with, fetch_raw_with};

    /// An `r2d2` pool of SQLite connections.
    ///
    /// The pool implements the RURQE
    /// [`QueryBackend`](rurqe_core::query::QueryBackend) contract, so the
    /// `fetch_all`, `fetch_one`, `fetch_optional`, and `execute` helpers of
    /// [`QueryBackendExt`](rurqe_core::query::QueryBackendExt) become available
    /// directly on the pool: every call checks a connection out, runs the
    /// statement, and returns the connection to the pool.
    ///
    /// ```
    /// use r2d2_sqlite::SqliteConnectionManager;
    ///
    /// let manager = SqliteConnectionManager::memory();
    /// let pool = r2d2::Pool::builder().max_size(4).build(manager).unwrap();
    /// let _pool = rurqe_sqlite::SqlitePool::new(pool);
    /// ```
    pub struct SqlitePool<M: ManageConnection> {
        pool: r2d2::Pool<M>,
    }

    impl<M: ManageConnection<Connection = ::rusqlite::Connection>> SqlitePool<M> {
        /// Wraps an existing `r2d2` pool managing `rusqlite` connections.
        pub fn new(pool: r2d2::Pool<M>) -> Self {
            Self { pool }
        }

        /// Checks a connection out of the pool.
        pub fn get(&self) -> Result<r2d2::PooledConnection<M>, r2d2::Error> {
            self.pool.get()
        }
    }

    /// Error raised while executing a query through an `r2d2` SQLite pool.
    #[derive(Debug)]
    pub enum SqlitePoolError {
        /// The connection could not be checked out of the pool.
        Pool(r2d2::Error),
        /// The statement failed to execute.
        Query(::rusqlite::Error),
    }

    impl fmt::Display for SqlitePoolError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                SqlitePoolError::Pool(error) => write!(f, "connection pool error: {error}"),
                SqlitePoolError::Query(error) => write!(f, "SQLite query error: {error}"),
            }
        }
    }

    impl StdError for SqlitePoolError {
        fn source(&self) -> Option<&(dyn StdError + 'static)> {
            match self {
                SqlitePoolError::Pool(error) => Some(error),
                SqlitePoolError::Query(error) => Some(error),
            }
        }
    }

    impl<M: ManageConnection<Connection = ::rusqlite::Connection>> QueryBackend for SqlitePool<M> {
        type Error = SqlitePoolError;

        fn fetch_raw(&mut self, query: &BuiltQuery) -> Result<Vec<Vec<SqlVal>>, Self::Error> {
            let mut connection = self.pool.get().map_err(SqlitePoolError::Pool)?;
            fetch_raw_with(&mut *connection, query).map_err(SqlitePoolError::Query)
        }

        fn execute_raw(&mut self, query: &BuiltQuery) -> Result<u64, Self::Error> {
            let mut connection = self.pool.get().map_err(SqlitePoolError::Pool)?;
            execute_raw_with(&mut *connection, query).map_err(SqlitePoolError::Query)
        }
    }
}

#[cfg(feature = "r2d2")]
pub use r2d2_pool::{SqlitePool, SqlitePoolError};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_standard_and_escaped_identifiers() {
        assert_eq!(Sqlite::<Sqlite3_35>::quote_identifier("users"), "\"users\"");
        assert_eq!(
            Sqlite::<Sqlite3_35>::quote_identifier("user\"name"),
            "\"user\"\"name\""
        );
        assert_eq!(
            Sqlite::<Sqlite3_0>::quote_identifier("select"),
            "\"select\""
        );
    }

    #[test]
    fn formats_anonymous_placeholders() {
        assert_eq!(Sqlite::<Sqlite3_35>::placeholder(1), "?");
        assert_eq!(Sqlite::<Sqlite3_35>::placeholder(42), "?");
        assert_eq!(Sqlite::<Sqlite3_0>::placeholder(7), "?");
    }

    #[test]
    fn capabilities_are_gated_by_version() {
        fn assert_returning<D: Dialect + SupportsReturning>() {}
        fn assert_on_conflict<D: Dialect + SupportsOnConflict>() {}
        fn assert_update_from<D: Dialect + SupportsUpdateFrom>() {}
        fn assert_aggregate_filter<D: Dialect + SupportsAggregateFilter>() {}

        assert_on_conflict::<Sqlite<Sqlite3_24>>();
        assert_on_conflict::<Sqlite<Sqlite3_30>>();
        assert_on_conflict::<Sqlite<Sqlite3_35>>();

        assert_returning::<Sqlite<Sqlite3_35>>();
        assert_returning::<Sqlite>();
        assert_on_conflict::<Sqlite>();

        assert_aggregate_filter::<Sqlite<Sqlite3_30>>();
        assert_aggregate_filter::<Sqlite<Sqlite3_35>>();
        assert_aggregate_filter::<Sqlite>();

        assert_update_from::<Sqlite<Sqlite3_35>>();
        assert_update_from::<Sqlite>();
    }

    #[test]
    fn expression_capabilities_are_gated_by_version() {
        fn assert_window_functions<D: Dialect + SupportsWindowFunctions>() {}
        fn assert_nulls_ordering<D: Dialect + SupportsNullsOrdering>() {}
        fn assert_json_aggregation<D: Dialect + SupportsJsonAggregation>() {}

        assert_window_functions::<Sqlite<Sqlite3_25>>();
        assert_window_functions::<Sqlite<Sqlite3_30>>();
        assert_window_functions::<Sqlite<Sqlite3_35>>();
        assert_window_functions::<Sqlite>();

        assert_nulls_ordering::<Sqlite<Sqlite3_30>>();
        assert_nulls_ordering::<Sqlite<Sqlite3_35>>();
        assert_nulls_ordering::<Sqlite>();

        assert_json_aggregation::<Sqlite<Sqlite3_24>>();
        assert_json_aggregation::<Sqlite<Sqlite3_25>>();
        assert_json_aggregation::<Sqlite<Sqlite3_30>>();
        assert_json_aggregation::<Sqlite<Sqlite3_35>>();
        assert_json_aggregation::<Sqlite>();
    }
}
