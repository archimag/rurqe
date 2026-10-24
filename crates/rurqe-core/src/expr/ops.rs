//! Operand compatibility, conversion, and call-site expression traits.
//!
//! The traits here answer the questions an operand raises at the call site of a
//! comparison or an arithmetic operator: [`SqlCompatible`] asserts that the Rust
//! types line up, [`IntoExpr`] converts a bound value into an expression node,
//! [`Expression`] lets a column marker be written directly, and
//! [`NumericOperand`] admits the right-hand side of arithmetic. [`CombineAggr`]
//! then derives whether the arithmetic still collapses the rows of a group.

use super::ast::{ExprKind, MergeDialect, MergedDialect, Subquery, SubqueryOp};
use super::builder::{Aggregate, Expr, FilteredAggregate, NonAggregate};
use super::scope::{MergeScope, Merged, Merged3, Unscoped};
use super::val::ToSqlVal;
use crate::dialect::Dialect;
use crate::query::{HasProjection, LockUnavailable, SelectQuery, TableScope};
use crate::schema::{Column, Table};

/// Statically asserts that a value of type `ValType` may be compared with a
/// column whose Rust type is `ColType`.
///
/// The trait is satisfied when both types match exactly, and additionally for
/// string literals compared with string columns.
#[diagnostic::on_unimplemented(
    message = "value of type `{ValType}` cannot be compared with a column of type `{ColType}`",
    label = "incompatible operand types",
    note = "convert the value to the column's `Column::RustType` before comparing"
)]
pub trait SqlCompatible<ColType, ValType> {}

impl<T> SqlCompatible<T, T> for T {}

impl<'a> SqlCompatible<String, &'a str> for &'a str {}

impl<'a> SqlCompatible<Vec<u8>, &'a [u8]> for &'a [u8] {}

impl<T, Scope, Aggr, D> SqlCompatible<T, Expr<T, Scope, Aggr, D>> for Expr<T, Scope, Aggr, D> {}

/// A value or expression that can be compared with an expression of type `T`.
///
/// Operand values are encoded through [`ToSqlVal`], so the parameter
/// representation of a comparison is produced by the same contract that binds
/// standalone query parameters: every literal takes the exact route
/// [`SqlVal`](crate::expr::SqlVal) construction takes, and an existing [`Expr`]
/// keeps its own tree instead.
#[diagnostic::on_unimplemented(
    message = "value of type `{Self}` cannot be compared with a column of type `{T}`",
    label = "incompatible operand types",
    note = "convert the value to the column's `Column::RustType` before comparing"
)]
pub trait IntoExpr<T>: SqlCompatible<T, Self> + Sized {
    /// The scope of the tables the operand references.
    ///
    /// Bound values reference no table and keep the empty scope `()`, while a
    /// built expression keeps the scope of its columns.
    type Scope;

    /// The dialect marker of the operand.
    type Dialect;

    /// Converts `self` into an expression AST node.
    fn into_expr_kind(self) -> ExprKind;
}

impl IntoExpr<i32> for i32 {
    type Scope = ();
    type Dialect = ();

    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(self.to_sql_val())
    }
}

impl IntoExpr<i64> for i64 {
    type Scope = ();
    type Dialect = ();

    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(self.to_sql_val())
    }
}

impl IntoExpr<f64> for f64 {
    type Scope = ();
    type Dialect = ();

    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(self.to_sql_val())
    }
}

#[cfg(feature = "decimal")]
impl IntoExpr<rust_decimal::Decimal> for rust_decimal::Decimal {
    type Scope = ();
    type Dialect = ();

    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(self.to_sql_val())
    }
}

#[cfg(feature = "decimal")]
impl SqlCompatible<rust_decimal::Decimal, &rust_decimal::Decimal> for &rust_decimal::Decimal {}

#[cfg(feature = "decimal")]
impl IntoExpr<rust_decimal::Decimal> for &rust_decimal::Decimal {
    type Scope = ();
    type Dialect = ();

    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(self.to_sql_val())
    }
}

