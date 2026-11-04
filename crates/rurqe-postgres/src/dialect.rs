//! PostgreSQL dialect implementation and engine version markers.

use core::fmt::Write;
use core::marker::PhantomData;

use rurqe_core::dialect::{
    Dialect, SupportsAggregateFilter, SupportsAliasedInsertTarget, SupportsDeleteUsing,
    SupportsDistinctFrom, SupportsDistinctOn, SupportsFullOuterJoin, SupportsGreatest,
    SupportsJsonAggregation, SupportsLateralJoin, SupportsNullsOrdering, SupportsOnConflict,
    SupportsReturning, SupportsRowLocking, SupportsUpdateFrom, SupportsWindowFunctions,
};

/// Marker trait representing a PostgreSQL engine version.
///
/// The marker currently gates nothing: no capability trait, associated constant,
/// method, or version-threshold trait consults the parameter it binds, and all
/// three pinned releases — [`Pg14`], [`Pg15`], [`Pg16`] — support every
/// capability this crate implements. The same chain therefore composes under each
/// of them and renders identical SQL.
///
/// It is reserved for the first capability whose engine support genuinely varies
/// between releases: the release that does not provide the capability is what
/// introduces a version threshold, in the shape `SqliteVersionAtLeast` gives the
/// SQLite dialect. Until such a divergence is found the capability impls stay
/// unconditional, because a gate written ahead of a real difference would claim a
/// compile-time check that no accepted statement depends on.
pub trait PgVersion: 'static + Send + Sync {}

/// PostgreSQL 14 marker.
///
/// Supports every capability this crate implements; see [`PgVersion`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Pg14;
impl PgVersion for Pg14 {}

/// PostgreSQL 15 marker.
///
/// Supports every capability this crate implements; see [`PgVersion`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Pg15;
impl PgVersion for Pg15 {}

/// PostgreSQL 16 marker.
///
/// Supports every capability this crate implements, and is the release the
/// dialect targets by default; see [`PgVersion`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Pg16;
impl PgVersion for Pg16 {}

/// PostgreSQL dialect parameterised by engine version (defaulting to [`Pg16`]).
///
/// The parameter records the release a statement targets and changes no rendered
/// SQL today: the capability impls below are written for every `V: PgVersion`, so
/// [`Pg14`], [`Pg15`], and [`Pg16`] accept the same clauses and emit the same
/// text. It narrows that set only once a release is found that does not provide a
/// capability — see [`PgVersion`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Postgres<V: PgVersion = Pg16>(PhantomData<fn() -> V>);

impl<V: PgVersion> Dialect for Postgres<V> {
    // PostgreSQL aggregates a nested row set natively, which keeps the ordering,
    // the filter, and the row limit of the nested statement in charge of the
    // produced document.
    const SUPPORTS_JSONB_ROW_AGG: bool = true;

    // `INSERT INTO "users" AS "u" ...` is an extension of the standard that
    // PostgreSQL parses: the alias makes the inserted row reachable in the
    // `RETURNING` clause and in the assignments of an `ON CONFLICT` clause.
    // SQLite has no such production, so only this dialect states the
    // capability.
    const SUPPORTS_ALIASED_INSERT_TARGET: bool = true;

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

// Every capability below is implemented for every pinned release, which is why
// each impl is written for `V: PgVersion` alone rather than for a single release
// marker: PostgreSQL 14, 15, and 16 all accept the clauses these capabilities
// admit, so a threshold on any of them would reject a statement the engine
// parses. A threshold belongs on the first capability whose support genuinely
// varies between releases; until one is found, the parameter records the target
// and gates nothing.
impl<V: PgVersion> SupportsReturning for Postgres<V> {}
impl<V: PgVersion> SupportsDistinctOn for Postgres<V> {}
impl<V: PgVersion> SupportsOnConflict for Postgres<V> {}
impl<V: PgVersion> SupportsLateralJoin for Postgres<V> {}
// An alias on the target of an `INSERT` is an extension ISO SQL does not
// define; PostgreSQL is the engine of this workspace that parses it.
impl<V: PgVersion> SupportsAliasedInsertTarget for Postgres<V> {}

// The marker admits `InsertQuery::into_aliased_table`, while the builder
// consults `Dialect::SUPPORTS_ALIASED_INSERT_TARGET` when it assembles the
// statement: both describe one capability, so every form the marker admits is
// linked to the constant right beside it. The bare `Postgres` alias resolves to
// `Pg16`, so the three pinned markers cover every instantiable form of the
// dialect.
const _: () = assert!(<Postgres<Pg14> as Dialect>::SUPPORTS_ALIASED_INSERT_TARGET);
const _: () = assert!(<Postgres<Pg15> as Dialect>::SUPPORTS_ALIASED_INSERT_TARGET);
const _: () = assert!(<Postgres<Pg16> as Dialect>::SUPPORTS_ALIASED_INSERT_TARGET);
impl<V: PgVersion> SupportsUpdateFrom for Postgres<V> {}
// `DELETE ... USING` selects the deleted rows through another relation, which
// PostgreSQL implements on every pinned release; SQLite has no such clause.
impl<V: PgVersion> SupportsDeleteUsing for Postgres<V> {}
impl<V: PgVersion> SupportsFullOuterJoin for Postgres<V> {}
impl<V: PgVersion> SupportsRowLocking for Postgres<V> {}
impl<V: PgVersion> SupportsAggregateFilter for Postgres<V> {}
impl<V: PgVersion> SupportsWindowFunctions for Postgres<V> {}
impl<V: PgVersion> SupportsNullsOrdering for Postgres<V> {}
impl<V: PgVersion> SupportsGreatest for Postgres<V> {}
impl<V: PgVersion> SupportsJsonAggregation for Postgres<V> {}
impl<V: PgVersion> SupportsDistinctFrom for Postgres<V> {}

#[cfg(test)]
mod tests {
    use super::*;
    use rurqe_core::dml::InsertQuery;
    use rurqe_core::expr::SqlVal;
    use rurqe_core::schema::{Aliased, AliasedCol};
    use rurqe_core::{table, table_alias};

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
    fn the_default_version_resolves_to_the_newest_marker() {
        // The struct declares `Pg16` as its default type parameter, so a type
        // that names no release resolves to the newest marker. Comparing the
        // bare name against `Postgres<Pg16>` stops compiling the moment the
        // default stops naming it — first of all when a newer release marker
        // is introduced and the default is left behind.
        let newest: Postgres = Postgres::<Pg16>::default();
        let _: Postgres<Pg16> = newest;
    }

