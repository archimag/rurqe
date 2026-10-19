//! Operand compatibility, conversion, and call-site expression traits.

use super::ast::ExprKind;
use super::builder::Expr;
use super::val::SqlVal;
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

impl<T> SqlCompatible<T, Expr<T>> for Expr<T> {}

/// A value or expression that can be compared with an expression of type `T`.
#[diagnostic::on_unimplemented(
    message = "value of type `{Self}` cannot be compared with a column of type `{T}`",
    label = "incompatible operand types",
    note = "convert the value to the column's `Column::RustType` before comparing"
)]
pub trait IntoExpr<T>: SqlCompatible<T, Self> + Sized {
    /// Converts `self` into an expression AST node.
    fn into_expr_kind(self) -> ExprKind;
}

impl IntoExpr<i32> for i32 {
    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(SqlVal::I32(self))
    }
}

impl IntoExpr<i64> for i64 {
    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(SqlVal::I64(self))
    }
}

impl IntoExpr<f64> for f64 {
    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(SqlVal::F64(self))
    }
}

impl IntoExpr<bool> for bool {
    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(SqlVal::Bool(self))
    }
}

impl IntoExpr<String> for String {
    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(SqlVal::Text(self))
    }
}

impl IntoExpr<String> for &str {
    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(SqlVal::Text(self.to_owned()))
    }
}

impl IntoExpr<Vec<u8>> for Vec<u8> {
    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(SqlVal::Bytes(self))
    }
}

impl IntoExpr<Vec<u8>> for &[u8] {
    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(SqlVal::Bytes(self.to_vec()))
    }
}

impl<T> IntoExpr<T> for Expr<T> {
    fn into_expr_kind(self) -> ExprKind {
        self.into_kind()
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
    /// Converts `self` into the expression it stands for.
    fn into_expression(self) -> Expr<T>;

    /// Compares the expression with `value` using the SQL `=` operator.
    fn eq<V>(self, value: V) -> Expr<bool>
    where
        Self: Sized,
        V: IntoExpr<T>,
    {
        self.into_expression().eq(value)
    }

    /// Compares the expression with `value` using the SQL `<>` operator.
    fn ne<V>(self, value: V) -> Expr<bool>
    where
        Self: Sized,
        V: IntoExpr<T>,
    {
        self.into_expression().ne(value)
    }

    /// Compares the expression with `value` using the SQL `>` operator.
    fn gt<V>(self, value: V) -> Expr<bool>
    where
        Self: Sized,
        V: IntoExpr<T>,
    {
        self.into_expression().gt(value)
    }

    /// Compares the expression with `value` using the SQL `<` operator.
    fn lt<V>(self, value: V) -> Expr<bool>
    where
        Self: Sized,
        V: IntoExpr<T>,
    {
        self.into_expression().lt(value)
    }

    /// Builds an `IS NULL` predicate over the expression.
    fn is_null(self) -> Expr<bool>
    where
        Self: Sized,
    {
        // Call the inherent `Expr` method explicitly: `Expr<T>::is_null` takes
        // `&self`, so a plain method call would resolve back to this by-value
        // trait method and recurse.
        Expr::is_null(&self.into_expression())
    }

    /// Builds an `IS NOT NULL` predicate over the expression.
    fn is_not_null(self) -> Expr<bool>
    where
        Self: Sized,
    {
        // See `Expression::is_null` for why the inherent method is called explicitly.
        Expr::is_not_null(&self.into_expression())
    }

    /// Builds a `BETWEEN low AND high` range predicate over the expression.
    ///
    /// Both bounds are inclusive and must be compatible with the expression type.
    fn between<L, H>(self, low: L, high: H) -> Expr<bool>
    where
        Self: Sized,
        L: IntoExpr<T>,
        H: IntoExpr<T>,
    {
        self.into_expression().between(low, high)
    }

    /// Builds an `IN (value, ...)` membership predicate over the expression.
    fn in_list<I, V>(self, values: I) -> Expr<bool>
    where
        Self: Sized,
        I: IntoIterator<Item = V>,
        V: IntoExpr<T>,
    {
        self.into_expression().in_list(values)
    }

    /// Builds a `NOT IN (value, ...)` negated membership predicate.
    fn not_in_list<I, V>(self, values: I) -> Expr<bool>
    where
        Self: Sized,
        I: IntoIterator<Item = V>,
        V: IntoExpr<T>,
    {
        self.into_expression().not_in_list(values)
    }
}

impl<T> Expression<T> for Expr<T> {
    fn into_expression(self) -> Expr<T> {
        self
    }
}

impl<C: Column> Expression<C::RustType> for C {
    fn into_expression(self) -> Expr<C::RustType> {
        Expr::<C::RustType>::column::<C>()
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
    /// Converts `self` into the string expression it stands for.
    fn into_string_expression(self) -> Expr<String>;

    /// Builds a case-sensitive `LIKE` pattern-matching predicate.
    fn like<V>(self, pattern: V) -> Expr<bool>
    where
        Self: Sized,
        V: IntoExpr<String>,
    {
        self.into_string_expression().like(pattern)
    }

    /// Builds a case-insensitive `ILIKE` pattern-matching predicate.
    fn ilike<V>(self, pattern: V) -> Expr<bool>
    where
        Self: Sized,
        V: IntoExpr<String>,
    {
        self.into_string_expression().ilike(pattern)
    }
}

impl StringExpression for Expr<String> {
    fn into_string_expression(self) -> Expr<String> {
        self
    }
}

impl<C: Column<RustType = String>> StringExpression for C {
    fn into_string_expression(self) -> Expr<String> {
        Expr::<String>::column::<C>()
    }
}