impl IntoExpr<bool> for bool {
    type Scope = ();
    type Dialect = ();

    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(self.to_sql_val())
    }
}

impl IntoExpr<String> for String {
    type Scope = ();
    type Dialect = ();

    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(self.to_sql_val())
    }
}

impl IntoExpr<String> for &str {
    type Scope = ();
    type Dialect = ();

    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(self.to_sql_val())
    }
}

impl IntoExpr<Vec<u8>> for Vec<u8> {
    type Scope = ();
    type Dialect = ();

    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(self.to_sql_val())
    }
}

impl IntoExpr<Vec<u8>> for &[u8] {
    type Scope = ();
    type Dialect = ();

    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(self.to_sql_val())
    }
}

impl<T, Scope, Aggr, D> IntoExpr<T> for Expr<T, Scope, Aggr, D> {
    type Scope = Scope;
    type Dialect = D;

    fn into_expr_kind(self) -> ExprKind {
        self.into_kind()
    }
}

/// A collection of candidate values of a membership predicate.
///
/// A list is any iterator of values compatible with the tested expression type,
/// and its candidates carry the tables they reference, so a membership predicate
/// joins their scope with its own. A list of bound values references no table,
/// which leaves the scope of the predicate exactly the scope of the tested
/// expression:
///
/// ```
/// use rurqe_core::expr::{ExprKind, Expression};
///
/// rurqe_core::table! {
///     /// The `users` table.
///     User => "users" {
///         UserId => "id": i64,
///     }
/// }
///
/// let predicate = UserId.in_list(vec![1i64, 2i64]);
///
/// assert!(matches!(
///     predicate.kind(),
///     ExprKind::InList { negated: false, .. }
/// ));
/// ```
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a list of values comparable with a column of type `{T}`",
    label = "incompatible membership candidates",
    note = "pass an iterator of values of the tested column's `Column::RustType`, or of expressions of that type"
)]
pub trait IntoExprList<T> {
    /// The scope of the tables the candidates reference.
    type Scope;

    /// The dialect marker of the candidates.
    type Dialect;

    /// Appends the candidate expressions to `values`.
    #[doc(hidden)]
    fn push_expr_kinds(self, values: &mut Vec<ExprKind>);
}

impl<I, V, T> IntoExprList<T> for I
where
    I: IntoIterator<Item = V>,
    V: IntoExpr<T>,
{
    type Scope = V::Scope;
    type Dialect = V::Dialect;

    fn push_expr_kinds(self, values: &mut Vec<ExprKind>) {
        for value in self {
            values.push(value.into_expr_kind());
        }
    }
}

// A single-column query projects exactly one value, so it is comparable with a
// column of that value's type. Queries projecting several columns have no
// `(T,)` output and therefore no such implementation, which is what rejects
// them as operands at the call site.
impl<D, Tables, T> SqlCompatible<T, SelectQuery<D, Tables, (T,), HasProjection>>
    for SelectQuery<D, Tables, (T,), HasProjection>
where
    D: Dialect,
    Tables: TableScope,
{
}

impl<D, Tables, T> IntoExpr<T> for SelectQuery<D, Tables, (T,), HasProjection>
where
    D: Dialect,
    Tables: TableScope,
{
    type Scope = ();
    type Dialect = D;

    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Subquery {
            op: SubqueryOp::Scalar,
            operand: None,
            query: self.into_subquery(),
            negated: false,
        }
    }
}

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

// A deduplicated statement cannot request a row-level lock, but it is still a
// plain statement an enclosing one can nest, so it participates in the
// predicates that read a nested query. Only the lockable queries excluded by
// the impl above — the ones carrying `Locked` — stay statements of their own.
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

// A deduplicated statement compares its rows like any other nested query, so it
// accepts the same row contract in a membership predicate.
impl<D, Tables, Row> InSubquery<Row> for SelectQuery<D, Tables, Row, HasProjection, LockUnavailable>
where
    D: Dialect,
    Tables: TableScope,
{
}

