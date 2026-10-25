//! Aggregate construction and the markers the grouped-query rules rely on.
//!
//! An aggregate collapses the rows of a group into a single value, which is why
//! the produced expression carries the [`DirectAggregate`] marker instead of the
//! default `NonAggregate` one. The marker records two facts about the expression:
//! it collapses the rows of its group, and its root node still is the aggregate
//! call itself, so the optional `DISTINCT` and `FILTER (WHERE ...)` clauses of
//! that call apply to it directly — the property [`AcceptsAggregateClause`]
//! states.
//!
//! Every composite expression built from an aggregate — an arithmetic
//! combination, a scalar function call, a `CASE` branch — degrades to the general
//! [`Aggregate`] marker through [`CombineAggr`](crate::expr::CombineAggr). It
//! keeps collapsing its group and therefore stays projectable from a grouped
//! query, but its root node is the operator combining the calls, so SQL does not
//! allow `DISTINCT` or `FILTER` on it and neither method exists for it.
//! Restricting an aggregate with a `FILTER (WHERE ...)` clause switches it to the
//! [`FilteredAggregate`] marker, which keeps accepting the clauses so repeated
//! calls accumulate their conditions.

use super::{Aggregate, Expr};
use crate::expr::ast::{ExprKind, MergeDialect, MergedDialect};
use crate::expr::scope::{MergeScope, Merged};
use crate::expr::val::AggregateFunc;
use crate::schema::Column;

impl<T> Expr<T, ()> {
    /// Builds a `SUM(C)` expression for column `C`.
    ///
    /// The produced expression carries the [`DirectAggregate`] marker: it
    /// collapses the rows of its group, so it stays projectable from a grouped
    /// query even when `C` is not grouped, and its root node is the call itself,
    /// so `distinct` and `filter_where` apply to it. It also references the table
    /// of `C`, so a statement whose scope lacks that table rejects it like any
    /// other column reference.
    pub fn sum<C>() -> Expr<T, C::Table, DirectAggregate>
    where
        C: Column<RustType = T>,
    {
        Expr::new(ExprKind::Aggregate {
            func: AggregateFunc::Sum,
            distinct: false,
            arg: Some(Box::new(C::expr_kind())),
            filter: None,
        })
    }

    /// Builds a `SUM(DISTINCT C)` expression for column `C`.
    ///
    /// The scope is the table of `C` in both the distinct and the plain form, so
    /// the aggregate is checked against the statement it is projected into.
    pub fn sum_distinct<C>() -> Expr<T, C::Table, DirectAggregate>
    where
        C: Column<RustType = T>,
    {
        Expr::new(ExprKind::Aggregate {
            func: AggregateFunc::Sum,
            distinct: true,
            arg: Some(Box::new(C::expr_kind())),
            filter: None,
        })
    }

    /// Builds a `MIN(C)` expression for column `C`.
    ///
    /// The produced expression carries the [`DirectAggregate`] marker, so it
    /// stays projectable from a grouped query even when `C` is not grouped, and
    /// it accepts the `DISTINCT` and `FILTER (WHERE ...)` clauses of the call. It
    /// also references the table of `C`, so a statement whose scope lacks that
    /// table rejects it like any other column reference.
    pub fn min<C>() -> Expr<T, C::Table, DirectAggregate>
    where
        C: Column<RustType = T>,
    {
        Expr::new(ExprKind::Aggregate {
            func: AggregateFunc::Min,
            distinct: false,
            arg: Some(Box::new(C::expr_kind())),
            filter: None,
        })
    }

    /// Builds a `MAX(C)` expression for column `C`.
    ///
    /// The produced expression carries the [`DirectAggregate`] marker, so it
    /// stays projectable from a grouped query even when `C` is not grouped, and
    /// it accepts the `DISTINCT` and `FILTER (WHERE ...)` clauses of the call. It
    /// also references the table of `C`, so a statement whose scope lacks that
    /// table rejects it like any other column reference.
    pub fn max<C>() -> Expr<T, C::Table, DirectAggregate>
    where
        C: Column<RustType = T>,
    {
        Expr::new(ExprKind::Aggregate {
            func: AggregateFunc::Max,
            distinct: false,
            arg: Some(Box::new(C::expr_kind())),
            filter: None,
        })
    }
}

/// Marker of an expression whose root node is a direct aggregate function call.
///
/// The expression collapses the rows of its group and keeps the aggregate call at
/// its root, so the optional `DISTINCT` and `FILTER (WHERE ...)` clauses of the
/// call are written on it directly. Combining the call with arithmetic, a scalar
/// function, or a `CASE` branch moves the expression to the general [`Aggregate`]
/// marker, which still collapses its group but no longer accepts those clauses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DirectAggregate;

