//! The `DISTINCT` and `FILTER (WHERE ...)` clauses of an aggregate function call.
//!
//! Both clauses restrict the rows the call aggregates, so they are written on the
//! call itself: the methods are reachable through [`AcceptsAggregateClause`]
//! alone.

use super::markers::{AcceptsAggregateClause, FilteredAggregate};
use crate::expr::ast::{ExprKind, MergeDialect, MergedDialect};
use crate::expr::scope::{MergeScope, Merged};
use crate::expr::Expr;

/// Text rendered in place of a `FILTER (WHERE ...)` clause attached to an
/// expression that is not an aggregate call.
///
/// The clause belongs to an aggregate call, and the [`AcceptsAggregateClause`]
/// gate keeps [`Expr::filter_where`] unreachable on any other expression, so the
/// public API cannot produce this situation. The constant is the total defence
/// the guard falls back to for a tree assembled by hand — a downstream crate
/// forging the marker onto a root that is no aggregate call — because such a
/// branch must neither unwind nor silently drop the predicate the return type
/// keeps promising: the placeholder keeps the fault visible in every build
/// profile, and a release build shows the discarded clause instead of hiding it.
const DROPPED_FILTER_PLACEHOLDER: &str = "/* invalid aggregate marker: FILTER predicate dropped */";

/// Text rendered in place of the `DISTINCT` modifier attached to an expression
/// that is not an aggregate call.
///
/// The modifier belongs to an aggregate call, and the [`AcceptsAggregateClause`]
/// gate keeps [`Expr::distinct`] unreachable on any other expression, so the
/// public API cannot produce this situation. The constant is the total defence
/// the guard falls back to for a tree assembled by hand — a downstream crate
/// forging the marker onto a root that is no aggregate call — because such a
/// branch must neither unwind nor silently drop the modifier the return type
/// keeps promising: the placeholder keeps the fault visible in every build
/// profile, and a release build shows the dropped modifier instead of hiding it.
const DROPPED_DISTINCT_PLACEHOLDER: &str =
    "/* invalid aggregate marker: DISTINCT modifier dropped */";

