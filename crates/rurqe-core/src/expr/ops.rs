//! Operand compatibility, conversion, and call-site expression traits.

use super::ast::{ExprKind, SubqueryOp};
use super::builder::Expr;
use super::scope::{MergeScope, Merged, Merged3, Unscoped};
use super::val::ToSqlVal;
use crate::dialect::Dialect;
use crate::query::{HasProjection, SelectQuery, TableScope};
use crate::schema::Column;

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

impl<T, Scope, Aggr> SqlCompatible<T, Expr<T, Scope, Aggr>> for Expr<T, Scope, Aggr> {}

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

    /// Converts `self` into an expression AST node.
    fn into_expr_kind(self) -> ExprKind;
}

impl IntoExpr<i32> for i32 {
    type Scope = ();

    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(self.to_sql_val())
    }
}

impl IntoExpr<i64> for i64 {
    type Scope = ();

    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(self.to_sql_val())
    }
}

impl IntoExpr<f64> for f64 {
    type Scope = ();

    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(self.to_sql_val())
    }
}

impl IntoExpr<bool> for bool {
    type Scope = ();

    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(self.to_sql_val())
    }
}

impl IntoExpr<String> for String {
    type Scope = ();

    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(self.to_sql_val())
    }
}

impl IntoExpr<String> for &str {
    type Scope = ();

    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(self.to_sql_val())
    }
}

impl IntoExpr<Vec<u8>> for Vec<u8> {
    type Scope = ();

    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(self.to_sql_val())
    }
}

impl IntoExpr<Vec<u8>> for &[u8] {
    type Scope = ();

    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(self.to_sql_val())
    }
}

impl<T, Scope, Aggr> IntoExpr<T> for Expr<T, Scope, Aggr> {
    type Scope = Scope;

    fn into_expr_kind(self) -> ExprKind {
        self.into_kind()
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

    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Subquery {
            op: SubqueryOp::Scalar,
            operand: None,
            query: self.into_subquery(),
            negated: false,
        }
    }
}

#[cfg(feature = "uuid")]
impl SqlCompatible<uuid::Uuid, &uuid::Uuid> for &uuid::Uuid {}

#[cfg(feature = "uuid")]
impl IntoExpr<uuid::Uuid> for uuid::Uuid {
    type Scope = ();

    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(self.to_sql_val())
    }
}

#[cfg(feature = "uuid")]
impl IntoExpr<uuid::Uuid> for &uuid::Uuid {
    type Scope = ();

    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(self.to_sql_val())
    }
}

#[cfg(feature = "uuid")]
impl<'a> SqlCompatible<uuid::Uuid, &'a str> for &'a str {}

#[cfg(feature = "uuid")]
impl IntoExpr<uuid::Uuid> for &str {
    type Scope = ();

    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(self.to_sql_val())
    }
}

#[cfg(feature = "chrono")]
impl SqlCompatible<chrono::NaiveDateTime, &chrono::NaiveDateTime> for &chrono::NaiveDateTime {}

#[cfg(feature = "chrono")]
impl IntoExpr<chrono::NaiveDateTime> for chrono::NaiveDateTime {
    type Scope = ();

    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(self.to_sql_val())
    }
}

#[cfg(feature = "chrono")]
impl IntoExpr<chrono::NaiveDateTime> for &chrono::NaiveDateTime {
    type Scope = ();

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

    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(self.to_sql_val())
    }
}

#[cfg(feature = "chrono")]
impl IntoExpr<chrono::DateTime<chrono::Utc>> for &chrono::DateTime<chrono::Utc> {
    type Scope = ();

    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(self.to_sql_val())
    }
}

#[cfg(feature = "chrono")]
impl SqlCompatible<chrono::NaiveDate, &chrono::NaiveDate> for &chrono::NaiveDate {}

#[cfg(feature = "chrono")]
impl IntoExpr<chrono::NaiveDate> for chrono::NaiveDate {
    type Scope = ();

    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(self.to_sql_val())
    }
}

#[cfg(feature = "chrono")]
impl IntoExpr<chrono::NaiveDate> for &chrono::NaiveDate {
    type Scope = ();

    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(self.to_sql_val())
    }
}

