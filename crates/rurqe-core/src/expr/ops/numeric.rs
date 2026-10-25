//! SQL arithmetic operand contracts and aggregation marker combination.

use crate::expr::ast::ExprKind;
use crate::expr::builder::{Aggregate, DirectAggregate, Expr, FilteredAggregate, NonAggregate};
use crate::expr::scope::Unscoped;
use crate::expr::val::ToSqlVal;
use crate::schema::{Column, Table};

/// A Rust type SQL arithmetic is defined over.
///
/// The trait names the numeric types a column maps onto, so an arithmetic
/// expression is only accepted for a value the database can compute with. Both
/// operands carry the same type: mixing an `i64` with an `f64` has no SQL
/// equivalent that keeps the Rust type of the result, so the mixture is rejected
/// at the call site instead of being widened silently.
///
/// ```
/// use rurqe_core::expr::Numeric;
///
/// fn assert_numeric<T: Numeric>() {}
///
/// assert_numeric::<i32>();
/// assert_numeric::<i64>();
/// assert_numeric::<f64>();
/// ```
#[diagnostic::on_unimplemented(
    message = "`{Self}` does not support SQL arithmetic",
    label = "unsupported arithmetic type",
    note = "arithmetic combines `i32`, `i64`, `f64`, and `Decimal` values, which are the numeric types a column maps onto"
)]
pub trait Numeric: ToSqlVal + Sized {}

impl Numeric for i32 {}

impl Numeric for i64 {}

impl Numeric for f64 {}

#[cfg(feature = "decimal")]
impl Numeric for rust_decimal::Decimal {}

/// A value an arithmetic expression combines with a numeric expression.
///
/// Arithmetic reads the way the SQL it renders does, so the right-hand side of an
/// operator is a value of the numeric type, a column declared as that type, or an
/// expression of that type. Beside the tables the operand references, the
/// operand states the aggregation marker it contributes, which
/// [`CombineAggr`] folds with the marker of the left operand.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not an operand of a `{T}` arithmetic expression",
    label = "incompatible arithmetic operand",
    note = "arithmetic combines a numeric expression with a value of the same type, an expression of that type, or a column declared as that type"
)]
pub trait NumericOperand<T: Numeric> {
    /// The scope of the tables the operand references.
    type Scope;

    /// The aggregation marker the operand contributes to the expression.
    ///
    /// A bound value or a column reference computes one value per row, so the
    /// operand carries [`NonAggregate`]. An operand that is already an
    /// expression keeps the marker it was built with, which keeps
    /// `SUM("total") + SUM("tax")` an aggregate.
    type Aggr;

    /// The dialect marker of the operand.
    type Dialect;

    /// Converts `self` into the expression node of the operand.
    fn into_operand_kind(self) -> ExprKind;
}

/// Implements [`NumericOperand`] for the listed scalar types.
///
/// The types are written out instead of being covered by a blanket implementation
/// over [`Numeric`], because a blanket one would overlap with the implementations
/// for column markers and for expressions.
macro_rules! impl_scalar_numeric_operand {
    ($($ty:ty),+ $(,)?) => {
        $(
            impl NumericOperand<$ty> for $ty {
                type Scope = ();
                type Aggr = NonAggregate;
                type Dialect = ();

                fn into_operand_kind(self) -> ExprKind {
                    ExprKind::Value(self.to_sql_val())
                }
            }
        )+
    };
}

impl_scalar_numeric_operand!(i32, i64, f64);

// An exact decimal is the numeric type of a `NUMERIC` column, so it takes part
// in arithmetic exactly like the built-in scalars do. The borrowed form lets a
// decimal that is reused by the caller be bound without giving up ownership.
#[cfg(feature = "decimal")]
impl NumericOperand<rust_decimal::Decimal> for rust_decimal::Decimal {
    type Scope = ();
    type Aggr = NonAggregate;
    type Dialect = ();

    fn into_operand_kind(self) -> ExprKind {
        ExprKind::Value(self.to_sql_val())
    }
}