#[cfg(feature = "uuid")]
impl SqlCompatible<uuid::Uuid, &uuid::Uuid> for &uuid::Uuid {}

#[cfg(feature = "uuid")]
impl IntoExpr<uuid::Uuid> for uuid::Uuid {
    type Scope = ();
    type Dialect = ();

    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(self.to_sql_val())
    }
}

#[cfg(feature = "uuid")]
impl IntoExpr<uuid::Uuid> for &uuid::Uuid {
    type Scope = ();
    type Dialect = ();

    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(self.to_sql_val())
    }
}

#[cfg(feature = "uuid")]
impl<'a> SqlCompatible<uuid::Uuid, &'a str> for &'a str {}

#[cfg(feature = "uuid")]
impl IntoExpr<uuid::Uuid> for &str {
    type Scope = ();
    type Dialect = ();

    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(self.to_sql_val())
    }
}

#[cfg(feature = "chrono")]
impl SqlCompatible<chrono::NaiveDateTime, &chrono::NaiveDateTime> for &chrono::NaiveDateTime {}

#[cfg(feature = "chrono")]
impl IntoExpr<chrono::NaiveDateTime> for chrono::NaiveDateTime {
    type Scope = ();
    type Dialect = ();

    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(self.to_sql_val())
    }
}

#[cfg(feature = "chrono")]
impl IntoExpr<chrono::NaiveDateTime> for &chrono::NaiveDateTime {
    type Scope = ();
    type Dialect = ();

    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(self.to_sql_val())
    }
}

#[cfg(feature = "chrono")]
impl SqlCompatible<chrono::DateTime<chrono::Utc>, &chrono::DateTime<chrono::Utc>>
    for &chrono::DateTime<chrono::Utc>
{
}

#[cfg(feature = "chrono")]
impl IntoExpr<chrono::DateTime<chrono::Utc>> for chrono::DateTime<chrono::Utc> {
    type Scope = ();
    type Dialect = ();

    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(self.to_sql_val())
    }
}

#[cfg(feature = "chrono")]
impl IntoExpr<chrono::DateTime<chrono::Utc>> for &chrono::DateTime<chrono::Utc> {
    type Scope = ();
    type Dialect = ();

    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(self.to_sql_val())
    }
}

#[cfg(feature = "chrono")]
impl SqlCompatible<chrono::NaiveDate, &chrono::NaiveDate> for &chrono::NaiveDate {}

#[cfg(feature = "chrono")]
impl IntoExpr<chrono::NaiveDate> for chrono::NaiveDate {
    type Scope = ();
    type Dialect = ();

    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(self.to_sql_val())
    }
}

#[cfg(feature = "chrono")]
impl IntoExpr<chrono::NaiveDate> for &chrono::NaiveDate {
    type Scope = ();
    type Dialect = ();

    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(self.to_sql_val())
    }
}

#[cfg(feature = "serde_json")]
impl SqlCompatible<serde_json::Value, &serde_json::Value> for &serde_json::Value {}

#[cfg(feature = "serde_json")]
impl IntoExpr<serde_json::Value> for serde_json::Value {
    type Scope = ();
    type Dialect = ();

    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(self.to_sql_val())
    }
}

#[cfg(feature = "serde_json")]
impl IntoExpr<serde_json::Value> for &serde_json::Value {
    type Scope = ();
    type Dialect = ();

    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(self.to_sql_val())
    }
}

#[cfg(feature = "serde_json")]
impl<T: serde::Serialize> IntoExpr<crate::expr::Json<T>> for crate::expr::Json<T> {
    type Scope = ();
    type Dialect = ();

    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(self.to_sql_val())
    }
}

#[cfg(feature = "serde_json")]
impl<'a, T: serde::Serialize> SqlCompatible<crate::expr::Json<T>, &'a crate::expr::Json<T>>
    for &'a crate::expr::Json<T>
{
}

