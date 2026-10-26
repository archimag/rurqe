//! Nesting contracts for statements embedded in predicates.

use crate::dialect::Dialect;
use crate::query::{HasProjection, LockUnavailable, SelectQuery, SetOpApplied, TableScope};

use crate::expr::ast::Subquery;

/// A statement a predicate of an enclosing statement nests.
///
/// Every nested statement — a scalar operand, an `EXISTS` predicate, and an `IN`
/// predicate — is accepted through this contract, so a query has to project its
/// rows before it can be embedded, and a query locking the rows it selects for its
/// own transaction stays a statement of its own.
///
/// The trait states only that a statement can be nested. Which row a membership
/// predicate accepts is stated by [`InSubquery`] instead, so `EXISTS` reads a
/// statement whatever it projects while `IN` compares it with a row.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a statement a predicate can nest",
    label = "the nested statement is not a projected query",
    note = "call `.select(...)` before nesting the query, so the statement projects the compared columns"
)]
pub trait AnySubquery {
    /// The dialect marker of the nested query.
    type Dialect;

    /// Converts `self` into the nested statement it stands for.
    fn into_subquery(self) -> Subquery;
}

impl AnySubquery for Subquery {
    type Dialect = ();

    fn into_subquery(self) -> Subquery {
        self
    }
}

impl<D, Tables, Output> AnySubquery for SelectQuery<D, Tables, Output, HasProjection>
where
    D: Dialect,
    Tables: TableScope,
{
    type Dialect = D;

    fn into_subquery(self) -> Subquery {
        self.into_subquery()
    }
}

// A statement a set operation combined or a deduplication changed cannot request
// a row-level lock, but both are still plain statements an enclosing one can
// nest, so both participate in the predicates that read a nested query. Only the
// lockable queries excluded by the impl above — the ones carrying `Locked` — stay
// statements of their own.
impl<D, Tables, Output> AnySubquery for SelectQuery<D, Tables, Output, HasProjection, LockUnavailable>
where
    D: Dialect,
    Tables: TableScope,
{
    type Dialect = D;

    fn into_subquery(self) -> Subquery {
        self.into_subquery()
    }
}

impl<D, Tables, Output> AnySubquery for SelectQuery<D, Tables, Output, HasProjection, SetOpApplied>
where
    D: Dialect,
    Tables: TableScope,
{
    type Dialect = D;

    fn into_subquery(self) -> Subquery {
        self.into_subquery()
    }
}

/// A statement nested as the right-hand side of a membership predicate.
///
/// The `Row` parameter is the row the predicate compares, so the nested query has
/// to project exactly that row: a scalar expression of type `T` accepts a query
/// projecting `(T,)`, which is how a single-column query is read as the values of
/// that column, and a row value of columns accepts a query projecting the same
/// tuple in the same order. A query of another arity — or of another element type
/// — has no implementation for the compared row, so the mismatch is rejected at
/// the call site instead of reaching the engine:
///
/// ```compile_fail
/// use rurqe_core::dialect::Dialect;
/// use rurqe_core::expr::InSubquery;
/// use rurqe_core::query::{HasProjection, SelectQuery, TableScope};
///
/// fn accepts_one_value<S: InSubquery<(i64,)>>() {}
///
/// fn rejected<D, Tables>()
/// where
///     D: Dialect,
///     Tables: TableScope,
/// {
///     // Fails to compile: the nested query projects two columns, while a value is
///     // compared with a set of values.
///     accepts_one_value::<SelectQuery<D, Tables, (i64, i64), HasProjection>>();
/// }
/// ```
///
/// The element type takes part in the contract as well, so a query of values of
/// another type is rejected the same way:
///
/// ```compile_fail
/// use rurqe_core::dialect::Dialect;
/// use rurqe_core::expr::InSubquery;
/// use rurqe_core::query::{HasProjection, SelectQuery, TableScope};
///
/// fn accepts_one_value<S: InSubquery<(i64,)>>() {}
///
/// fn rejected<D, Tables>()
/// where
///     D: Dialect,
///     Tables: TableScope,
/// {
///     // Fails to compile: the nested query projects text, while the compared
///     // value is an integer.
///     accepts_one_value::<SelectQuery<D, Tables, (String,), HasProjection>>();
/// }
/// ```
#[diagnostic::on_unimplemented(
    message = "the nested query does not project the row the predicate compares",
    label = "expected a subquery projecting `{Row}`",
    note = "project exactly the compared column, or the tuple of compared columns in the same order, from the nested query"
)]
pub trait InSubquery<Row>: AnySubquery {}

impl<D, Tables, Row> InSubquery<Row> for SelectQuery<D, Tables, Row, HasProjection>
where
    D: Dialect,
    Tables: TableScope,
{
}

// A deduplicated or combined statement compares its rows like any other nested
// query, so both accept the same row contract in a membership predicate.
impl<D, Tables, Row> InSubquery<Row> for SelectQuery<D, Tables, Row, HasProjection, LockUnavailable>
where
    D: Dialect,
    Tables: TableScope,
{
}

impl<D, Tables, Row> InSubquery<Row> for SelectQuery<D, Tables, Row, HasProjection, SetOpApplied>
where
    D: Dialect,
    Tables: TableScope,
{
}