#[cfg(feature = "serde_json")]
impl SqlCompatible<serde_json::Value, &serde_json::Value> for &serde_json::Value {}

#[cfg(feature = "serde_json")]
impl IntoExpr<serde_json::Value> for serde_json::Value {
    type Scope = ();

    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(self.to_sql_val())
    }
}

#[cfg(feature = "serde_json")]
impl IntoExpr<serde_json::Value> for &serde_json::Value {
    type Scope = ();

    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(self.to_sql_val())
    }
}

#[cfg(feature = "serde_json")]
impl<T: serde::Serialize> IntoExpr<crate::expr::Json<T>> for crate::expr::Json<T> {
    type Scope = ();

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
    note = "arithmetic combines `i32`, `i64`, and `f64` values, which are the numeric types a column maps onto"
)]
pub trait Numeric: ToSqlVal + Sized {}

impl Numeric for i32 {}

impl Numeric for i64 {}

impl Numeric for f64 {}

/// A value an arithmetic expression combines with a numeric expression.
///
/// Arithmetic reads the way the SQL it renders does, so the right-hand side of an
/// operator is either a value of the numeric type, an expression of that type, or
/// a column declared as that type:
///
/// ```
/// use rurqe_core::expr::{Expr, NumericOperand};
///
/// fn assert_operand<T: NumericOperand<f64>>() {}
///
/// assert_operand::<f64>();
/// assert_operand::<Expr<f64>>();
/// ```
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not an operand of a `{T}` arithmetic expression",
    label = "incompatible arithmetic operand",
    note = "arithmetic combines a numeric expression with a value of the same type, an expression of that type, or a column declared as that type"
)]
pub trait NumericOperand<T: Numeric> {
    /// The scope of the tables the operand references.
    type Scope;

    /// Converts `self` into the expression node of the operand.
    fn into_operand_kind(self) -> ExprKind;
}

/// Implements [`NumericOperand`] for the listed scalar types.
///
/// The types are written out instead of being covered by a blanket implementation
/// for every [`Numeric`] type: a blanket one would overlap with the implementations
/// for expressions and column markers, which a downstream crate may combine with a
/// numeric type of its own.
macro_rules! impl_numeric_operand {
    ($($ty:ty),+ $(,)?) => {
        $(
            impl NumericOperand<$ty> for $ty {
                type Scope = ();

                fn into_operand_kind(self) -> ExprKind {
                    ExprKind::Value(self.to_sql_val())
                }
            }
        )+
    };
}

impl_numeric_operand!(i32, i64, f64);

impl<T, Scope, Aggr> NumericOperand<T> for Expr<T, Scope, Aggr>
where
    T: Numeric,
{
    type Scope = Scope;

    fn into_operand_kind(self) -> ExprKind {
        self.into_kind()
    }
}

