//! The arithmetic operators of an expression.
//!
//! Arithmetic follows the standard operator traits, so a numeric expression
//! composes the way the SQL it renders reads: `Expr::column::<OrderTotal>() + 1.5`
//! renders `("total" + $1)`. The database evaluates the operator, which is why the
//! operands only have to carry the same numeric type and no value is folded on the
//! client. The aggregation marker of the produced expression is derived by
//! [`CombineAggr`] from the marker of the left operand and the operand on the
//! right, so an aggregate scaled by a constant stays projectable from a grouped
//! query, while an aggregate combined with a column read per row stops being one:
//!
//! ```text
//! SUM("total") + $1     -> an aggregate
//! SUM("total") + "tax"  -> one value per row
//! ```

use core::ops::{Add, Div, Mul, Rem, Sub};

use super::Expr;
use crate::expr::ast::{ExprKind, MergeDialect, MergedDialect};
use crate::expr::ops::{CombineAggr, CombinedAggr, Numeric, NumericOperand};
use crate::expr::scope::{MergeScope, Merged};
use crate::expr::val::BinaryOp;

impl<T, Scope, Aggr, D> Expr<T, Scope, Aggr, D> {
    /// Builds an arithmetic expression applying `op` to this expression and `rhs`.
    ///
    /// The method backs the operator overloads of [`Expr`], so the operator is
    /// applied without inspecting the operands: the database evaluates the
    /// expression, which keeps an overflow or a division by zero the decision of
    /// the server instead of the client.
    fn arithmetic<TargetScope, TargetAggr, TargetDialect>(
        self,
        op: BinaryOp,
        rhs: ExprKind,
    ) -> Expr<T, TargetScope, TargetAggr, TargetDialect> {
        Expr::new(ExprKind::Binary {
            op,
            lhs: Box::new(self.kind),
            rhs: Box::new(rhs),
        })
    }
}

impl<T, Scope, Aggr, D, Rhs> Add<Rhs> for Expr<T, Scope, Aggr, D>
where
    T: Numeric,
    Scope: MergeScope<Rhs::Scope>,
    D: MergeDialect<Rhs::Dialect>,
    Rhs: NumericOperand<T>,
    Aggr: CombineAggr<Rhs::Aggr, Rhs::Scope>,
{
    type Output = Expr<
        T,
        Merged<Scope, Rhs::Scope>,
        CombinedAggr<Aggr, Rhs::Aggr, Rhs::Scope>,
        MergedDialect<D, Rhs::Dialect>,
    >;

    fn add(self, rhs: Rhs) -> Self::Output {
        self.arithmetic(BinaryOp::Add, rhs.into_operand_kind())
    }
}

impl<T, Scope, Aggr, D, Rhs> Sub<Rhs> for Expr<T, Scope, Aggr, D>
where
    T: Numeric,
    Scope: MergeScope<Rhs::Scope>,
    D: MergeDialect<Rhs::Dialect>,
    Rhs: NumericOperand<T>,
    Aggr: CombineAggr<Rhs::Aggr, Rhs::Scope>,
{
    type Output = Expr<
        T,
        Merged<Scope, Rhs::Scope>,
        CombinedAggr<Aggr, Rhs::Aggr, Rhs::Scope>,
        MergedDialect<D, Rhs::Dialect>,
    >;

    fn sub(self, rhs: Rhs) -> Self::Output {
        self.arithmetic(BinaryOp::Sub, rhs.into_operand_kind())
    }
}

impl<T, Scope, Aggr, D, Rhs> Mul<Rhs> for Expr<T, Scope, Aggr, D>
where
    T: Numeric,
    Scope: MergeScope<Rhs::Scope>,
    D: MergeDialect<Rhs::Dialect>,
    Rhs: NumericOperand<T>,
    Aggr: CombineAggr<Rhs::Aggr, Rhs::Scope>,
{
    type Output = Expr<
        T,
        Merged<Scope, Rhs::Scope>,
        CombinedAggr<Aggr, Rhs::Aggr, Rhs::Scope>,
        MergedDialect<D, Rhs::Dialect>,
    >;

    fn mul(self, rhs: Rhs) -> Self::Output {
        self.arithmetic(BinaryOp::Mul, rhs.into_operand_kind())
    }
}

impl<T, Scope, Aggr, D, Rhs> Div<Rhs> for Expr<T, Scope, Aggr, D>
where
    T: Numeric,
    Scope: MergeScope<Rhs::Scope>,
    D: MergeDialect<Rhs::Dialect>,
    Rhs: NumericOperand<T>,
    Aggr: CombineAggr<Rhs::Aggr, Rhs::Scope>,
{
    type Output = Expr<
        T,
        Merged<Scope, Rhs::Scope>,
        CombinedAggr<Aggr, Rhs::Aggr, Rhs::Scope>,
        MergedDialect<D, Rhs::Dialect>,
    >;

    fn div(self, rhs: Rhs) -> Self::Output {
        self.arithmetic(BinaryOp::Div, rhs.into_operand_kind())
    }
}

impl<T, Scope, Aggr, D, Rhs> Rem<Rhs> for Expr<T, Scope, Aggr, D>
where
    T: Numeric,
    Scope: MergeScope<Rhs::Scope>,
    D: MergeDialect<Rhs::Dialect>,
    Rhs: NumericOperand<T>,
    Aggr: CombineAggr<Rhs::Aggr, Rhs::Scope>,
{
    type Output = Expr<
        T,
        Merged<Scope, Rhs::Scope>,
        CombinedAggr<Aggr, Rhs::Aggr, Rhs::Scope>,
        MergedDialect<D, Rhs::Dialect>,
    >;

    fn rem(self, rhs: Rhs) -> Self::Output {
        self.arithmetic(BinaryOp::Rem, rhs.into_operand_kind())
    }
}
