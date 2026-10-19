//! The generic [`Expr`] type and its fluent construction and comparison methods.

use core::marker::PhantomData;

use super::ast::{CaseBuilder, ExprKind, Subquery, SubqueryOp};
use super::ops::IntoExpr;
use super::render::{parse_raw_sql, split_validated, RawSqlError};
use super::val::{AggregateFunc, BinaryOp, SqlVal};
use crate::schema::{Column, Table};

/// A SQL expression whose result has the Rust type `T` and which references the
/// tables named by `Scope`.
///
/// Column references carry the table of their column, so a projection built from
/// a column of another table is rejected at the call site. Constructed values
/// (literals, raw SQL, aggregates, `CASE` branches) reference no table and keep
/// the default scope `()`.
///
/// ```
/// use rurqe_core::expr::{BinaryOp, Expr, ExprKind, SqlVal};
///
/// rurqe_core::table! {
///     /// The `users` table.
///     User => "users" {
///         UserId => "id": i64,
///     }
/// }
///
/// let predicate = Expr::column::<UserId>().gt(10i64);
///
/// assert_eq!(
///     predicate.kind(),
///     &ExprKind::Binary {
///         op: BinaryOp::Gt,
///         lhs: Box::new(ExprKind::Column { name: "id" }),
///         rhs: Box::new(ExprKind::Value(SqlVal::I64(10))),
///     }
/// );
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct Expr<T, Scope = ()> {
    pub(super) kind: ExprKind,
    _result: PhantomData<fn() -> (T, Scope)>,
}

impl<T, Scope> Expr<T, Scope> {
    pub(super) fn new(kind: ExprKind) -> Self {
        Self {
            kind,
            _result: PhantomData,
        }
    }

    /// The expression tree of this expression.
    pub fn kind(&self) -> &ExprKind {
        &self.kind
    }

    /// Consumes the expression and returns its tree.
    pub fn into_kind(self) -> ExprKind {
        self.kind
    }

    /// Builds an `IS NULL` predicate over this expression.
    ///
    /// The predicate compares against SQL `NULL` itself instead of a bound
    /// parameter, so it works for any expression type, optional or not.
    pub fn is_null(&self) -> Expr<bool> {
        Expr::new(ExprKind::IsNull {
            operand: Box::new(self.kind.clone()),
            negated: false,
        })
    }

    /// Builds an `IS NOT NULL` predicate over this expression.
    pub fn is_not_null(&self) -> Expr<bool> {
        Expr::new(ExprKind::IsNull {
            operand: Box::new(self.kind.clone()),
            negated: true,
        })
    }

    /// Builds an `IN (subquery)` predicate matching this expression.
    ///
    /// The nested query renders inline with the enclosing statement, so its
    /// parameters continue the placeholder numbering of the outer query.
    pub fn in_subquery<S: Into<Subquery>>(self, subquery: S) -> Expr<bool> {
        Expr::new(ExprKind::Subquery {
            op: SubqueryOp::In,
            operand: Some(Box::new(self.kind)),
            query: subquery.into(),
            negated: false,
        })
    }

    /// Builds a `NOT IN (subquery)` predicate matching this expression.
    pub fn not_in_subquery<S: Into<Subquery>>(self, subquery: S) -> Expr<bool> {
        Expr::new(ExprKind::Subquery {
            op: SubqueryOp::In,
            operand: Some(Box::new(self.kind)),
            query: subquery.into(),
            negated: true,
        })
    }

    /// Compares the expression with `value` using the SQL `=` operator.
    pub fn eq<V>(self, value: V) -> Expr<bool>
    where
        V: IntoExpr<T>,
    {
        self.compare(BinaryOp::Eq, value.into_expr_kind())
    }

    /// Compares the expression with `value` using the SQL `<>` operator.
    pub fn ne<V>(self, value: V) -> Expr<bool>
    where
        V: IntoExpr<T>,
    {
        self.compare(BinaryOp::Ne, value.into_expr_kind())
    }

    /// Compares the expression with `value` using the SQL `>` operator.
    pub fn gt<V>(self, value: V) -> Expr<bool>
    where
        V: IntoExpr<T>,
    {
        self.compare(BinaryOp::Gt, value.into_expr_kind())
    }

    /// Compares the expression with `value` using the SQL `<` operator.
    pub fn lt<V>(self, value: V) -> Expr<bool>
    where
        V: IntoExpr<T>,
    {
        self.compare(BinaryOp::Lt, value.into_expr_kind())
    }

    /// Builds a `BETWEEN low AND high` range predicate over this expression.
    ///
    /// Both bounds are inclusive and must be compatible with the expression type.
    pub fn between<L, H>(self, low: L, high: H) -> Expr<bool>
    where
        L: IntoExpr<T>,
        H: IntoExpr<T>,
    {
        Expr::new(ExprKind::Between {
            operand: Box::new(self.kind),
            low: Box::new(low.into_expr_kind()),
            high: Box::new(high.into_expr_kind()),
        })
    }

    /// Builds an `IN (value, ...)` membership predicate over this expression.
    ///
    /// Every candidate value must be compatible with the expression type.
    pub fn in_list<I, V>(self, values: I) -> Expr<bool>
    where
        I: IntoIterator<Item = V>,
        V: IntoExpr<T>,
    {
        self.membership(values, false)
    }

    /// Builds a `NOT IN (value, ...)` negated membership predicate over this expression.
    pub fn not_in_list<I, V>(self, values: I) -> Expr<bool>
    where
        I: IntoIterator<Item = V>,
        V: IntoExpr<T>,
    {
        self.membership(values, true)
    }