impl<T, C> NumericOperand<T> for C
where
    T: Numeric,
    C: Column<RustType = T>,
{
    type Scope = C::Table;

    fn into_operand_kind(self) -> ExprKind {
        C::expr_kind()
    }
}

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

    /// Converts `self` into the expression it stands for.
    fn into_expression(self) -> Expr<T, Self::Scope>;

    /// Drops the relation scope from this expression, enabling use in correlated subqueries.
    ///
    /// The dropped tables are replaced with the [`Unscoped`] marker, so the
    /// expression is accepted in every query scope without becoming
    /// indistinguishable from a constant.
    fn without_scope(self) -> Expr<T, Unscoped>
    where
        Self: Sized,
    {
        self.into_expression().without_scope()
    }

    /// Compares the expression with `value` using the SQL `=` operator.
    fn eq<V>(self, value: V) -> Expr<bool, Merged<Self::Scope, V::Scope>>
    where
        Self: Sized,
        Self::Scope: MergeScope<V::Scope>,
        V: IntoExpr<T>,
    {
        self.into_expression().eq(value)
    }

    /// Compares the expression with `value` using the SQL `<>` operator.
    fn ne<V>(self, value: V) -> Expr<bool, Merged<Self::Scope, V::Scope>>
    where
        Self: Sized,
        Self::Scope: MergeScope<V::Scope>,
        V: IntoExpr<T>,
    {
        self.into_expression().ne(value)
    }

    /// Compares the expression with `value` using the SQL `>` operator.
    fn gt<V>(self, value: V) -> Expr<bool, Merged<Self::Scope, V::Scope>>
    where
        Self: Sized,
        Self::Scope: MergeScope<V::Scope>,
        V: IntoExpr<T>,
    {
        self.into_expression().gt(value)
    }

    /// Compares the expression with `value` using the SQL `<` operator.
    fn lt<V>(self, value: V) -> Expr<bool, Merged<Self::Scope, V::Scope>>
    where
        Self: Sized,
        Self::Scope: MergeScope<V::Scope>,
        V: IntoExpr<T>,
    {
        self.into_expression().lt(value)
    }

    /// Builds an `IS NULL` predicate over the expression.
    fn is_null(self) -> Expr<bool, Self::Scope>
    where
        Self: Sized,
    {
        // Call the inherent `Expr` method explicitly: `Expr<T>::is_null` takes
        // `&self`, so a plain method call would resolve back to this by-value
        // trait method and recurse.
        Expr::is_null(&self.into_expression())
    }

    /// Builds an `IS NOT NULL` predicate over the expression.
    fn is_not_null(self) -> Expr<bool, Self::Scope>
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
    ) -> Expr<bool, Merged3<Self::Scope, L::Scope, H::Scope>>
    where
        Self: Sized,
        Self::Scope: MergeScope<L::Scope>,
        Merged<Self::Scope, L::Scope>: MergeScope<H::Scope>,
        L: IntoExpr<T>,
        H: IntoExpr<T>,
    {
        self.into_expression().between(low, high)
    }

    /// Builds an `IN (value, ...)` membership predicate over the expression.
    fn in_list<I, V>(self, values: I) -> Expr<bool, Self::Scope>
    where
        Self: Sized,
        I: IntoIterator<Item = V>,
        V: IntoExpr<T>,
    {
        self.into_expression().in_list(values)
    }

    /// Builds a `NOT IN (value, ...)` negated membership predicate.
    fn not_in_list<I, V>(self, values: I) -> Expr<bool, Self::Scope>
    where
        Self: Sized,
        I: IntoIterator<Item = V>,
        V: IntoExpr<T>,
    {
        self.into_expression().not_in_list(values)
    }
}

impl<T, Scope> Expression<T> for Expr<T, Scope> {
    type Scope = Scope;

    fn into_expression(self) -> Expr<T, Scope> {
        self
    }
}

impl<C: Column> Expression<C::RustType> for C {
    type Scope = C::Table;

    fn into_expression(self) -> Expr<C::RustType, C::Table> {
        Expr::<C::RustType, C::Table>::new(C::expr_kind())
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

    /// Converts `self` into the string expression it stands for.
    fn into_string_expression(self) -> Expr<String, Self::Scope>;

    /// Builds a case-sensitive `LIKE` pattern-matching predicate.
    fn like<V>(self, pattern: V) -> Expr<bool, Merged<Self::Scope, V::Scope>>
    where
        Self: Sized,
        Self::Scope: MergeScope<V::Scope>,
        V: IntoExpr<String>,
    {
        self.into_string_expression().like(pattern)
    }

    /// Builds a case-insensitive `ILIKE` pattern-matching predicate.
    fn ilike<V>(self, pattern: V) -> Expr<bool, Merged<Self::Scope, V::Scope>>
    where
        Self: Sized,
        Self::Scope: MergeScope<V::Scope>,
        V: IntoExpr<String>,
    {
        self.into_string_expression().ilike(pattern)
    }
}

impl<Scope> StringExpression for Expr<String, Scope> {
    type Scope = Scope;

    fn into_string_expression(self) -> Expr<String, Scope> {
        self
    }
}

impl<C: Column<RustType = String>> StringExpression for C {
    type Scope = C::Table;

    fn into_string_expression(self) -> Expr<String, C::Table> {
        Expr::<String, C::Table>::new(C::expr_kind())
    }
}
