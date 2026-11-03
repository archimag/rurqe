//! Conversion of bound values and statements into operand expression nodes.

use crate::dialect::Dialect;
use crate::query::{HasProjection, SelectQuery, TableScope, Unlocked};

use crate::expr::ast::{ExprKind, SubqueryOp};
use crate::expr::builder::{Expr, NonAggregate};
use crate::expr::val::ToSqlVal;
use crate::schema::Column;

/// A value or expression that can be compared with an expression of type `T`.
///
/// Operand values are encoded through [`ToSqlVal`], so the parameter
/// representation of a comparison is produced by the same contract that binds
/// standalone query parameters: every literal takes the exact route
/// [`SqlVal`](crate::expr::SqlVal) construction takes, and an existing [`Expr`]
/// keeps its own tree instead.
///
/// A column marker is an operand of the same standing: `C: Column` converts into
/// the node [`Expr::column`] builds, so `OrderUserId.eq(UserId)` compares two
/// columns without an explicit wrapper, and the scope of the conversion is the
/// table of the column. Correlation stays an explicit operation, because the
/// marker always claims its table: dropping the scope goes through
/// [`Expr::without_scope`].
///
/// The trait is the single gate of operand-type compatibility, so a value of a
/// type with no conversion is rejected here with the diagnostic below, at the
/// call site of the comparison.
///
/// A column marker is an operand of the comparison as it stands, and the tree it
/// produces is the one its explicit expression produces:
///
/// ```
/// use rurqe_core::expr::{Expr, Expression};
///
/// rurqe_core::table!(
///     /// The `users` table.
///     User => "users" {
///         UserId => "id": i64,
///     }
/// );
///
/// rurqe_core::table!(
///     /// The `orders` table.
///     Order => "orders" {
///         OrderUserId => "user_id": i64,
///     }
/// );
///
/// assert_eq!(
///     OrderUserId.eq(UserId),
///     Expr::column::<OrderUserId>().eq(Expr::column::<UserId>()),
/// );
/// ```
///
/// A marker of an unrelated type converts into no operand of the compared type,
/// so the comparison is rejected at its own call site:
///
/// ```compile_fail
/// use rurqe_core::expr::Expression;
///
/// rurqe_core::table!(
///     /// The `users` table.
///     User => "users" {
///         UserId => "id": i64,
///         UserName => "name": String,
///     }
/// );
///
/// // Fails to compile: the compared column holds an `i64`, while the marker
/// // carries a `String`, so the marker implements no `IntoExpr<i64>`.
/// let _ = UserId.eq(UserName);
/// ```
#[diagnostic::on_unimplemented(
    message = "value of type `{Self}` cannot be compared with a column of type `{T}`",
    label = "incompatible operand types",
    note = "convert the value to the column's `Column::RustType` before comparing"
)]
pub trait IntoExpr<T>: Sized {
    /// The scope of the tables the operand references.
    ///
    /// Bound values reference no table and keep the empty scope `()`, while a
    /// built expression keeps the scope of its columns.
    type Scope;

    /// The aggregation marker the operand contributes to a predicate.
    ///
    /// A bound value and a scalar subquery compute one value per row of the
    /// enclosing statement, so their implementations state [`NonAggregate`],
    /// which leaves the aggregation marker of the expression they are compared
    /// with standing. A built expression states the marker it was constructed
    /// with, so a comparison folds the markers of both operands the same way
    /// arithmetic does.
    type Aggr;

    /// The dialect marker of the operand.
    type Dialect;

    /// Converts `self` into an expression AST node.
    fn into_expr_kind(self) -> ExprKind;
}

impl IntoExpr<i32> for i32 {
    type Scope = ();
    type Aggr = NonAggregate;
    type Dialect = ();

    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(self.to_sql_val())
    }
}

impl IntoExpr<i64> for i64 {
    type Scope = ();
    type Aggr = NonAggregate;
    type Dialect = ();

    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(self.to_sql_val())
    }
}

impl IntoExpr<f64> for f64 {
    type Scope = ();
    type Aggr = NonAggregate;
    type Dialect = ();

    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(self.to_sql_val())
    }
}

#[cfg(feature = "decimal")]
impl IntoExpr<rust_decimal::Decimal> for rust_decimal::Decimal {
    type Scope = ();
    type Aggr = NonAggregate;
    type Dialect = ();

    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(self.to_sql_val())
    }
}

#[cfg(feature = "decimal")]
impl IntoExpr<rust_decimal::Decimal> for &rust_decimal::Decimal {
    type Scope = ();
    type Aggr = NonAggregate;
    type Dialect = ();

    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(self.to_sql_val())
    }
}

impl IntoExpr<bool> for bool {
    type Scope = ();
    type Aggr = NonAggregate;
    type Dialect = ();

    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(self.to_sql_val())
    }
}

impl IntoExpr<String> for String {
    type Scope = ();
    type Aggr = NonAggregate;
    type Dialect = ();

    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(self.to_sql_val())
    }
}

impl IntoExpr<String> for &str {
    type Scope = ();
    type Aggr = NonAggregate;
    type Dialect = ();

    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(self.to_sql_val())
    }
}

impl IntoExpr<Vec<u8>> for Vec<u8> {
    type Scope = ();
    type Aggr = NonAggregate;
    type Dialect = ();

    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(self.to_sql_val())
    }
}

impl IntoExpr<Vec<u8>> for &[u8] {
    type Scope = ();
    type Aggr = NonAggregate;
    type Dialect = ();

    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(self.to_sql_val())
    }
}

