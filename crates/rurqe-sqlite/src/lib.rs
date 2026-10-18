//! SQLite dialect implementation and runtime adapters for RURQE.
//!
//! The dialect is parameterised by a type-level engine version, so SQL features
//! introduced by newer SQLite releases exist only for versions that actually
//! support them:
//!
//! - [`Sqlite3_0`] — baseline dialect without `ON CONFLICT` or `RETURNING`.
//! - [`Sqlite3_24`] — adds `ON CONFLICT` upsert syntax.
//! - [`Sqlite3_35`] — adds the `RETURNING` clause.
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
use rurqe_core::dialect::{Dialect, SupportsOnConflict, SupportsReturning};

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

/// SQLite 3.35 marker: introduces the `RETURNING` clause on DML statements.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Sqlite3_35;
impl SqliteVersion for Sqlite3_35 {}
impl SqliteVersionAtLeast<3, 0> for Sqlite3_35 {}
impl SqliteVersionAtLeast<3, 24> for Sqlite3_35 {}
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Sqlite<V: SqliteVersion = Sqlite3_35>(PhantomData<fn() -> V>);

impl<V: SqliteVersion> Dialect for Sqlite<V> {
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
    use rurqe_core::query::{BuiltQuery, QueryBackend};

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
    }

    impl SqliteConnection {
        /// Wraps an existing `rusqlite` connection.
        pub fn new(connection: Connection) -> Self {
            Self { connection }
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
            let value_ref = match self.0 {
                SqlVal::Null => ValueRef::Null,
                SqlVal::I32(value) => ValueRef::Integer(i64::from(*value)),
                SqlVal::I64(value) => ValueRef::Integer(*value),
                SqlVal::F64(value) => ValueRef::Real(*value),
                SqlVal::Bool(value) => ValueRef::Integer(*value as i64),
                SqlVal::Text(value) => ValueRef::Text(value.as_bytes()),
            };
            Ok(ToSqlOutput::Borrowed(value_ref))
        }
    }

    /// Decodes a raw SQLite value into the matching [`SqlVal`] variant.
    fn value_ref_to_sql_val(value: ValueRef<'_>) -> SqlVal {
        match value {
            ValueRef::Null => SqlVal::Null,
            ValueRef::Integer(value) => SqlVal::I64(value),
            ValueRef::Real(value) => SqlVal::F64(value),
            ValueRef::Text(bytes) => SqlVal::Text(String::from_utf8_lossy(bytes).into_owned()),
            ValueRef::Blob(bytes) => SqlVal::Text(String::from_utf8_lossy(bytes).into_owned()),
        }
    }

    impl QueryBackend for SqliteConnection {
        type Error = ::rusqlite::Error;

        fn fetch_raw(&mut self, query: &BuiltQuery) -> Result<Vec<Vec<SqlVal>>, Self::Error> {
            let mut statement = self.connection.prepare(&query.sql)?;
            let column_count = statement.column_count();
            let params = query.params.iter().map(SqlValParam);
            let mut rows = statement.query(::rusqlite::params_from_iter(params))?;

            let mut result = Vec::new();
            while let Some(row) = rows.next()? {
                let mut values = Vec::with_capacity(column_count);
                for index in 0..column_count {
                    values.push(value_ref_to_sql_val(row.get_ref(index)?));
                }
                result.push(values);
            }
            Ok(result)
        }

        fn execute_raw(&mut self, query: &BuiltQuery) -> Result<u64, Self::Error> {
            let params = query.params.iter().map(SqlValParam);
            let affected = self
                .connection
                .execute(&query.sql, ::rusqlite::params_from_iter(params))?;
            Ok(affected as u64)
        }
    }
}

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

        assert_on_conflict::<Sqlite<Sqlite3_24>>();
        assert_on_conflict::<Sqlite<Sqlite3_35>>();

        assert_returning::<Sqlite<Sqlite3_35>>();
        assert_returning::<Sqlite>();
        assert_on_conflict::<Sqlite>();
    }
}
