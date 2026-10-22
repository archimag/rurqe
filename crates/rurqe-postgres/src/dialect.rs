//! PostgreSQL dialect implementation and engine version markers.

use core::fmt::Write;
use core::marker::PhantomData;

use rurqe_core::dialect::{
    Dialect, SupportsDeleteUsing, SupportsDistinctOn, SupportsLateralJoin, SupportsOnConflict,
    SupportsReturning, SupportsRowLocking, SupportsUpdateFrom,
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
impl<V: PgVersion> SupportsDeleteUsing for Postgres<V> {}
impl<V: PgVersion> SupportsUpdateFrom for Postgres<V> {}
impl<V: PgVersion> SupportsRowLocking for Postgres<V> {}

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
            D: Dialect
                + SupportsReturning
                + SupportsDistinctOn
                + SupportsOnConflict
                + SupportsLateralJoin
                + SupportsDeleteUsing
                + SupportsUpdateFrom,
        >() {}

        assert_capabilities::<Postgres<Pg14>>();
        assert_capabilities::<Postgres<Pg15>>();
        assert_capabilities::<Postgres<Pg16>>();
        assert_capabilities::<Postgres>();
    }

    #[test]
    fn row_level_locking_is_a_postgres_capability() {
        fn assert_locking<D: Dialect + SupportsRowLocking>() {}

        assert_locking::<Postgres<Pg14>>();
        assert_locking::<Postgres<Pg15>>();
        assert_locking::<Postgres<Pg16>>();
        assert_locking::<Postgres>();
    }
}