#[cfg(feature = "decimal")]
impl NumericOperand<rust_decimal::Decimal> for &rust_decimal::Decimal {
    type Scope = ();
    type Aggr = NonAggregate;
    type Dialect = ();

    fn into_operand_kind(self) -> ExprKind {
        ExprKind::Value(self.to_sql_val())
    }
}

// A column marker computes one value per row of its table.
impl<T, C> NumericOperand<T> for C
where
    T: Numeric,
    C: Column<RustType = T>,
{
    type Scope = C::Table;
    type Aggr = NonAggregate;
    type Dialect = ();

    fn into_operand_kind(self) -> ExprKind {
        C::expr_kind()
    }
}

// A built expression keeps both the tables and the aggregation marker it was
// constructed with.
impl<T, Scope, Marker, D> NumericOperand<T> for Expr<T, Scope, Marker, D>
where
    T: Numeric,
{
    type Scope = Scope;
    type Aggr = Marker;
    type Dialect = D;

    fn into_operand_kind(self) -> ExprKind {
        self.into_kind()
    }
}

/// Derives the aggregation marker of an arithmetic expression.
///
/// The left operand of an arithmetic operator states whether the expression
/// collapses the rows of its group, while the right operand states the same
/// through its [marker](NumericOperand::Aggr) and says through its
/// [scope](NumericOperand::Scope) whether it is constant within a group:
///
/// - two aggregates collapse their group, so the result stays [`Aggregate`];
/// - two per-row operands compute one value per row, so the result is
///   [`NonAggregate`];
/// - an aggregate combined with an operand referencing no table —
///   `SUM("total") + $1` — stays an aggregate, because a constant is the same
///   value in every row of the group;
/// - an aggregate combined with an operand reading a column —
///   `SUM("total") + "tax"` — computes one value per row, so the result is
///   [`NonAggregate`] and a grouped query rejects it.
///
/// The last two cases are what keeps the projection of a grouped query sound: the
/// arithmetic reads a column the query neither groups by nor aggregates, which
/// SQL rejects outside the `GROUP BY` clause.
///
/// A combination is never the aggregate call an operand ends with: the `DISTINCT`
/// and `FILTER (WHERE ...)` clauses attach to a call, so the expression an
/// operator builds over one — a bare call, or a call restricted by a filter that
/// stays recorded in its tree — degrades the specific [`DirectAggregate`] and
/// [`FilteredAggregate`] markers to the general [`Aggregate`] one. The marker a
/// pair resolves to therefore states that the combination collapses the rows of
/// its group while leaving both clauses reachable on the call alone.
#[diagnostic::on_unimplemented(
    message = "the aggregation marker `{Self}` cannot be combined with a `{RhsAggr}` operand of scope `{RhsScope}`",
    label = "unsupported arithmetic operand",
    note = "arithmetic combines a numeric expression with a value, a column reference, or an expression built from them"
)]
pub trait CombineAggr<RhsAggr, RhsScope> {
    /// The aggregation marker of the expression the operands build.
    type Output;
}

/// The aggregation marker of an arithmetic expression combining `LhsAggr` with an
/// operand carrying `RhsAggr` and referencing `RhsScope`.
pub type CombinedAggr<LhsAggr, RhsAggr, RhsScope> =
    <LhsAggr as CombineAggr<RhsAggr, RhsScope>>::Output;

// Two per-row operands compute one value per row, whatever tables they read.
impl<RhsScope> CombineAggr<NonAggregate, RhsScope> for NonAggregate {
    type Output = NonAggregate;
}

// A per-row value combined with an aggregate reads a column of the group, so the
// arithmetic stops collapsing it.
impl<RhsScope> CombineAggr<Aggregate, RhsScope> for NonAggregate {
    type Output = NonAggregate;
}

// A per-row operand on the left reads a column of the group, so a direct or a
// filtered call on the right stops collapsing it as well.
impl<RhsScope> CombineAggr<DirectAggregate, RhsScope> for NonAggregate {
    type Output = NonAggregate;
}

impl<RhsScope> CombineAggr<FilteredAggregate, RhsScope> for NonAggregate {
    type Output = NonAggregate;
}