#[cfg(feature = "serde_json")]
impl<T: serde::Serialize> IntoExpr<crate::expr::Json<T>> for &crate::expr::Json<T> {
    type Scope = ();
    type Dialect = ();

    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(self.to_sql_val())
    }
}

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

// Two aggregates collapse their group.
impl<RhsScope> CombineAggr<Aggregate, RhsScope> for Aggregate {
    type Output = Aggregate;
}

// A filtered aggregate collapses its group like a plain one, so combining it
// with another aggregate stays an aggregate while keeping the filter recorded.
impl<RhsScope> CombineAggr<Aggregate, RhsScope> for FilteredAggregate {
    type Output = FilteredAggregate;
}

impl<RhsScope> CombineAggr<FilteredAggregate, RhsScope> for Aggregate {
    type Output = FilteredAggregate;
}

impl<RhsScope> CombineAggr<FilteredAggregate, RhsScope> for FilteredAggregate {
    type Output = FilteredAggregate;
}

// A filtered aggregate on the left combined with a per-row operand computes one
// value per row, exactly like a plain aggregate does.
impl<RhsScope> CombineAggr<FilteredAggregate, RhsScope> for NonAggregate {
    type Output = NonAggregate;
}

// A value referencing no table is the same in every row of a group, so it leaves
// an aggregate standing.
impl CombineAggr<NonAggregate, ()> for Aggregate {
    type Output = Aggregate;
}

// The same constant keeps a filtered aggregate standing, so a filtered aggregate
// scaled by a value stays projectable from a grouped query.
impl CombineAggr<NonAggregate, ()> for FilteredAggregate {
    type Output = FilteredAggregate;
}

// A dropped scope names a column read that is checked against the enclosing
// statement instead of the query, so it is no more constant than the column.
impl CombineAggr<NonAggregate, Unscoped> for Aggregate {
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

/// A SQL expression of type `T` that can be compared with bound values.
///
/// The trait lets the expression be written directly at the call site — a column
/// marker, an alias-qualified column, or an already built [`Expr`] — without
/// spelling `Expr::column::<C>()` first:
///
/// ```
/// use rurqe_core::expr::{Expr, Expression};
///
/// rurqe_core::table! {
///     /// The `users` table.
///     User => "users" {
///         UserId => "id": i64,
///     }
/// }
///
/// let predicate = UserId.gt(10i64);
///
/// assert_eq!(predicate, Expr::column::<UserId>().gt(10i64));
/// ```
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not an expression of type `{T}`",
    label = "neither a column marker nor a built expression",
    note = "compare a column marker or an `Expr<T>` produced by the expression builders"
)]
#[allow(clippy::wrong_self_convention)]
pub trait Expression<T> {
    /// The scope of the tables the expression references.
    type Scope;

    /// The dialect marker of the expression.
    type Dialect;

    /// Converts `self` into the expression it stands for.
    fn into_expression(self) -> Expr<T, Self::Scope, NonAggregate, Self::Dialect>;

    /// Drops the relation scope from this expression, enabling use in correlated subqueries.
    ///
    /// The dropped tables are replaced with the [`Unscoped`] marker, so the
    /// expression is accepted in every query scope without becoming
    /// indistinguishable from a constant.
    fn without_scope(self) -> Expr<T, Unscoped, NonAggregate, Self::Dialect>
    where
        Self: Sized,
    {
        self.into_expression().without_scope()
    }

    /// Compares the expression with `value` using the SQL `=` operator.
    #[allow(clippy::type_complexity)]
    fn eq<V>(
        self,
        value: V,
    ) -> Expr<
        bool,
        Merged<Self::Scope, V::Scope>,
        NonAggregate,
        MergedDialect<Self::Dialect, V::Dialect>,
    >
    where
        Self: Sized,
        Self::Scope: MergeScope<V::Scope>,
        Self::Dialect: MergeDialect<V::Dialect>,
        V: IntoExpr<T>,
    {
        self.into_expression().eq(value)
    }

