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
