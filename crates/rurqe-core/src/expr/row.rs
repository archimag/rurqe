//! Row value expressions: tuples of columns and expressions compared as one value.
//!
//! SQL:1992 treats a parenthesised tuple of expressions as a row value, so a
//! pair of columns is compared element-wise against a pair of values and
//! renders as a native tuple comparison instead of a hand-written combination
//! of scalar predicates. Keyset pagination is the canonical use: an index
//! ordered by `(created_at, id)` is walked by comparing both columns against
//! the cursor as one value.
//!
//! ```text
//! ("created_at", "id") > ($1, $2)
//! ```
//!
//! Both sides stay checked at the call site: every element of the compared
//! tuple is validated against the result type of the row element it is compared
//! with, and the scopes of the two sides merge exactly like the scopes of
//! scalar operands.

mod impls;

#[cfg(test)]
mod tests;

use super::ast::{ExprKind, MergeDialect, MergedDialect, SubqueryOp};
use super::builder::{Expr, NonAggregate};
use super::ops::InSubquery;
use super::scope::{MergeScope, Merged};
use super::val::BinaryOp;

/// A tuple of two to eight columns or expressions compared as one composite value.
///
/// Every comparison operator accepts a tuple of values on the right, so a row
/// reads like the SQL it renders, and the tuple participates in `IN (subquery)`
/// membership predicates. A row of columns merges the scopes of its elements,
/// which keeps the usual scope checking of the enclosing query:
///
/// ```
/// use rurqe_core::dialect::Dialect;
/// use rurqe_core::expr::{render_expr, RowExpr};
///
/// struct MockDialect;
///
/// impl Dialect for MockDialect {
///     fn push_quoted_identifier(out: &mut String, ident: &str) {
///         out.push('"');
///         out.push_str(ident);
///         out.push('"');
///     }
///
///     fn push_placeholder(out: &mut String, index: usize) {
///         out.push('$');
///         out.push_str(&index.to_string());
///     }
/// }
///
/// rurqe_core::table! {
///     /// The `users` table.
///     User => "users" {
///         UserId => "id": i64,
///         UserName => "name": String,
///     }
/// }
///
/// let predicate = (UserId, UserName).gt((10i64, "root"));
///
/// let mut sql = String::new();
/// let mut params = Vec::new();
/// render_expr::<MockDialect>(&mut sql, &mut params, predicate.kind(), &[])
///     .expect("valid expression");
///
/// assert_eq!(sql, "((\"id\", \"name\") > ($1, $2))");
/// assert_eq!(params.len(), 2);
/// ```
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a row value expression",
    label = "a row value is a tuple of 2 to 8 columns or expressions",
    note = "build the row from column markers or expressions, such as `(UserId, UserName)`"
)]
pub trait RowExpr<Row> {
    /// The tables the row references.
    type Scope;

    /// The dialect marker of the row.
    type Dialect;

    /// Appends the element expressions of the row, in declaration order.
    #[doc(hidden)]
    fn push_elements(self, values: &mut Vec<ExprKind>);

    /// Compares the row with `values` using the SQL `=` operator.
    #[allow(clippy::type_complexity)]
    fn eq<Values>(
        self,
        values: Values,
    ) -> Expr<
        bool,
        Merged<Self::Scope, Values::Scope>,
        NonAggregate,
        MergedDialect<Self::Dialect, Values::Dialect>,
    >
    where
        Self: Sized,
        Self::Scope: MergeScope<Values::Scope>,
        Self::Dialect: MergeDialect<Values::Dialect>,
        Values: RowValues<Row>,
    {
        Expr::new(row_comparison::<Row, Self, Values>(
            self,
            BinaryOp::Eq,
            values,
        ))
    }

