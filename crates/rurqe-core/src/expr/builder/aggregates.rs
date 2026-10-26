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
//! [`Aggregate`](crate::expr::Aggregate) marker through
//! [`CombineAggr`](crate::expr::CombineAggr). It keeps collapsing its group and
//! therefore stays projectable from a grouped query, but its root node is the
//! operator combining the calls, so SQL does not allow `DISTINCT` or `FILTER` on
//! it and neither method exists for it. Restricting an aggregate call with a
//! `FILTER (WHERE ...)` clause switches it to the [`FilteredAggregate`] marker,
//! which keeps the clauses reachable so repeated calls accumulate their
//! conditions. The marker describes the call it restricts and nothing built over
//! it: the filter stays recorded in the tree of the call, so a composite
//! expression over a restricted call degrades to the general marker exactly like
//! one over a bare call.

mod clauses;
mod markers;

pub use self::markers::{
    AcceptsAggregateClause, AggregateMarker, DirectAggregate, FilteredAggregate,
};

use super::Expr;
use crate::expr::ast::ExprKind;
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

#[cfg(test)]
mod tests;