impl<T, Scope, Aggr, D> IntoExpr<T> for Expr<T, Scope, Aggr, D> {
    type Scope = Scope;
    type Aggr = Aggr;
    type Dialect = D;

    fn into_expr_kind(self) -> ExprKind {
        self.into_kind()
    }
}

// A column marker is an operand of its own: it converts into the very node
// `Expr::column::<C>()` builds, so `OrderUserId.eq(UserId)` compares two columns
// without an explicit wrapper. The marker carries the table of its column as the
// scope — which is what makes a comparison against a column of a relation the
// query never introduces fail at the call site — and contributes the per-row
// `NonAggregate` marker, exactly like the expression it stands for. Columns of
// an aliased table are covered as well, because `AliasedCol<A, C>` is a column.
impl<C: Column> IntoExpr<C::RustType> for C {
    type Scope = C::Table;
    type Aggr = NonAggregate;
    type Dialect = ();

    fn into_expr_kind(self) -> ExprKind {
        C::expr_kind()
    }
}
impl<D, Tables, T, Lock> IntoExpr<T> for SelectQuery<D, Tables, (T,), HasProjection, Lock>
where
    D: Dialect,
    Tables: TableScope,
    Lock: Unlocked,
{
    type Scope = ();
    type Aggr = NonAggregate;
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

#[cfg(feature = "uuid")]
impl IntoExpr<uuid::Uuid> for uuid::Uuid {
    type Scope = ();
    type Aggr = NonAggregate;
    type Dialect = ();

    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(self.to_sql_val())
    }
}

#[cfg(feature = "uuid")]
impl IntoExpr<uuid::Uuid> for &uuid::Uuid {
    type Scope = ();
    type Aggr = NonAggregate;
    type Dialect = ();

    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(self.to_sql_val())
    }
}

#[cfg(feature = "uuid")]
impl IntoExpr<uuid::Uuid> for &str {
    type Scope = ();
    type Aggr = NonAggregate;
    type Dialect = ();

    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(self.to_sql_val())
    }
}

#[cfg(feature = "chrono")]
impl IntoExpr<chrono::NaiveDateTime> for chrono::NaiveDateTime {
    type Scope = ();
    type Aggr = NonAggregate;
    type Dialect = ();

    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(self.to_sql_val())
    }
}

#[cfg(feature = "chrono")]
impl IntoExpr<chrono::NaiveDateTime> for &chrono::NaiveDateTime {
    type Scope = ();
    type Aggr = NonAggregate;
    type Dialect = ();

    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(self.to_sql_val())
    }
}

#[cfg(feature = "chrono")]
impl IntoExpr<chrono::DateTime<chrono::Utc>> for chrono::DateTime<chrono::Utc> {
    type Scope = ();
    type Aggr = NonAggregate;
    type Dialect = ();

    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(self.to_sql_val())
    }
}

#[cfg(feature = "chrono")]
impl IntoExpr<chrono::DateTime<chrono::Utc>> for &chrono::DateTime<chrono::Utc> {
    type Scope = ();
    type Aggr = NonAggregate;
    type Dialect = ();

    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(self.to_sql_val())
    }
}

#[cfg(feature = "chrono")]
impl IntoExpr<chrono::NaiveDate> for chrono::NaiveDate {
    type Scope = ();
    type Aggr = NonAggregate;
    type Dialect = ();

    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(self.to_sql_val())
    }
}

#[cfg(feature = "chrono")]
impl IntoExpr<chrono::NaiveDate> for &chrono::NaiveDate {
    type Scope = ();
    type Aggr = NonAggregate;
    type Dialect = ();

    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(self.to_sql_val())
    }
}

#[cfg(feature = "serde_json")]
impl IntoExpr<serde_json::Value> for serde_json::Value {
    type Scope = ();
    type Aggr = NonAggregate;
    type Dialect = ();

    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(self.to_sql_val())
    }
}

#[cfg(feature = "serde_json")]
impl IntoExpr<serde_json::Value> for &serde_json::Value {
    type Scope = ();
    type Aggr = NonAggregate;
    type Dialect = ();

    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(self.to_sql_val())
    }
}

#[cfg(feature = "serde_json")]
impl<T: serde::Serialize> IntoExpr<crate::expr::Json<T>> for crate::expr::Json<T> {
    type Scope = ();
    type Aggr = NonAggregate;
    type Dialect = ();

    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(self.to_sql_val())
    }
}

#[cfg(feature = "serde_json")]
impl<T: serde::Serialize> IntoExpr<crate::expr::Json<T>> for &crate::expr::Json<T> {
    type Scope = ();
    type Aggr = NonAggregate;
    type Dialect = ();

    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(self.to_sql_val())
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
/// rurqe_core::table!(
///     /// The `users` table.
///     User => "users" {
///         UserId => "id": i64,
///     }
/// );
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

    /// The aggregation marker the candidates contribute to a predicate.
    ///
    /// A list of bound values references no table and computes one value per
    /// row, so its implementation states [`NonAggregate`]; a list holding built
    /// expressions keeps the marker of its candidates, so a membership test
    /// folds the markers of the tested expression and its candidates together.
    type Aggr;

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
    type Aggr = V::Aggr;
    type Dialect = V::Dialect;

    fn push_expr_kinds(self, values: &mut Vec<ExprKind>) {
        for value in self {
            values.push(value.into_expr_kind());
        }
    }
}
