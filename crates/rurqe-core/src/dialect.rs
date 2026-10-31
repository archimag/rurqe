//! Multi-dialect abstractions, identifier escaping, and compile-time capability markers.

/// A SQL dialect specifying identifier quoting and parameter placeholder formatting.
pub trait Dialect: 'static + Send + Sync {
    /// Whether the dialect renders `ILIKE` directly.
    ///
    /// `ILIKE` is a PostgreSQL extension: SQLite only knows the case-sensitive
    /// `LIKE`. A dialect without it keeps the same case-insensitive semantics by
    /// folding both operands with `LOWER` when the predicate is rendered, so an
    /// `ilike` call never renders SQL the engine cannot parse. `LOWER` folds
    /// ASCII letters only, which keeps the fallback independent of the database
    /// collation.
    const SUPPORTS_ILIKE: bool = true;

    /// Whether the dialect aggregates a nested row set into native JSON.
    ///
    /// A dialect with native row aggregation renders a nested collection as
    /// `jsonb_agg(to_jsonb(...))`, which keeps the clauses of the nested
    /// statement in charge of the produced document. A dialect without it
    /// assembles every object field by field with `json_object` and collects the
    /// objects with `json_group_array`. Both forms produce the same JSON
    /// document, so a client decodes one representation whichever engine
    const SUPPORTS_JSONB_ROW_AGG: bool = false;

    /// Whether the dialect accepts an alias on the target of an `INSERT`.
    ///
    /// An aliased target renders `INSERT INTO "users" AS "u" (...)`, which makes
    /// the columns of the inserted row reachable under the alias in a
    /// `RETURNING` clause and in the assignments of an `ON CONFLICT` clause. ISO
    /// SQL defines no alias production for the `INSERT` target, so whether the
    /// statement is accepted depends on the engine rather than on the standard:
    /// PostgreSQL parses it, while SQLite has no such production at all.
    ///
    /// The alias follows from the table marker the statement targets, which is a
    /// value-level property the phantom-marker typestate cannot observe, so the
    /// builders read this constant while they assemble the statement and reject
    /// the alias before rendering a single character of it.
    /// [`SupportsAliasedInsertTarget`] states the same capability as a bound,
    /// which keeps the aliased constructor of an `INSERT` unreachable at compile
    /// time.
    const SUPPORTS_ALIASED_INSERT_TARGET: bool = false;

    /// Appends a quoted identifier to the output SQL buffer.
    fn push_quoted_identifier(out: &mut String, ident: &str);

    /// Appends a parameter placeholder to the output SQL buffer for a 1-based parameter index.
    fn push_placeholder(out: &mut String, index: usize);

    /// Returns a newly allocated quoted identifier string.
    fn quote_identifier(ident: &str) -> String {
        let mut out = String::with_capacity(ident.len() + 2);
        Self::push_quoted_identifier(&mut out, ident);
        out
    }

    /// Returns a newly allocated parameter placeholder string for `index`.
    fn placeholder(index: usize) -> String {
        let mut out = String::new();
        Self::push_placeholder(&mut out, index);
        out
    }
}

/// Marker trait for dialects supporting the `RETURNING` clause on DML statements.
#[diagnostic::on_unimplemented(
    message = "dialect `{Self}` does not support the RETURNING clause",
    label = "RETURNING is not supported by this dialect",
    note = "the RETURNING clause requires a dialect that implements `SupportsReturning` (such as PostgreSQL or SQLite 3.35+)"
)]
pub trait SupportsReturning: Dialect {}

/// Marker trait for dialects supporting `DISTINCT ON (...)` expressions in queries.
#[diagnostic::on_unimplemented(
    message = "dialect `{Self}` does not support DISTINCT ON",
    label = "DISTINCT ON is not supported by this dialect",
    note = "DISTINCT ON is a PostgreSQL-specific clause requiring `SupportsDistinctOn`"
)]
pub trait SupportsDistinctOn: Dialect {}

/// Marker trait for dialects supporting `ON CONFLICT` upsert syntax.
#[diagnostic::on_unimplemented(
    message = "dialect `{Self}` does not support ON CONFLICT upsert syntax",
    label = "ON CONFLICT is not supported by this dialect",
    note = "upsert clauses require a dialect that implements `SupportsOnConflict` (such as PostgreSQL or SQLite 3.24+)"
)]
pub trait SupportsOnConflict: Dialect {}