impl<T, Scope, Marker, D> Expr<T, Scope, Marker, D>
where
    Marker: AcceptsAggregateClause,
{
    /// Marks the aggregate expression to evaluate only over `DISTINCT` values.
    ///
    /// The modifier belongs to the aggregate call itself, so the method exists on
    /// [`DirectAggregate`](crate::expr::DirectAggregate) and [`FilteredAggregate`]
    /// only: an expression combining the call with arithmetic or a scalar function
    /// carries [`Aggregate`](crate::expr::Aggregate) and offers no `distinct`,
    /// which keeps the modifier from being written where SQL has no place for it.
    ///
    /// A per-row expression is no aggregate call, so no modifier applies to it:
    ///
    /// ```compile_fail
    /// use rurqe_core::expr::Expr;
    ///
    /// rurqe_core::table!(
    ///     Order => "orders" {
    ///         OrderTotal => "total": f64,
    ///     }
    /// );
    ///
    /// // Fails to compile: a column reference aggregates nothing, so there is
    /// // no call the `DISTINCT` modifier could restrict.
    /// let _ = Expr::column::<OrderTotal>().distinct();
    /// ```
    ///
    /// An aggregate call takes the modifier, and the produced expression still is
    /// that call:
    ///
    /// ```
    /// use rurqe_core::expr::{sum, ExprKind};
    ///
    /// rurqe_core::table!(
    ///     Order => "orders" {
    ///         OrderTotal => "total": f64,
    ///     }
    /// );
    ///
    /// let distinct = sum(OrderTotal).distinct();
    ///
    /// assert!(matches!(
    ///     distinct.kind(),
    ///     ExprKind::Aggregate { distinct: true, .. }
    /// ));
    /// ```
    ///
    /// The method never discards the modifier it applies. The
    /// [`AcceptsAggregateClause`] gate keeps it unreachable on a tree that is no
    /// aggregate call, so the fallback below covers a tree assembled by hand — a
    /// downstream crate forging the marker onto a non-aggregate root — instead of
    /// a state the public API reaches: the branch stays total in every build
    /// profile, because an assertion would fire in a debug build alone and a
    /// release build would silently drop the modifier the return type keeps
    /// promising. The trace records the contradiction and the returned tree
    /// carries a diagnostic placeholder in place of the modifier, so the rendered
    /// statement shows the dropped `DISTINCT` instead of hiding it.
    pub fn distinct(self) -> Self {
        // The marker gate restricts the method to an aggregate call, so the
        // public API cannot reach this branch. The check stays a total branch
        // rather than an assertion, because it guards a tree assembled by hand
        // whose marker contradicts its root: an assertion fires in a debug build
        // alone, so a release build would silently drop the modifier the return
        // type keeps promising, which the panic-free invariant of the crate
        // forbids just as much as unwinding would.
        let ExprKind::Aggregate {
            func, arg, filter, ..
        } = self.kind
        else {
            // Only an aggregate call accepts the modifier, so the public API
            // never reaches this branch: every aggregate constructor roots its
            // tree in the call. The branch guards a tree assembled by hand whose
            // marker contradicts its root, and it keeps the method total — and
            // the crate panic-free — while reporting the contradiction in both
            // build profiles: the trace records it, and the returned tree carries
            // a diagnostic placeholder where the modifier would have been, so the
            // rendered statement shows the dropped `DISTINCT` instead of hiding
            // it.
            #[cfg(feature = "tracing")]
            tracing::error!(
                "an expression marked as an aggregate call does not hold one; its DISTINCT modifier is replaced by a diagnostic placeholder"
            );
            return Expr::new(ExprKind::Raw {
                fragments: vec![DROPPED_DISTINCT_PLACEHOLDER.to_owned()],
                params: Vec::new(),
            });
        };

        Expr::new(ExprKind::Aggregate {
            func,
            distinct: true,
            arg,
            filter,
        })
    }

    /// Adds a `FILTER (WHERE ...)` clause restricting the rows this aggregate evaluates.
    ///
    /// Multiple calls combine their conditions with the SQL `AND` operator, and
    /// the expression records the filter in its tree instead of in the statement
    /// that renders it. The aggregate therefore stays a dialect-neutral value and
    /// the caller chooses the dialect when rendering the statement it is
    /// projected into.
    ///
    /// The clause belongs to the aggregate call itself, so the method exists on
    /// [`DirectAggregate`](crate::expr::DirectAggregate) and [`FilteredAggregate`]
    /// only, and the produced expression carries [`FilteredAggregate`], so the
    /// projection and filtering rules of a grouped query keep accepting it and
    /// repeated calls keep accumulating their conditions on the same call.
    ///
    /// A per-row expression is no aggregate call, so no clause applies to it:
    ///
    /// ```compile_fail
    /// use rurqe_core::expr::Expr;
    ///
    /// rurqe_core::table!(
    ///     Order => "orders" {
    ///         OrderTotal => "total": f64,
    ///     }
    /// );
    ///
    /// // Fails to compile: a column reference aggregates nothing, so there is
    /// // no call for the `FILTER` clause to restrict.
    /// let _ = Expr::column::<OrderTotal>()
    ///     .filter_where(Expr::column::<OrderTotal>().gt(0.0));
    /// ```
    ///
    /// The method never discards the clause it is given. The
    /// [`AcceptsAggregateClause`] gate keeps it unreachable on a tree that is no
    /// aggregate call, so the fallback below covers a tree assembled by hand — a
    /// downstream crate forging the marker onto a non-aggregate root — instead of
    /// a state the public API reaches: the branch stays total in every build
    /// profile, because an assertion would fire in a debug build alone and a
    /// release build would silently discard the clause the return type keeps
    /// promising. The trace records the contradiction and the returned tree
    /// carries a diagnostic placeholder where the `FILTER` clause would have
    /// been, so the rendered statement shows the discarded predicate instead of
    /// hiding it.
    ///
    /// ```
    /// use rurqe_core::expr::{sum, Expr, ExprKind};
    ///
    /// rurqe_core::table!(
    ///     Order => "orders" {
    ///         OrderTotal => "total": f64,
    ///     }
    /// );
    /// let filtered = sum(OrderTotal)
    ///     .filter_where(Expr::column::<OrderTotal>().gt(10.0))
    ///     .filter_where(Expr::column::<OrderTotal>().lt(100.0));
    ///
    /// assert!(matches!(
    ///     filtered.kind(),
    ///     ExprKind::Aggregate { filter: Some(_), .. }
    /// ));
    /// ```
    ///
    /// An expression built from the call is no longer the call, so it accepts no
    /// clause at all:
    ///
    /// ```compile_fail
    /// use rurqe_core::expr::{sum, Expr};
    ///
    /// rurqe_core::table!(
    ///     Order => "orders" {
    ///         OrderTotal => "total": f64,
    ///     }
    /// );
    ///
    /// // Fails to compile: the arithmetic combination is an `Aggregate`, not the
    /// // aggregate call itself, so it takes no `FILTER` clause.
    /// let _ = (sum(OrderTotal) + 1.0)
    ///     .filter_where(Expr::column::<OrderTotal>().gt(0.0));
    /// ```
    ///
    /// A restricted call degrades exactly like a bare one, because the filter
    /// recorded in its tree belongs to the call and to nothing built over it:
    ///
    /// ```compile_fail
    /// use rurqe_core::expr::{sum, Expr};
    ///
    /// rurqe_core::table!(
    ///     Order => "orders" {
    ///         OrderTotal => "total": f64,
    ///     }
    /// );
    ///
    /// // Fails to compile: the scaled filtered call is an `Aggregate` as well,
    /// // so the `FILTER` clause it would take names no call to restrict.
    /// let _ = (sum(OrderTotal).filter_where(Expr::column::<OrderTotal>().gt(0.0)) * 1.0)
    ///     .filter_where(Expr::column::<OrderTotal>().gt(0.0));
    /// ```
    pub fn filter_where<P, PredAggr, PredD>(
        self,
        predicate: Expr<bool, P, PredAggr, PredD>,
    ) -> Expr<T, Merged<Scope, P>, FilteredAggregate, MergedDialect<D, PredD>>
    where
        Scope: MergeScope<P>,
        D: MergeDialect<PredD>,
    {
        // The marker gate restricts the method to an aggregate call, so the
        // public API cannot reach this branch. The check stays a total branch
        // rather than an assertion, because it guards a tree assembled by hand
        // whose marker contradicts its root: an assertion fires in a debug build
        // alone, so a release build would silently discard the clause the return
        // type keeps promising, which the panic-free invariant of the crate
        // forbids just as much as unwinding would.
        let ExprKind::Aggregate {
            func,
            distinct,
            arg,
            filter,
        } = self.kind
        else {
            // Only an aggregate call accepts the clause, so the public API never
            // reaches this branch: every aggregate constructor roots its tree in
            // the call. The branch guards a tree assembled by hand whose marker
            // contradicts its root, and it keeps the method total — and the crate
            // panic-free — while reporting the contradiction in both build
            // profiles: the trace records it, and the returned tree carries a
            // diagnostic placeholder where the `FILTER` clause would have been,
            // so the rendered statement shows the discarded predicate instead of
            // hiding it.
            #[cfg(feature = "tracing")]
            tracing::error!(
                "an expression marked as an aggregate call does not hold one; its FILTER predicate is replaced by a diagnostic placeholder"
            );
            return Expr::new(ExprKind::Raw {
                fragments: vec![DROPPED_FILTER_PLACEHOLDER.to_owned()],
                params: Vec::new(),
            });
        };

        let new_filter = match filter {
            Some(existing) => Box::new(ExprKind::Binary {
                op: crate::expr::val::BinaryOp::And,
                lhs: existing,
                rhs: Box::new(predicate.into_kind()),
            }),
            None => Box::new(predicate.into_kind()),
        };

        Expr::new(ExprKind::Aggregate {
            func,
            distinct,
            arg,
            filter: Some(new_filter),
        })
    }
}
