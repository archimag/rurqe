//! Aggregate construction and the markers the grouped-query rules rely on.
//!
//! An aggregate collapses the rows of a group into a single value, which is why
//! the produced expression carries the [`Aggregate`] marker instead of the
//! default `NonAggregate` one. Restricting an aggregate with a `FILTER (WHERE ...)`
//! clause switches it to the [`FilteredAggregate`] marker, so the projection and
//! filtering rules of a grouped query keep accepting it while its tree records
//! which rows it reads.

use super::{Aggregate, Expr};
use crate::expr::ast::{ExprKind, MergeDialect, MergedDialect};
use crate::expr::scope::{MergeScope, Merged};
use crate::expr::val::AggregateFunc;
use crate::schema::Column;

impl<T> Expr<T, ()> {
    /// Builds a `SUM(C)` expression for column `C`.
    ///
    /// The produced expression carries the [`Aggregate`] marker, so it stays
    /// projectable from a grouped query even when `C` is not grouped. It also
    /// references the table of `C`, so a statement whose scope lacks that table
    /// rejects it like any other column reference.
    pub fn sum<C>() -> Expr<T, C::Table, Aggregate>
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
    pub fn sum_distinct<C>() -> Expr<T, C::Table, Aggregate>
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
    /// The produced expression carries the [`Aggregate`] marker, so it stays
    /// projectable from a grouped query even when `C` is not grouped. It also
    /// references the table of `C`, so a statement whose scope lacks that table
    /// rejects it like any other column reference.
    pub fn min<C>() -> Expr<T, C::Table, Aggregate>
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
    /// The produced expression carries the [`Aggregate`] marker, so it stays
    /// projectable from a grouped query even when `C` is not grouped. It also
    /// references the table of `C`, so a statement whose scope lacks that table
    /// rejects it like any other column reference.
    pub fn max<C>() -> Expr<T, C::Table, Aggregate>
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
/// The marker is implemented by [`Aggregate`] and [`FilteredAggregate`], so the
/// projection and filtering rules of a grouped query accept both kinds of
/// aggregate.
#[doc(hidden)]
pub trait AggregateMarker {}

impl AggregateMarker for Aggregate {}

impl AggregateMarker for FilteredAggregate {}

impl<T, Scope, Marker, D> Expr<T, Scope, Marker, D>
where
    Marker: AggregateMarker,
{
    /// Marks the aggregate expression to evaluate only over `DISTINCT` values.
    pub fn distinct(mut self) -> Self {
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
    /// The produced expression carries [`FilteredAggregate`], so the projection
    /// and filtering rules of a grouped query keep accepting it while arithmetic
    /// combinations with constants preserve the marker.
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