    /// Compares the row with `values` using the SQL `<>` operator.
    #[allow(clippy::type_complexity)]
    fn ne<Values>(
        self,
        values: Values,
    ) -> Expr<
        bool,
        Merged<Self::Scope, Values::Scope>,
        NonAggregate,
        MergedDialect<Self::Dialect, Values::Dialect>,
    >
    where
        Self: Sized,
        Self::Scope: MergeScope<Values::Scope>,
        Self::Dialect: MergeDialect<Values::Dialect>,
        Values: RowValues<Row>,
    {
        Expr::new(row_comparison::<Row, Self, Values>(
            self,
            BinaryOp::Ne,
            values,
        ))
    }

    /// Compares the row with `values` using the SQL `>` operator.
    #[allow(clippy::type_complexity)]
    fn gt<Values>(
        self,
        values: Values,
    ) -> Expr<
        bool,
        Merged<Self::Scope, Values::Scope>,
        NonAggregate,
        MergedDialect<Self::Dialect, Values::Dialect>,
    >
    where
        Self: Sized,
        Self::Scope: MergeScope<Values::Scope>,
        Self::Dialect: MergeDialect<Values::Dialect>,
        Values: RowValues<Row>,
    {
        Expr::new(row_comparison::<Row, Self, Values>(
            self,
            BinaryOp::Gt,
            values,
        ))
    }

    /// Compares the row with `values` using the SQL `<` operator.
    #[allow(clippy::type_complexity)]
    fn lt<Values>(
        self,
        values: Values,
    ) -> Expr<
        bool,
        Merged<Self::Scope, Values::Scope>,
        NonAggregate,
        MergedDialect<Self::Dialect, Values::Dialect>,
    >
    where
        Self: Sized,
        Self::Scope: MergeScope<Values::Scope>,
        Self::Dialect: MergeDialect<Values::Dialect>,
        Values: RowValues<Row>,
    {
        Expr::new(row_comparison::<Row, Self, Values>(
            self,
            BinaryOp::Lt,
            values,
        ))
    }

    /// Compares the row with `values` using the SQL `>=` operator.
    #[allow(clippy::type_complexity)]
    fn gte<Values>(
        self,
        values: Values,
    ) -> Expr<
        bool,
        Merged<Self::Scope, Values::Scope>,
        NonAggregate,
        MergedDialect<Self::Dialect, Values::Dialect>,
    >
    where
        Self: Sized,
        Self::Scope: MergeScope<Values::Scope>,
        Self::Dialect: MergeDialect<Values::Dialect>,
        Values: RowValues<Row>,
    {
        Expr::new(row_comparison::<Row, Self, Values>(
            self,
            BinaryOp::Gte,
            values,
        ))
    }

    /// Compares the row with `values` using the SQL `<=` operator.
    #[allow(clippy::type_complexity)]
    fn lte<Values>(
        self,
        values: Values,
    ) -> Expr<
        bool,
        Merged<Self::Scope, Values::Scope>,
        NonAggregate,
        MergedDialect<Self::Dialect, Values::Dialect>,
    >
    where
        Self: Sized,
        Self::Scope: MergeScope<Values::Scope>,
        Self::Dialect: MergeDialect<Values::Dialect>,
        Values: RowValues<Row>,
    {
        Expr::new(row_comparison::<Row, Self, Values>(
            self,
            BinaryOp::Lte,
            values,
        ))
    }