// A combination is never the aggregate call an operand ends with: the `DISTINCT`
// and `FILTER (WHERE ...)` clauses attach to a call, so an expression built over
// one — a bare call, or a call restricted by a filter that stays recorded in its
// tree — degrades to the general marker whichever side of the operator the call
// sits on. The pairing therefore covers the whole matrix of the aggregate
// markers: the combination keeps collapsing the rows of its group while neither
// clause stays reachable on it.
macro_rules! impl_aggregate_pair_combine {
    ($($aggr:ident),+) => {
        $(
            impl<RhsScope> CombineAggr<Aggregate, RhsScope> for $aggr {
                type Output = Aggregate;
            }

            impl<RhsScope> CombineAggr<DirectAggregate, RhsScope> for $aggr {
                type Output = Aggregate;
            }

            impl<RhsScope> CombineAggr<FilteredAggregate, RhsScope> for $aggr {
                type Output = Aggregate;
            }
        )+
    };
}

impl_aggregate_pair_combine!(Aggregate, DirectAggregate, FilteredAggregate);

// A value referencing no table is the same in every row of a group, so it leaves
// an aggregate standing.
impl CombineAggr<NonAggregate, ()> for Aggregate {
    type Output = Aggregate;
}

// A direct call scaled by a value still collapses its group, and the arithmetic
// expression is no longer the call itself, so it degrades to the general marker
// without losing its place in the projection of a grouped query.
impl CombineAggr<NonAggregate, ()> for DirectAggregate {
    type Output = Aggregate;
}

// A constant scales the whole call, but the combination is no longer the call
// itself, so a filtered aggregate scaled by a value degrades to the general
// marker as well: it keeps collapsing its group and stays projectable from a
// grouped query, while the filter recorded in its tree belongs to the call
// instead of the combination that consumed it.
impl CombineAggr<NonAggregate, ()> for FilteredAggregate {
    type Output = Aggregate;
}

// A dropped scope names a column read that is checked against the enclosing
// statement instead of the query, so it is no more constant than the column.
impl CombineAggr<NonAggregate, Unscoped> for Aggregate {
    type Output = NonAggregate;
}

impl CombineAggr<NonAggregate, Unscoped> for DirectAggregate {
    type Output = NonAggregate;
}

impl CombineAggr<NonAggregate, Unscoped> for FilteredAggregate {
    type Output = NonAggregate;
}

// A table scope is the scope of a column reference, which computes one value per
// row.
impl<T: Table> CombineAggr<NonAggregate, T> for Aggregate {
    type Output = NonAggregate;
}

impl<T: Table> CombineAggr<NonAggregate, T> for DirectAggregate {
    type Output = NonAggregate;
}

impl<T: Table> CombineAggr<NonAggregate, T> for FilteredAggregate {
    type Output = NonAggregate;
}

// A compound operand references the tables of every column it reads.
macro_rules! impl_tuple_combine_aggr {
    ($($table:ident),+) => {
        impl<$($table: Table),+> CombineAggr<NonAggregate, ($($table,)+)> for Aggregate {
            type Output = NonAggregate;
        }

        impl<$($table: Table),+> CombineAggr<NonAggregate, ($($table,)+)> for FilteredAggregate {
            type Output = NonAggregate;
        }

        impl<$($table: Table),+> CombineAggr<NonAggregate, ($($table,)+)> for DirectAggregate {
            type Output = NonAggregate;
        }
    };
}

impl_tuple_combine_aggr!(T1);
impl_tuple_combine_aggr!(T1, T2);
impl_tuple_combine_aggr!(T1, T2, T3);
impl_tuple_combine_aggr!(T1, T2, T3, T4);
impl_tuple_combine_aggr!(T1, T2, T3, T4, T5);
impl_tuple_combine_aggr!(T1, T2, T3, T4, T5, T6);
impl_tuple_combine_aggr!(T1, T2, T3, T4, T5, T6, T7);
impl_tuple_combine_aggr!(T1, T2, T3, T4, T5, T6, T7, T8);