    #[test]
    fn capabilities_are_satisfied() {
        fn assert_capabilities<
            D: Dialect
                + SupportsReturning
                + SupportsDistinctOn
                + SupportsOnConflict
                + SupportsLateralJoin
                + SupportsAliasedInsertTarget
                + SupportsDeleteUsing
                + SupportsUpdateFrom
                + SupportsFullOuterJoin
                + SupportsAggregateFilter,
        >() {
        }

        assert_capabilities::<Postgres<Pg14>>();
        assert_capabilities::<Postgres<Pg15>>();
        assert_capabilities::<Postgres<Pg16>>();
        assert_capabilities::<Postgres>();
    }

    #[test]
    fn expression_capabilities_hold_for_every_version() {
        fn assert_expression_capabilities<
            D: Dialect
                + SupportsWindowFunctions
                + SupportsNullsOrdering
                + SupportsGreatest
                + SupportsJsonAggregation
                + SupportsDistinctFrom,
        >() {
        }

        assert_expression_capabilities::<Postgres<Pg14>>();
        assert_expression_capabilities::<Postgres<Pg15>>();
        assert_expression_capabilities::<Postgres<Pg16>>();
        assert_expression_capabilities::<Postgres>();
    }

    #[test]
    fn row_level_locking_is_a_postgres_capability() {
        fn assert_locking<D: Dialect + SupportsRowLocking>() {}

        assert_locking::<Postgres<Pg14>>();
        assert_locking::<Postgres<Pg15>>();
        assert_locking::<Postgres<Pg16>>();
        assert_locking::<Postgres>();
    }

    #[test]
    fn an_aliased_insert_target_is_a_postgres_capability() {
        fn assert_aliased_insert<D: Dialect + SupportsAliasedInsertTarget>() {
            assert!(
                D::SUPPORTS_ALIASED_INSERT_TARGET,
                "a dialect accepting the alias states the capability as a constant too"
            );
        }

        assert_aliased_insert::<Postgres<Pg14>>();
        assert_aliased_insert::<Postgres<Pg15>>();
        assert_aliased_insert::<Postgres<Pg16>>();
        assert_aliased_insert::<Postgres>();
    }

    table!(
        /// The `users` fixture table the aliased-insert test targets.
        User => "users" {
            UserId => "id": i64,
        }
    );

    table_alias! {
        /// The `u` alias of the users fixture table.
        UserAlias => User as "u"
    }

    #[test]
    fn an_aliased_insert_target_renders_its_as_clause_on_postgres() {
        let built = InsertQuery::<Postgres<Pg16>, Aliased<User, UserAlias>>::into_aliased_table()
            .values(AliasedCol::<UserAlias, UserId>::set(1i64))
            .build()
            .expect("valid query");

        assert_eq!(
            built.sql,
            "INSERT INTO \"users\" AS \"u\" (\"id\") VALUES ($1)"
        );
        assert_eq!(built.params, vec![SqlVal::I64(1)]);
    }
}