/// Marker of an aggregate expression restricted by a `FILTER (WHERE ...)` clause.
///
/// A filtered aggregate collapses the rows of its group exactly like a plain
/// aggregate; the marker only records that the aggregate no longer reads every
/// row of the group. Arithmetic combinations with constants preserve it, so a
/// filtered aggregate scaled by a value stays projectable from a grouped query.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FilteredAggregate;

/// A marker stating that an expression collapses the rows of its group.
///
/// The marker is implemented by [`DirectAggregate`], [`FilteredAggregate`], and
/// [`Aggregate`], so the projection and filtering rules of a grouped query accept
/// every expression that collapses its group, whether or not its root node still
/// is the aggregate call.
#[doc(hidden)]
pub trait AggregateMarker {}

impl AggregateMarker for DirectAggregate {}

impl AggregateMarker for FilteredAggregate {}

impl AggregateMarker for Aggregate {}

/// A marker stating that the root node of an expression is an aggregate call.
///
/// The `DISTINCT` and `FILTER (WHERE ...)` clauses belong to an aggregate call
/// itself: they restrict the rows the call aggregates, so they exist only where an
/// aggregate is invoked. A composite expression built from an aggregate —
/// `SUM("total") + $1`, `COALESCE(SUM("total"), $1)`, a `CASE` over aggregates —
/// collapses its group as well and stays projectable from a grouped query, but its
/// root node is an operator or a function, so neither clause can be attached to
/// it.
///
/// The marker implemented by [`DirectAggregate`] and [`FilteredAggregate`] gates
/// [`Expr::distinct`] and [`Expr::filter_where`], so a clause is reachable only on
/// an aggregate call. The composite expressions carry [`Aggregate`] instead, keep
/// their grouping guarantees, and cannot lose a clause silently: none can be
/// attached in the first place.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not an aggregate call, so `DISTINCT` and `FILTER (WHERE ...)` do not apply to it",
    label = "the expression collapses a group, but does not end with the aggregate call itself",
    note = "attach `distinct` or `filter_where` to the aggregate function call, before combining it with arithmetic or a scalar function"
)]
pub trait AcceptsAggregateClause {}

impl AcceptsAggregateClause for DirectAggregate {}

impl AcceptsAggregateClause for FilteredAggregate {}

impl<T, Scope, Marker, D> Expr<T, Scope, Marker, D>
where
    Marker: AcceptsAggregateClause,
{
    /// Marks the aggregate expression to evaluate only over `DISTINCT` values.
    ///
    /// The modifier belongs to the aggregate call itself, so the method exists on
    /// [`DirectAggregate`] and [`FilteredAggregate`] only: an expression combining
    /// the call with arithmetic or a scalar function carries [`Aggregate`] and
    /// offers no `distinct`, which keeps the modifier from being written where SQL
    /// has no place for it.
    pub fn distinct(mut self) -> Self {
        // The marker gate restricts the method to a call, so the tree is an
        // aggregate; matching it keeps the modifier on the call instead of
        // rewriting the tree.
        if let ExprKind::Aggregate {
            ref mut distinct, ..
        } = self.kind
        {
            *distinct = true;
        }
        self
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
    /// [`DirectAggregate`] and [`FilteredAggregate`] only, and the produced
    /// expression carries [`FilteredAggregate`], so the projection and filtering
    /// rules of a grouped query keep accepting it while arithmetic combinations
    /// with constants preserve the marker.
    ///
    /// ```
    /// use rurqe_core::expr::{sum, Expr, ExprKind};
    ///
    /// rurqe_core::table! {
    ///     Order => "orders" {
    ///         OrderTotal => "total": f64,
    ///     }
    /// }
    ///
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
    /// rurqe_core::table! {
    ///     Order => "orders" {
    ///         OrderTotal => "total": f64,
    ///     }
    /// }
    ///
    /// // Fails to compile: the arithmetic combination is an `Aggregate`, not the
    /// // aggregate call itself, so it takes no `FILTER` clause.
    /// let _ = (sum(OrderTotal) + 1.0)
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
        let ExprKind::Aggregate {
            func,
            distinct,
            arg,
            filter,
        } = self.kind
        else {
            // Only an aggregate call accepts the clause, so the tree is an
            // aggregate here; the branch keeps the method total — and the
            // invariant panic-free — without ever being reached.
            return Expr::new(self.kind);
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
