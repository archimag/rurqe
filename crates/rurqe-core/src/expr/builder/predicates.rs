//! Comparisons, ranges, membership tests, and the null predicates of an expression.
//!
//! Every method checks operand compatibility at the call site and merges the
//! scopes of its operands, so a predicate comparing a value read from one table
//! with a value read from another references both tables and is accepted only by
//! a query selecting from them.

use super::Expr;
use crate::expr::ast::{ExprKind, MergeDialect, MergedDialect, SubqueryOp};
use crate::expr::ops::{InSubquery, IntoExpr, IntoExprList};
use crate::expr::scope::{MergeScope, Merged, Merged3};
use crate::expr::val::BinaryOp;

impl<T, Scope, Aggr, D> Expr<T, Scope, Aggr, D> {
    /// Builds an `IS NULL` predicate over this expression.
    ///
    /// The predicate compares against SQL `NULL` itself instead of a bound
    /// parameter, so it works for any expression type, optional or not.
    pub fn is_null(&self) -> Expr<bool, Scope, Aggr, D> {
        Expr::new(ExprKind::IsNull {
            operand: Box::new(self.kind.clone()),
            negated: false,
        })
    }

    /// Builds an `IS NOT NULL` predicate over this expression.
    pub fn is_not_null(&self) -> Expr<bool, Scope, Aggr, D> {
        Expr::new(ExprKind::IsNull {
            operand: Box::new(self.kind.clone()),
            negated: true,
        })
    }

    /// Builds an `IN (subquery)` predicate matching this expression.
    ///
    /// The nested query renders inline with the enclosing statement, so its
    /// parameters continue the placeholder numbering of the outer query.
    pub fn in_subquery<S: InSubquery>(
        self,
        subquery: S,
    ) -> Expr<bool, Scope, Aggr, MergedDialect<D, S::Dialect>>
    where
        D: MergeDialect<S::Dialect>,
    {
        Expr::new(ExprKind::Subquery {
            op: SubqueryOp::In,
            operand: Some(Box::new(self.kind)),
            query: subquery.into_subquery(),
            negated: false,
        })
    }

    /// Builds a `NOT IN (subquery)` predicate matching this expression.
    pub fn not_in_subquery<S: InSubquery>(
        self,
        subquery: S,
    ) -> Expr<bool, Scope, Aggr, MergedDialect<D, S::Dialect>>
    where
        D: MergeDialect<S::Dialect>,
    {
        Expr::new(ExprKind::Subquery {
            op: SubqueryOp::In,
            operand: Some(Box::new(self.kind)),
            query: subquery.into_subquery(),
            negated: true,
        })
    }

    /// Compares the expression with `value` using the SQL `=` operator.
    #[allow(clippy::type_complexity)]
    pub fn eq<V>(
        self,
        value: V,
    ) -> Expr<bool, Merged<Scope, V::Scope>, Aggr, MergedDialect<D, V::Dialect>>
    where
        Scope: MergeScope<V::Scope>,
        D: MergeDialect<V::Dialect>,
        V: IntoExpr<T>,
    {
        self.compare(BinaryOp::Eq, value.into_expr_kind())
    }

    /// Compares the expression with `value` using the SQL `<>` operator.
    #[allow(clippy::type_complexity)]
    pub fn ne<V>(
        self,
        value: V,
    ) -> Expr<bool, Merged<Scope, V::Scope>, Aggr, MergedDialect<D, V::Dialect>>
    where
        Scope: MergeScope<V::Scope>,
        D: MergeDialect<V::Dialect>,
        V: IntoExpr<T>,
    {
        self.compare(BinaryOp::Ne, value.into_expr_kind())
    }

    /// Compares the expression with `value` using the SQL `>` operator.
    #[allow(clippy::type_complexity)]
    pub fn gt<V>(
        self,
        value: V,
    ) -> Expr<bool, Merged<Scope, V::Scope>, Aggr, MergedDialect<D, V::Dialect>>
    where
        Scope: MergeScope<V::Scope>,
        D: MergeDialect<V::Dialect>,
        V: IntoExpr<T>,
    {
        self.compare(BinaryOp::Gt, value.into_expr_kind())
    }

    /// Compares the expression with `value` using the SQL `<` operator.
    #[allow(clippy::type_complexity)]
    pub fn lt<V>(
        self,
        value: V,
    ) -> Expr<bool, Merged<Scope, V::Scope>, Aggr, MergedDialect<D, V::Dialect>>
    where
        Scope: MergeScope<V::Scope>,
        D: MergeDialect<V::Dialect>,
        V: IntoExpr<T>,
    {
        self.compare(BinaryOp::Lt, value.into_expr_kind())
    }

    /// Builds a `BETWEEN low AND high` range predicate over this expression.
    ///
    /// Both bounds are inclusive and must be compatible with the expression type.
    #[allow(clippy::type_complexity)]
    pub fn between<L, H>(
        self,
        low: L,
        high: H,
    ) -> Expr<
        bool,
        Merged3<Scope, L::Scope, H::Scope>,
        Aggr,
        MergedDialect<MergedDialect<D, L::Dialect>, H::Dialect>,
    >
    where
        Scope: MergeScope<L::Scope>,
        Merged<Scope, L::Scope>: MergeScope<H::Scope>,
        D: MergeDialect<L::Dialect>,
        MergedDialect<D, L::Dialect>: MergeDialect<H::Dialect>,
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
    /// Every candidate value must be compatible with the expression type, and the
    /// tables the candidates reference join the scope of the predicate: a list of
    /// bound values references no table and leaves the scope unchanged, while a
    /// candidate reading a column of another table extends it, so the enclosing
    /// query has to select from that table as well.
    #[allow(clippy::type_complexity)]
    pub fn in_list<I>(
        self,
        values: I,
    ) -> Expr<bool, Merged<Scope, I::Scope>, Aggr, MergedDialect<D, I::Dialect>>
    where
        Scope: MergeScope<I::Scope>,
        D: MergeDialect<I::Dialect>,
        I: IntoExprList<T>,
    {
        self.membership(values, false)
    }

    /// Builds a `NOT IN (value, ...)` negated membership predicate over this expression.
    ///
    /// The negation keeps the scope of the positive form: every candidate value
    /// still has to be comparable with the expression, and the tables it
    /// references still join the scope of the predicate.
    #[allow(clippy::type_complexity)]
    pub fn not_in_list<I>(
        self,
        values: I,
    ) -> Expr<bool, Merged<Scope, I::Scope>, Aggr, MergedDialect<D, I::Dialect>>
    where
        Scope: MergeScope<I::Scope>,
        D: MergeDialect<I::Dialect>,
        I: IntoExprList<T>,
    {
        self.membership(values, true)
    }

    #[allow(clippy::type_complexity)]
    fn membership<I>(
        self,
        values: I,
        negated: bool,
    ) -> Expr<bool, Merged<Scope, I::Scope>, Aggr, MergedDialect<D, I::Dialect>>
    where
        Scope: MergeScope<I::Scope>,
        D: MergeDialect<I::Dialect>,
        I: IntoExprList<T>,
    {
        let mut candidates = Vec::new();
        values.push_expr_kinds(&mut candidates);

        if candidates.is_empty() {
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
            values: candidates,
            negated,
        })
    }

    fn compare<TargetScope, TargetDialect>(
        self,
        op: BinaryOp,
        rhs: ExprKind,
    ) -> Expr<bool, TargetScope, Aggr, TargetDialect> {
        Expr::new(ExprKind::Binary {
            op,
            lhs: Box::new(self.kind),
            rhs: Box::new(rhs),
        })
    }
}
