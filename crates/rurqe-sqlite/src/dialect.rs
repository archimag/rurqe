//! The SQLite dialect and its type-level engine versions.

use core::marker::PhantomData;

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
impl SqliteVersionAtLeast<3, 33> for Sqlite3_35 {}
impl SqliteVersionAtLeast<3, 35> for Sqlite3_35 {}

/// SQLite 3.38 marker: the first release that always bundles the JSON functions.
///
/// The JSON1 extension (`json_object`, `json_group_array`, and the rest of the
/// family) was an opt-in compile-time option (`SQLITE_ENABLE_JSON1`) until
/// 3.38.0, so a default build of an earlier release rejects those functions.
/// A nested collection renders with them, so the capability that admits it is
/// gated at this release rather than at the upsert threshold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Sqlite3_38;
impl SqliteVersion for Sqlite3_38 {}
impl SqliteVersionAtLeast<3, 0> for Sqlite3_38 {}
impl SqliteVersionAtLeast<3, 24> for Sqlite3_38 {}
impl SqliteVersionAtLeast<3, 25> for Sqlite3_38 {}
impl SqliteVersionAtLeast<3, 30> for Sqlite3_38 {}
impl SqliteVersionAtLeast<3, 33> for Sqlite3_38 {}
impl SqliteVersionAtLeast<3, 35> for Sqlite3_38 {}
impl SqliteVersionAtLeast<3, 38> for Sqlite3_38 {}

/// SQLite dialect parameterised by engine version (defaulting to [`Sqlite3_35`]).
///
/// Identifiers are double-quoted and escaped by doubling embedded quotes, while
/// bind parameters use anonymous `?` placeholders.
///
/// SQLite has no `ILIKE` operator, so the dialect keeps
/// [`Dialect::SUPPORTS_ILIKE`] disabled and a case-insensitive predicate renders
/// as `LOWER(...) LIKE LOWER(...)` instead of the unsupported keyword.
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
/// SQLite gained `UPDATE ... FROM` in 3.33.0, so the capability is gated on that
/// introduction threshold and a version older than it does not implement
/// [`SupportsUpdateFrom`]:
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
///
/// The JSON functions a nested collection renders with were an opt-in extension
/// until 3.38.0, so no version before [`Sqlite3_38`] implements
/// [`SupportsJsonAggregation`]:
///
/// ```compile_fail
/// use rurqe_core::dialect::SupportsJsonAggregation;
/// use rurqe_sqlite::{Sqlite, Sqlite3_35};
///
/// fn assert_json_aggregation<D: SupportsJsonAggregation>() {}
/// assert_json_aggregation::<Sqlite<Sqlite3_35>>();
/// ```
///
/// SQLite has no alias production on the target of an `INSERT`, so the
/// `SupportsAliasedInsertTarget` capability is never implemented:
///
/// ```compile_fail
/// use rurqe_core::dialect::SupportsAliasedInsertTarget;
/// use rurqe_sqlite::{Sqlite, Sqlite3_35};
///
/// fn assert_aliased_insert<D: SupportsAliasedInsertTarget>() {}
/// assert_aliased_insert::<Sqlite<Sqlite3_35>>();
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Sqlite<V: SqliteVersion = Sqlite3_35>(PhantomData<fn() -> V>);