    /// Builds an `IN (subquery)` membership predicate matching the row.
    ///
    /// The nested query renders inline, so its parameters continue the placeholder
    /// numbering of the enclosing statement. It must project as many columns as the
    /// row has elements, and every projected column must hold the type of the row
    /// element it is compared with, because the two tuples are compared element by
    /// element. A query of another shape is rejected at the call site:
    ///
    /// ```compile_fail
    /// use rurqe_core::dialect::Dialect;
    /// use rurqe_core::expr::RowExpr;
    /// use rurqe_core::query::SelectQuery;
    ///
    /// struct MockDialect;
    ///
    /// impl Dialect for MockDialect {
    ///     fn push_quoted_identifier(out: &mut String, ident: &str) {
    ///         out.push('"');
    ///         out.push_str(ident);
    ///         out.push('"');
    ///     }
    ///
    ///     fn push_placeholder(out: &mut String, index: usize) {
    ///         out.push('$');
    ///         out.push_str(&index.to_string());
    ///     }
    /// }
    ///
    /// rurqe_core::table! {
    ///     /// The `users` table.
    ///     User => "users" {
    ///         UserId => "id": i64,
    ///         UserName => "name": String,
    ///     }
    /// }
    ///
    /// rurqe_core::table! {
    ///     /// The `orders` table.
    ///     Order => "orders" {
    ///         OrderId => "id": i64,
    ///         OrderName => "name": String,
    ///     }
    /// }
    ///
    /// let pairs = SelectQuery::<MockDialect>::new()
    ///     .from::<Order>()
    ///     .select((OrderName, OrderId));
    ///
    /// // Fails to compile: the row compares an integer with a string in its first
    /// // position, while the nested query projects them the other way round.
    /// let _ = (UserId, UserName).in_subquery(pairs);
    /// ```
    #[allow(clippy::type_complexity)]
    fn in_subquery<S>(
        self,
        subquery: S,
    ) -> Expr<bool, Self::Scope, NonAggregate, MergedDialect<Self::Dialect, S::Dialect>>
    where
        Self: Sized,
        Self::Dialect: MergeDialect<S::Dialect>,
        S: InSubquery<Row>,
    {
        Expr::new(row_membership::<Row, Self, S>(self, subquery, false))
    }

    /// Builds a `NOT IN (subquery)` negated membership predicate matching the row.
    ///
    /// The negation keeps the contract of the positive form: the nested query
    /// still has to project the same tuple as the compared row, as
    /// [`in_subquery`](Self::in_subquery) requires.
    #[allow(clippy::type_complexity)]
    fn not_in_subquery<S>(
        self,
        subquery: S,
    ) -> Expr<bool, Self::Scope, NonAggregate, MergedDialect<Self::Dialect, S::Dialect>>
    where
        Self: Sized,
        Self::Dialect: MergeDialect<S::Dialect>,
        S: InSubquery<Row>,
    {
        Expr::new(row_membership::<Row, Self, S>(self, subquery, true))
    }
}

/// A tuple of values compared element-wise with a [`RowExpr`] of type `Row`.
///
/// Every element is validated against the result type of the row element it is
/// compared with, so a pair of columns only compares with a pair of compatible
/// values. Plain values and built expressions are both accepted; a column
/// marker on the right is spelled as an expression through
/// [`Expression::into_expression`](crate::expr::Expression::into_expression).
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot be compared with a row value of type `{Row}`",
    label = "incompatible row element types",
    note = "every element must be compatible with the type of the row element it is compared with"
)]
pub trait RowValues<Row> {
    /// The tables the values reference.
    type Scope;

    /// The dialect marker of the values.
    type Dialect;

    /// Appends the compared values, in declaration order.
    #[doc(hidden)]
    fn push_values(self, values: &mut Vec<ExprKind>);
}

/// Builds the tuple comparison of `lhs` with `rhs` under the operator `op`.
fn row_comparison<Row, L, R>(lhs: L, op: BinaryOp, rhs: R) -> ExprKind
where
    L: RowExpr<Row>,
    R: RowValues<Row>,
{
    let mut elements = Vec::new();
    lhs.push_elements(&mut elements);

    let mut values = Vec::new();
    rhs.push_values(&mut values);

    ExprKind::Binary {
        op,
        lhs: Box::new(ExprKind::Row { values: elements }),
        rhs: Box::new(ExprKind::Row { values }),
    }
}

/// Builds the row membership predicate of `lhs` against `subquery`.
fn row_membership<Row, L, S>(lhs: L, subquery: S, negated: bool) -> ExprKind
where
    L: RowExpr<Row>,
    S: InSubquery<Row>,
{
    let mut elements = Vec::new();
    lhs.push_elements(&mut elements);

    ExprKind::Subquery {
        op: SubqueryOp::In,
        operand: Some(Box::new(ExprKind::Row { values: elements })),
        query: subquery.into_subquery(),
        negated,
    }
}
