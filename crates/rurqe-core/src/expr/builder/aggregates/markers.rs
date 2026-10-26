//! The markers naming what an expression aggregates.
//!
//! The markers are zero-sized and carry no data: they exist to gate the
//! `DISTINCT` and `FILTER (WHERE ...)` clauses of an aggregate call, which live
//! in the sibling `clauses` module.

use crate::expr::Aggregate;

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
/// [`filter_where`](crate::expr::Expr::filter_where) calls accumulate their
/// conditions on it. It never survives a composition: the clause belongs to the
/// call itself, so an expression combining a restricted call — an arithmetic
/// combination, a scalar function, a `CASE` branch — degrades to the general
/// [`Aggregate`] marker, which keeps collapsing the group while offering no
/// clause of its own.
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
/// [`Expr::distinct`](crate::expr::Expr::distinct) and
/// [`Expr::filter_where`](crate::expr::Expr::filter_where), so a clause is
/// reachable only on an aggregate call. The composite expressions carry
/// [`Aggregate`] instead, keep their grouping guarantees, and cannot lose a clause
/// silently: none can be attached in the first place.
#[diagnostic::on_unimplemented(
    message = "`DISTINCT` and `FILTER (WHERE ...)` restrict an aggregate function call, and `{Self}` is not one",
    label = "the expression collapses a group, but its root is no aggregate call",
    note = "write `distinct` or `filter_where` on the aggregate call itself — `SUM(\"total\").filter_where(...)` — before combining it with arithmetic or a scalar function"
)]
pub trait AcceptsAggregateClause {}

impl AcceptsAggregateClause for DirectAggregate {}

impl AcceptsAggregateClause for FilteredAggregate {}