    fn membership<I, V>(self, values: I, negated: bool) -> Expr<bool>
    where
        I: IntoIterator<Item = V>,
        V: IntoExpr<T>,
    {
        let values: Vec<ExprKind> = values
            .into_iter()
            .map(|value| value.into_expr_kind())
            .collect();

        if values.is_empty() {
            // `x IN ()` is a syntax error in every dialect, so an empty
            // membership degrades into a constant predicate: nothing is a member
            // of an empty set, and everything is absent from it.
            return Expr::new(ExprKind::Raw {
                fragments: vec![String::from(if negated { "1 = 1" } else { "1 = 0" })],
                params: Vec::new(),
            });
        }

        Expr::new(ExprKind::InList {
            operand: Box::new(self.kind),
            values,
            negated,
        })
    }

    fn compare(self, op: BinaryOp, rhs: ExprKind) -> Expr<bool> {
        Expr::new(ExprKind::Binary {
            op,
            lhs: Box::new(self.kind),
            rhs: Box::new(rhs),
        })
    }

    /// Builds a window expression from this expression over `window`.
    pub fn over(self, window: crate::query::WindowSpec) -> crate::query::WindowExpr<T> {
        crate::query::WindowExpr::from_expr(self.kind, window)
    }

    /// Starts a `LAG(C)` window function builder for column `C`.
    pub fn lag<C: Column<RustType = T>>(_col: C) -> crate::query::LagLeadFn<Option<T>> {
        crate::query::lag(_col)
    }

    /// Starts a `LEAD(C)` window function builder for column `C`.
    pub fn lead<C: Column<RustType = T>>(_col: C) -> crate::query::LagLeadFn<Option<T>> {
        crate::query::lead(_col)
    }
}

impl<T> Expr<T, ()> {
    /// Builds an expression holding the literal `value` typed as `T`.
    ///
    /// Literals are the constant operands of scalar function calls (see
    /// [`ExprKind::Function`]), where the value must carry the same type as the
    /// expression it accompanies.
    pub fn literal<V>(value: V) -> Self
    where
        V: IntoExpr<T>,
    {
        Self::new(value.into_expr_kind())
    }

    /// Starts a searched `CASE` expression whose branches produce values of type `T`.
    ///
    /// The builder starts in the [`NoBranches`](super::ast::NoBranches) state and
    /// only becomes finalisable once a [`CaseBuilder::when`] branch has been
    /// declared, so a `CASE` expression without branches does not compile.
    pub fn case() -> CaseBuilder<T> {
        CaseBuilder::new()
    }

    /// Builds a raw SQL expression with bound parameters and dialect-aware placeholder rendering.
    ///
    /// The template uses `?` or `{}` placeholders — never both — and must declare
    /// exactly one placeholder per bound parameter; a literal question mark is
    /// escaped as `??` and counts as no placeholder. A template that does not
    /// match its parameters is reported as [`RawSqlError`] instead of being
    /// rendered into invalid SQL.
    pub fn raw(template: &str, params: Vec<SqlVal>) -> Result<Self, RawSqlError> {
        let fragments = parse_raw_sql(template, params.len())?;
        Ok(Self::new(ExprKind::Raw { fragments, params }))
    }

    /// Builds a raw SQL expression from a template validated elsewhere.
    ///
    /// The `sql!` and `raw_sql!` macros check the placeholder census of their
    /// template at compile time and construct the expression through this
    /// constructor, so no unchecked template reaches the renderer.
    #[doc(hidden)]
    pub fn raw_validated(template: &str, params: Vec<SqlVal>) -> Self {
        Self::new(ExprKind::Raw {
            fragments: split_validated(template),
            params,
        })
    }

    /// Builds a `SUM(C)` expression for column `C`.
    pub fn sum<C>() -> Self
    where
        C: Column<RustType = T>,
    {
        Self::new(ExprKind::Aggregate {
            func: AggregateFunc::Sum,
            arg: Some(Box::new(ExprKind::Column { name: C::NAME })),
        })
    }

    /// Builds a `MIN(C)` expression for column `C`.
    pub fn min<C>() -> Self
    where
        C: Column<RustType = T>,
    {
        Self::new(ExprKind::Aggregate {
            func: AggregateFunc::Min,
            arg: Some(Box::new(ExprKind::Column { name: C::NAME })),
        })
    }

    /// Builds a `MAX(C)` expression for column `C`.
    pub fn max<C>() -> Self
    where
        C: Column<RustType = T>,
    {
        Self::new(ExprKind::Aggregate {
            func: AggregateFunc::Max,
            arg: Some(Box::new(ExprKind::Column { name: C::NAME })),
        })
    }
}

impl Expr<(), ()> {
    /// Builds an expression referencing the column `C`.
    ///
    /// The column fixes both the result type and the expression scope, so the
    /// returned expression is checked against the query scope at the call site.
    /// Columns of an aliased table build an alias-qualified reference, so
    /// `Expr::column::<AliasedCol<A, C>>()` renders as `"alias"."column"`.
    pub fn column<C>() -> Expr<C::RustType, C::Table>
    where
        C: Column,
    {
        Expr::new(C::expr_kind())
    }

    /// Builds an expression referencing the column `C` qualified by its table `Q`.
    ///
    /// Qualified references render as `"table"."column"`, which keeps joins over
    /// tables that share a column name unambiguous.
    pub fn qualified_column<Q, C>() -> Expr<C::RustType, Q>
    where
        Q: Table,
        C: Column<Table = Q>,
    {
        Expr::new(ExprKind::QualifiedColumn {
            table: Q::NAME,
            column: C::NAME,
        })
    }
}