/// Marker trait for dialects supporting `LATERAL` joins.
///
/// A `LATERAL` subquery may reference columns of the tables to its left in the
/// `FROM` clause, which lets the query express correlated per-row subqueries
/// (such as "the latest order of every user") directly in the join.
#[diagnostic::on_unimplemented(
    message = "dialect `{Self}` does not support LATERAL joins",
    label = "LATERAL joins are not supported by this dialect",
    note = "LATERAL joins require a dialect that implements `SupportsLateralJoin` (such as PostgreSQL)"
)]
pub trait SupportsLateralJoin: Dialect {}

/// Marker trait for dialects supporting the `USING` clause of `DELETE` statements.
///
/// `DELETE FROM t USING u` selects the deleted rows through another relation,
/// which PostgreSQL and MySQL implement while SQLite has no such clause at all.
#[diagnostic::on_unimplemented(
    message = "dialect `{Self}` does not support the `USING` clause of DELETE",
    label = "`DELETE ... USING` is not supported by this dialect",
    note = "deleting rows through another relation requires a dialect that implements `SupportsDeleteUsing` (such as PostgreSQL)"
)]
pub trait SupportsDeleteUsing: Dialect {}

/// Marker trait for dialects supporting the `FROM` clause of `UPDATE` statements.
///
/// `UPDATE t SET ... FROM u` reads the assigned values from another relation,
/// which PostgreSQL implements and SQLite gained in 3.33.0.
#[diagnostic::on_unimplemented(
    message = "dialect `{Self}` does not support the `FROM` clause of UPDATE",
    label = "`UPDATE ... FROM` is not supported by this dialect",
    note = "updating rows from another relation requires a dialect that implements `SupportsUpdateFrom` (such as PostgreSQL or SQLite 3.35+)"
)]
pub trait SupportsUpdateFrom: Dialect {}

/// Marker trait for dialects accepting an alias on the target of an `INSERT`.
///
/// `INSERT INTO "users" AS "u" ...` names the inserted relation under an alias,
/// which makes its columns reachable through that alias in the `RETURNING`
/// clause and in the assignments of an `ON CONFLICT` clause. ISO SQL defines no
/// alias production for the `INSERT` target, so the clause is an extension of
/// the standard: PostgreSQL parses the statement, while SQLite has no such
/// production and rejects it. A dialect declaring the capability also sets
/// [`Dialect::SUPPORTS_ALIASED_INSERT_TARGET`] to `true`, which is what the
/// builder reads when it assembles a statement from a target it cannot inspect
/// at compile time.
#[diagnostic::on_unimplemented(
    message = "dialect `{Self}` does not support an alias on the INSERT target",
    label = "an aliased INSERT target is not supported by this dialect",
    note = "an aliased INSERT target requires a dialect that implements `SupportsAliasedInsertTarget` (such as PostgreSQL)"
)]
pub trait SupportsAliasedInsertTarget: Dialect {}

/// Marker trait for dialects supporting `FULL OUTER JOIN`.
///
/// A full outer join keeps the unmatched rows of both sides, nulling the columns
/// of the side that found no match, so a row of either relation may reach the
/// projection without its counterpart. PostgreSQL implements the join, while
/// SQLite gained it only in 3.39.0, past the baseline version its dialect
/// declares, and therefore has no such capability.
#[diagnostic::on_unimplemented(
    message = "dialect `{Self}` does not support FULL OUTER JOIN",
    label = "`FULL OUTER JOIN` is not supported by this dialect",
    note = "a full outer join requires a dialect that implements `SupportsFullOuterJoin` (such as PostgreSQL)"
)]
pub trait SupportsFullOuterJoin: Dialect {}

/// Marker trait for dialects supporting row-level locking clauses.
///
/// A locking clause makes the statement lock the rows it selects until the
/// enclosing transaction ends, which is how a worker claims a batch of pending
/// jobs (`FOR UPDATE SKIP LOCKED`) without contending with its peers. PostgreSQL
/// implements the clause, while SQLite serialises access by locking the whole
/// database and therefore has no per-row equivalent.
#[diagnostic::on_unimplemented(
    message = "dialect `{Self}` does not support row-level locking",
    label = "`FOR UPDATE` and `FOR SHARE` are not supported by this dialect",
    note = "locking selected rows requires a dialect that implements `SupportsRowLocking` (such as PostgreSQL)"
)]
pub trait SupportsRowLocking: Dialect {}