    /// Compares the expression with `value` using the SQL `<>` operator.
    #[allow(clippy::type_complexity)]
    fn ne<V>(
        self,
        value: V,
    ) -> Expr<
        bool,
        Merged<Self::Scope, V::Scope>,
        NonAggregate,
        MergedDialect<Self::Dialect, V::Dialect>,
    >
    where
        Self: Sized,
        Self::Scope: MergeScope<V::Scope>,
        Self::Dialect: MergeDialect<V::Dialect>,
        V: IntoExpr<T>,
    {
        self.into_expression().ne(value)
    }

    /// Compares the expression with `value` using the SQL `>` operator.
    #[allow(clippy::type_complexity)]
    fn gt<V>(
        self,
        value: V,
    ) -> Expr<
        bool,
        Merged<Self::Scope, V::Scope>,
        NonAggregate,
        MergedDialect<Self::Dialect, V::Dialect>,
    >
    where
        Self: Sized,
        Self::Scope: MergeScope<V::Scope>,
        Self::Dialect: MergeDialect<V::Dialect>,
        V: IntoExpr<T>,
    {
        self.into_expression().gt(value)
    }

    /// Compares the expression with `value` using the SQL `<` operator.
    #[allow(clippy::type_complexity)]
    fn lt<V>(
        self,
        value: V,
    ) -> Expr<
        bool,
        Merged<Self::Scope, V::Scope>,
        NonAggregate,
        MergedDialect<Self::Dialect, V::Dialect>,
    >
    where
        Self: Sized,
        Self::Scope: MergeScope<V::Scope>,
        Self::Dialect: MergeDialect<V::Dialect>,
        V: IntoExpr<T>,
    {
        self.into_expression().lt(value)
    }

    /// Builds an `IS NULL` predicate over the expression.
    fn is_null(self) -> Expr<bool, Self::Scope, NonAggregate, Self::Dialect>
    where
        Self: Sized,
    {
        // Call the inherent `Expr` method explicitly: `Expr<T>::is_null` takes
        // `&self`, so a plain method call would resolve back to this by-value
        // trait method and recurse.
        Expr::is_null(&self.into_expression())
    }

    /// Builds an `IS NOT NULL` predicate over the expression.
    fn is_not_null(self) -> Expr<bool, Self::Scope, NonAggregate, Self::Dialect>
    where
        Self: Sized,
    {
        // See `Expression::is_null` for why the inherent method is called explicitly.
        Expr::is_not_null(&self.into_expression())
    }

    /// Builds a `BETWEEN low AND high` range predicate over the expression.
    ///
    /// Both bounds are inclusive and must be compatible with the expression type.
    #[allow(clippy::type_complexity)]
    fn between<L, H>(
        self,
        low: L,
        high: H,
    ) -> Expr<
        bool,
        Merged3<Self::Scope, L::Scope, H::Scope>,
        NonAggregate,
        MergedDialect<MergedDialect<Self::Dialect, L::Dialect>, H::Dialect>,
    >
    where
        Self: Sized,
        Self::Scope: MergeScope<L::Scope>,
        Merged<Self::Scope, L::Scope>: MergeScope<H::Scope>,
        Self::Dialect: MergeDialect<L::Dialect>,
        MergedDialect<Self::Dialect, L::Dialect>: MergeDialect<H::Dialect>,
        L: IntoExpr<T>,
        H: IntoExpr<T>,
    {
        self.into_expression().between(low, high)
    }

    /// Builds an `IN (value, ...)` membership predicate over the expression.
    ///
    /// The tables the candidates reference join the scope of the predicate, so a
    /// candidate reading a column of a table the query does not select from is
    /// rejected at the call site.
    #[allow(clippy::type_complexity)]
    fn in_list<I>(
        self,
        values: I,
    ) -> Expr<
        bool,
        Merged<Self::Scope, I::Scope>,
        NonAggregate,
        MergedDialect<Self::Dialect, I::Dialect>,
    >
    where
        Self: Sized,
        Self::Scope: MergeScope<I::Scope>,
        Self::Dialect: MergeDialect<I::Dialect>,
        I: IntoExprList<T>,
    {
        self.into_expression().in_list(values)
    }

