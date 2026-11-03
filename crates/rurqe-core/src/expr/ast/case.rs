//! The searched `CASE` expression builder and its branch typestate markers.
//!
//! A searched `CASE` expression is assembled branch by branch, and the builder
//! only becomes finalisable once at least one `WHEN` branch has been declared:
//! [`NoBranches`] and [`HasBranches`] record that state, so a `CASE` without a
//! branch does not compile. The branch node lives here as well, because it is
//! the element the builder accumulates.

use core::marker::PhantomData;

use crate::expr::{
    Aggregate, CombineAggr, CombinedAggr, Expr, ExprKind, FunctionOperand, MergeDialect,
    MergeScope, Merged, MergedDialect,
};

/// A single `WHEN condition THEN result` branch of a [`ExprKind::Case`] expression.
#[derive(Debug, Clone, PartialEq)]
pub struct CaseBranch {
    /// The predicate guarding the branch.
    pub condition: ExprKind,
    /// The value produced when `condition` holds.
    pub result: ExprKind,
}

/// Typestate marker of a `CASE` builder that has declared no `WHEN` branch yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NoBranches;

/// Typestate marker of a `CASE` builder holding at least one `WHEN` branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HasBranches;

/// Phantom marker carrying the result type, the branch state, the referenced
/// scope, the aggregation marker, and the dialect marker of a [`CaseBuilder`].
type CaseMarker<T, Branches, Scope, Aggr, D> = PhantomData<fn() -> (T, Branches, Scope, Aggr, D)>;

/// Builder for a searched `CASE` expression producing values of type `T`.
///
/// The builder carries the tables `Scope`, the dialect requirements `D`, and the
/// aggregation marker `Aggr` that its conditions and results contribute. The
/// aggregation marker starts at [`Aggregate`] — the identity of the folding
/// algebra, since a `CASE` without operands is a constant — and every branch
/// folds its own markers into it through [`CombineAggr`]:
///
/// - a condition or result that is an aggregate keeps the whole expression an
///   aggregate, so `CASE WHEN SUM("total") > 0 THEN SUM("total") ELSE 0 END`
///   stays projectable from a grouped query;
/// - a condition or result reading a bare column — or an operand whose scope was
///   dropped with [`Expr::without_scope`] — computes one value per row, so the
///   whole expression becomes [`NonAggregate`](crate::expr::NonAggregate) and a
///   grouped query rejects it unless every column it reads is grouped;
/// - a bound value references no table, so it leaves the marker of the remaining
///   operands standing.
///
/// The finished expression therefore keeps the scope, the dialect constraints,
/// and the aggregation marker of every operand instead of degrading into a
/// constant.
#[derive(Debug, Clone, PartialEq)]
pub struct CaseBuilder<T, Branches = NoBranches, Scope = (), Aggr = Aggregate, D = ()> {
    branches: Vec<CaseBranch>,
    else_branch: Option<ExprKind>,
    _marker: CaseMarker<T, Branches, Scope, Aggr, D>,
}

impl<T> CaseBuilder<T, NoBranches, (), Aggregate, ()> {
    pub(in crate::expr) fn new() -> Self {
        Self {
            branches: Vec::new(),
            else_branch: None,
            _marker: PhantomData,
        }
    }
}

impl<T, Branches, Scope, Aggr, D> CaseBuilder<T, Branches, Scope, Aggr, D> {
    /// Appends a `WHEN condition THEN result` branch.
    ///
    /// The branch folds the aggregation markers of both the condition and the
    /// result into the marker of the whole expression, so a branch reading a
    /// bare column makes the expression per-row while a branch over aggregates
    /// keeps it an aggregate.
    #[allow(clippy::type_complexity)]
    pub fn when<ConditionScope, ConditionAggr, ConditionD, V>(
        mut self,
        condition: Expr<bool, ConditionScope, ConditionAggr, ConditionD>,
        result: V,
    ) -> CaseBuilder<
        T,
        HasBranches,
        Merged<Merged<Scope, ConditionScope>, V::Scope>,
        CombinedAggr<CombinedAggr<Aggr, ConditionAggr, ConditionScope>, V::Aggr, V::Scope>,
        MergedDialect<MergedDialect<D, ConditionD>, V::Dialect>,
    >
    where
        Scope: MergeScope<ConditionScope>,
        Merged<Scope, ConditionScope>: MergeScope<V::Scope>,
        Aggr: CombineAggr<ConditionAggr, ConditionScope>,
        CombinedAggr<Aggr, ConditionAggr, ConditionScope>: CombineAggr<V::Aggr, V::Scope>,
        D: MergeDialect<ConditionD>,
        MergedDialect<D, ConditionD>: MergeDialect<V::Dialect>,
        V: FunctionOperand<T>,
    {
        self.branches.push(CaseBranch {
            condition: condition.into_kind(),
            result: result.into_operand_kind(),
        });
        CaseBuilder {
            branches: self.branches,
            else_branch: self.else_branch,
            _marker: PhantomData,
        }
    }

    /// Sets the `ELSE` fallback rendered when no branch matches.
    ///
    /// The fallback folds its own aggregation marker into the marker of the
    /// whole expression, exactly as a branch result does.
    #[allow(clippy::type_complexity)]
    pub fn else_<V>(
        mut self,
        value: V,
    ) -> CaseBuilder<
        T,
        Branches,
        Merged<Scope, V::Scope>,
        CombinedAggr<Aggr, V::Aggr, V::Scope>,
        MergedDialect<D, V::Dialect>,
    >
    where
        Scope: MergeScope<V::Scope>,
        Aggr: CombineAggr<V::Aggr, V::Scope>,
        D: MergeDialect<V::Dialect>,
        V: FunctionOperand<T>,
    {
        self.else_branch = Some(value.into_operand_kind());
        CaseBuilder {
            branches: self.branches,
            else_branch: self.else_branch,
            _marker: PhantomData,
        }
    }
}

impl<T, Scope, Aggr, D> CaseBuilder<T, HasBranches, Scope, Aggr, D> {
    /// Finalises the expression.
    pub fn end(self) -> Expr<T, Scope, Aggr, D> {
        Expr::new(ExprKind::Case {
            branches: self.branches,
            else_branch: self.else_branch.map(Box::new),
        })
    }
}
