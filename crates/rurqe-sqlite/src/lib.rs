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
