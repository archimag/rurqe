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
//! Restricting an aggregate call with a `FILTER (WHERE ...)` clause switches it
//! to the [`FilteredAggregate`] marker, which keeps the clauses reachable so
//! repeated calls accumulate their conditions. The marker describes the call it
//! restricts and nothing built over it: the filter stays recorded in the tree of
//! the call, so a composite expression over a restricted call degrades to the
//! general marker exactly like one over a bare call.

use super::{Aggregate, Expr};
use crate::expr::ast::{ExprKind, MergeDialect, MergedDialect};
use crate::expr::scope::{MergeScope, Merged};
use crate::expr::val::AggregateFunc;
use crate::schema::Column;

/// Text rendered in place of a `FILTER (WHERE ...)` clause attached to an
/// expression that is not an aggregate call.
///
/// The clause belongs to an aggregate call, so the situation is reachable only
/// through a tree whose aggregation marker contradicts its root — a tree only
/// this crate can assemble, because `Expr::new` is crate-private. The
/// placeholder keeps that internal fault visible in every build profile, so a
/// release build never silently drops the predicate the return type keeps
/// promising.
const DROPPED_FILTER_PLACEHOLDER: &str =
    "/* invalid aggregate marker: FILTER predicate dropped */";

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

/// Marker of an aggregate call restricted by a `FILTER (WHERE ...)` clause.
///
/// The marker describes the call it restricts: the call collapses the rows of its
/// group while skipping the rows its filter excludes, and repeated
/// [`filter_where`](Expr::filter_where) calls accumulate their conditions on it.
/// It never survives a composition: the clause belongs to the call itself, so an
/// expression combining a restricted call — an arithmetic combination, a scalar
/// function, a `CASE` branch — degrades to the general [`Aggregate`] marker,
/// which keeps collapsing the group while offering no clause of its own.
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
        debug_assert!(
            matches!(self.kind, ExprKind::Aggregate { .. }),
            "the aggregation marker names an aggregate call, but the tree is not one"
        );
        // The marker gate restricts the method to a call, so the tree is an
        // aggregate; matching it keeps the modifier on the call instead of
        // rewriting the tree. The assertion keeps a forged marker loud in
        // debug builds instead of letting the branch silently mutate nothing
        // while the return type still advertises the modifier.
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
    /// rules of a grouped query keep accepting it and repeated calls keep
    /// accumulating their conditions on the same call.
    ///
    /// The method never discards the clause it is given. An expression whose
    /// marker names an aggregate call while its tree holds none — a state only
    /// this crate can assemble, because `Expr::new` is crate-private — is
    /// reported in every build profile rather than in a debug build alone: the
    /// trace records the contradiction and the returned tree carries a
    /// diagnostic placeholder where the `FILTER` clause would have been, so the
    /// rendered statement shows the discarded predicate instead of hiding it.
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
    ///
    /// A restricted call degrades exactly like a bare one, because the filter
    /// recorded in its tree belongs to the call and to nothing built over it:
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
        // The marker gate restricts the method to an aggregate call, so the tree
        // is one whenever the type system was used as intended. The check is a
        // total branch rather than an assertion, because an assertion only fires
        // in a debug build: a release build would silently discard the clause
        // the return type keeps promising, which the panic-free invariant of the
        // crate forbids just as much as unwinding would.
        let ExprKind::Aggregate {
            func,
            distinct,
            arg,
            filter,
        } = self.kind
        else {
            // Only an aggregate call accepts the clause, so this branch is
            // unreachable through the public API: `Expr::new` is crate-private
            // and every aggregate constructor roots its tree in the call. It
            // still keeps the method total — and the crate panic-free — while
            // reporting the contradiction in both build profiles: the trace
            // records it, and the returned tree carries a diagnostic placeholder
            // where the `FILTER` clause would have been, so the rendered
            // statement shows the discarded predicate instead of hiding it.
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