    /// Builds a `NOT IN (value, ...)` negated membership predicate.
    ///
    /// The negation keeps the scope of the positive form: every candidate value
    /// still has to be comparable with the expression.
    #[allow(clippy::type_complexity)]
    fn not_in_list<I>(
        self,
        values: I,
    ) -> Expr<
        bool,
        Merged<Self::Scope, I::Scope>,
        NonAggregate,
        MergedDialect<Self::Dialect, I::Dialect>,
    >
    where
        Self: Sized,
        Self::Scope: MergeScope<I::Scope>,
        Self::Dialect: MergeDialect<I::Dialect>,
        I: IntoExprList<T>,
    {
        self.into_expression().not_in_list(values)
    }
}

impl<T, Scope, D> Expression<T> for Expr<T, Scope, NonAggregate, D> {
    type Scope = Scope;
    type Dialect = D;

    fn into_expression(self) -> Expr<T, Scope, NonAggregate, D> {
        self
    }
}

impl<C: Column> Expression<C::RustType> for C {
    type Scope = C::Table;
    type Dialect = ();

    fn into_expression(self) -> Expr<C::RustType, C::Table, NonAggregate, ()> {
        Expr::<C::RustType, C::Table, NonAggregate, ()>::new(C::expr_kind())
    }
}

/// A string expression that can be matched against `LIKE` patterns.
///
/// Both a string column marker and an already built [`Expr<String>`] implement
/// the trait, so pattern predicates can be written directly at the call site.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a string expression",
    label = "only string columns and `Expr<String>` values accept LIKE patterns",
    note = "match a column declared as `String` or an `Expr<String>` built from one"
)]
pub trait StringExpression {
    /// The scope of the tables the expression references.
    type Scope;

    /// The dialect marker of the expression.
    type Dialect;

    /// Converts `self` into the string expression it stands for.
    fn into_string_expression(self) -> Expr<String, Self::Scope, NonAggregate, Self::Dialect>;

    /// Builds a case-sensitive `LIKE` pattern-matching predicate.
    #[allow(clippy::type_complexity)]
    fn like<V>(
        self,
        pattern: V,
    ) -> Expr<
        bool,
        Merged<Self::Scope, V::Scope>,
        NonAggregate,
        MergedDialect<Self::Dialect, V::Dialect>,
    >
    where
        Self: Sized,
        Self::Scope: MergeScope<V::Scope>,
        Self::Dialect: MergeDialect<V::Dialect>,
        V: IntoExpr<String>,
    {
        self.into_string_expression().like(pattern)
    }

    /// Builds a case-insensitive `ILIKE` pattern-matching predicate.
    #[allow(clippy::type_complexity)]
    fn ilike<V>(
        self,
        pattern: V,
    ) -> Expr<
        bool,
        Merged<Self::Scope, V::Scope>,
        NonAggregate,
        MergedDialect<Self::Dialect, V::Dialect>,
    >
    where
        Self: Sized,
        Self::Scope: MergeScope<V::Scope>,
        Self::Dialect: MergeDialect<V::Dialect>,
        V: IntoExpr<String>,
    {
        self.into_string_expression().ilike(pattern)
    }
}

impl<Scope, D> StringExpression for Expr<String, Scope, NonAggregate, D> {
    type Scope = Scope;
    type Dialect = D;

    fn into_string_expression(self) -> Expr<String, Scope, NonAggregate, D> {
        self
    }
}

impl<C: Column<RustType = String>> StringExpression for C {
    type Scope = C::Table;
    type Dialect = ();

    fn into_string_expression(self) -> Expr<String, C::Table, NonAggregate, ()> {
        Expr::<String, C::Table, NonAggregate, ()>::new(C::expr_kind())
    }
}