impl<V: SqliteVersion> Dialect for Sqlite<V> {
    // SQLite knows only the case-sensitive `LIKE` operator, so a
    // case-insensitive predicate folds both operands with `LOWER` instead of
    // rendering `ILIKE`, which the engine cannot parse.
    const SUPPORTS_ILIKE: bool = false;

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

// `UPDATE ... FROM` belongs to SQLite since 3.33.0, so the gate names that release
// as the introduction threshold rather than the newest pinned marker. No marker
// between 3.30 and 3.35 is pinned, so the reachable capability set is unchanged,
// while a future `Sqlite3_33` marker would satisfy the gate without editing this
// impl. SQLite has no `DELETE ... USING` clause at all, so `SupportsDeleteUsing`
// stays unimplemented for every version.
impl<V: SqliteVersion + SqliteVersionAtLeast<3, 33>> SupportsUpdateFrom for Sqlite<V> {}

impl<V: SqliteVersion + SqliteVersionAtLeast<3, 30>> SupportsAggregateFilter for Sqlite<V> {}

// Window functions belong to SQLite since 3.25.0, and explicit `NULLS FIRST` /
// `NULLS LAST` since 3.30.0, so each capability follows the threshold that
// introduced it. `GREATEST` and `LEAST` were never added to the engine (its
// `max`/`min` scalar functions serve the same purpose), and `IS DISTINCT FROM`
// arrived only in 3.39.0, past the newest pinned marker, so neither capability is
// implemented for any declared version.
impl<V: SqliteVersion + SqliteVersionAtLeast<3, 25>> SupportsWindowFunctions for Sqlite<V> {}

impl<V: SqliteVersion + SqliteVersionAtLeast<3, 30>> SupportsNullsOrdering for Sqlite<V> {}

// The JSON1 extension — `json_object`, `json_group_array`, and the rest of the
// family — was an opt-in compile-time option until 3.38.0 made it part of the
// default build, so a nested collection is admitted only for a version that
// guarantees the functions exist. Gating it at the upsert threshold would let a
// statement compile that a default build rejects with "no such function".
impl<V: SqliteVersion + SqliteVersionAtLeast<3, 38>> SupportsJsonAggregation for Sqlite<V> {}

#[cfg(test)]
mod tests {
    use super::*;
    use rurqe_core::expr::{render_expr, Expr, SqlVal};
    use rurqe_core::table;

    table!(
        /// The `users` fixture table used by the rendering regression test.
        User => "users" {
            UserName => "name": String,
        }
    );

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
    fn update_from_is_gated_on_its_introduction_threshold() {
        fn assert_threshold<V: SqliteVersionAtLeast<3, 33>>() {}
        fn assert_update_from<D: Dialect + SupportsUpdateFrom>() {}

        // `UPDATE ... FROM` shipped in SQLite 3.33.0, so the capability is gated on
        // that release instead of on the newest pinned marker: every marker from
        // there on satisfies the threshold, and a future `Sqlite3_33` marker would
        // satisfy it without editing the capability impl.
        assert_threshold::<Sqlite3_35>();
        assert_threshold::<Sqlite3_38>();

        // No marker between 3.30 and 3.35 is pinned, so the reachable capability set
        // is unchanged: exactly the markers from 3.35 on expose the clause, and
        // `Sqlite<Sqlite3_24>` still fails the struct's compile-fail example.
        assert_update_from::<Sqlite<Sqlite3_35>>();
        assert_update_from::<Sqlite<Sqlite3_38>>();
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

        // The JSON functions only became part of the default build in 3.38.0, so
        // the capability starts at that marker and no earlier version has it.
        assert_json_aggregation::<Sqlite<Sqlite3_38>>();
    }

    #[test]
    fn ilike_folds_both_operands_instead_of_rendering_the_keyword() {
        let predicate = Expr::column::<UserName>().ilike("A%");
        let mut sql = String::new();
        let mut params = Vec::new();

        render_expr::<Sqlite<Sqlite3_35>>(&mut sql, &mut params, predicate.kind(), &[])
            .expect("valid expression");

        // SQLite only knows the case-sensitive `LIKE`, so the predicate keeps its
        // case-insensitive semantics by folding both operands.
        assert_eq!(sql, "(LOWER(\"name\") LIKE LOWER(?))");
        assert!(!sql.contains("ILIKE"));
        assert_eq!(params, vec![SqlVal::Text("A%".to_owned())]);
    }

    #[test]
    fn engine_flags_are_never_permissive_by_default() {
        fn assert_flags<D: Dialect>() {
            assert!(
                !D::SUPPORTS_ILIKE,
                "SQLite has no ILIKE operator and must fold both operands"
            );
            assert!(
                !D::SUPPORTS_JSONB_ROW_AGG,
                "SQLite has no native JSON row aggregation"
            );
            assert!(
                !D::SUPPORTS_ALIASED_INSERT_TARGET,
                "SQLite has no alias production on the INSERT target"
            );
        }

        assert_flags::<Sqlite<Sqlite3_0>>();
        assert_flags::<Sqlite<Sqlite3_24>>();
        assert_flags::<Sqlite<Sqlite3_25>>();
        assert_flags::<Sqlite<Sqlite3_30>>();
        assert_flags::<Sqlite<Sqlite3_35>>();
        assert_flags::<Sqlite<Sqlite3_38>>();
        assert_flags::<Sqlite>();
    }
}