/// Marker trait for dialects supporting aggregate `FILTER (WHERE ...)` clauses.
#[diagnostic::on_unimplemented(
    message = "dialect `{Self}` does not support aggregate FILTER (WHERE ...)",
    label = "`FILTER (WHERE ...)` is not supported by this dialect",
    note = "aggregate filters require a dialect that implements `SupportsAggregateFilter` (such as PostgreSQL or SQLite 3.30+)"
)]
pub trait SupportsAggregateFilter: Dialect {}

/// Marker trait for dialects supporting window functions (`OVER (...)`).
///
/// A window expression evaluates its function over a partition of the result
/// rows while keeping every row of the projection, which is how rankings,
/// running totals, and neighbouring-row lookups are expressed. PostgreSQL has
/// implemented the clause since 8.4, while SQLite gained it in 3.25.0, so a
/// dialect only exposes the builders when it declares this capability.
#[diagnostic::on_unimplemented(
    message = "dialect `{Self}` does not support window functions",
    label = "`OVER (...)` is not supported by this dialect",
    note = "window functions require a dialect that implements `SupportsWindowFunctions` (such as PostgreSQL or SQLite 3.25+)"
)]
pub trait SupportsWindowFunctions: Dialect {}

/// Marker trait for dialects supporting explicit `NULLS FIRST` and `NULLS LAST`.
///
/// An ordering that places nulls explicitly keeps the result stable across
/// engines whose default null positioning differs. PostgreSQL and SQLite 3.30+
/// render the modifier, while an older engine only knows its implicit placement
/// and must not be handed the keyword.
#[diagnostic::on_unimplemented(
    message = "dialect `{Self}` does not support explicit NULLS FIRST and NULLS LAST",
    label = "`NULLS FIRST` and `NULLS LAST` are not supported by this dialect",
    note = "explicit null positioning requires a dialect that implements `SupportsNullsOrdering` (such as PostgreSQL or SQLite 3.30+)"
)]
pub trait SupportsNullsOrdering: Dialect {}

/// Marker trait for dialects supporting the `GREATEST` and `LEAST` functions.
///
/// The two functions return the largest or smallest operand of their argument
/// list, which they extend to any number of values. PostgreSQL implements them
/// natively, while SQLite never gained them, so a dialect only exposes the
/// helpers when it declares this capability.
#[diagnostic::on_unimplemented(
    message = "dialect `{Self}` does not support the GREATEST and LEAST functions",
    label = "`GREATEST` and `LEAST` are not supported by this dialect",
    note = "the greatest and least helpers require a dialect that implements `SupportsGreatest` (such as PostgreSQL)"
)]
pub trait SupportsGreatest: Dialect {}

/// Marker trait for dialects supporting JSON aggregation of a nested row set.
///
/// A nested collection renders as `jsonb_agg(to_jsonb(...))` on an engine with
/// native row aggregation and as `json_group_array` over `json_object` fields on
/// an engine without it. Both forms produce the same JSON document, but a
/// dialect must declare which one it can render before the builders emit it.
#[diagnostic::on_unimplemented(
    message = "dialect `{Self}` does not support JSON aggregation of a row set",
    label = "JSON array and object aggregation is not supported by this dialect",
    note = "nested collections and objects require a dialect that implements `SupportsJsonAggregation` (such as PostgreSQL or SQLite 3.38+)"
)]
pub trait SupportsJsonAggregation: Dialect {}

/// Marker trait for dialects supporting `IS DISTINCT FROM` and `IS NOT DISTINCT FROM`.
///
/// The predicates compare two values by treating null as an ordinary operand, so
/// they never yield an unknown result. PostgreSQL implements them natively, while
/// SQLite only knows its own `IS` and `IS NOT` operators for the same purpose.
#[diagnostic::on_unimplemented(
    message = "dialect `{Self}` does not support IS DISTINCT FROM and IS NOT DISTINCT FROM",
    label = "`IS DISTINCT FROM` and `IS NOT DISTINCT FROM` are not supported by this dialect",
    note = "null-safe comparison requires a dialect that implements `SupportsDistinctFrom` (such as PostgreSQL)"
)]
pub trait SupportsDistinctFrom: Dialect {}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockDialect;

    impl Dialect for MockDialect {
        fn push_quoted_identifier(out: &mut String, ident: &str) {
            out.push('`');
            out.push_str(ident);
            out.push('`');
        }

        fn push_placeholder(out: &mut String, _index: usize) {
            out.push('?');
        }
    }

    #[test]
    fn helper_methods_delegate_to_push_methods() {
        assert_eq!(MockDialect::quote_identifier("users"), "`users`");
        assert_eq!(MockDialect::placeholder(1), "?");
    }
}
