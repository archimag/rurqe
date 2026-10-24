//! Ordering, pagination, and the clause-by-clause recording of both.

use crate::dialect::{Dialect, SupportsNullsOrdering};
use crate::expr::{NullsOrder, OrderCriterion, OrderDirection};
use crate::schema::{Column, TableInScope};

use super::SelectQuery;

impl<D: Dialect, Tables, Output, Projected, Lock> SelectQuery<D, Tables, Output, Projected, Lock> {
    /// Appends an ordering criterion for column `col` with `direction`.
    ///
    /// An ordering written before a set operation belongs to the left query and
    /// is rendered inside its parentheses; an ordering written after one applies
    /// to the combined rows.
    pub fn order_by<C: Column, P>(mut self, _col: C, direction: OrderDirection) -> Self
    where
        Tables: TableInScope<C::Table, P>,
    {
        self.push_ordering(OrderCriterion::new(C::expr_kind(), direction));
        self
    }

    /// Appends an ordering criterion fixing where the NULL values of `col` sort.
    pub fn order_by_nulls<C: Column, P>(
        mut self,
        _col: C,
        direction: OrderDirection,
        nulls: NullsOrder,
    ) -> Self
    where
        D: SupportsNullsOrdering,
        Tables: TableInScope<C::Table, P>,
    {
        self.push_ordering(OrderCriterion::new(C::expr_kind(), direction).nulls(nulls));
        self
    }

    /// Appends an ascending ordering criterion for column `col`.
    pub fn order_by_asc<C: Column, P>(self, col: C) -> Self
    where
        Tables: TableInScope<C::Table, P>,
    {
        self.order_by(col, OrderDirection::Asc)
    }

    /// Appends a descending ordering criterion for column `col`.
    pub fn order_by_desc<C: Column, P>(self, col: C) -> Self
    where
        Tables: TableInScope<C::Table, P>,
    {
        self.order_by(col, OrderDirection::Desc)
    }

    /// Appends an ascending ordering criterion putting NULL values first.
    pub fn order_by_asc_nulls_first<C: Column, P>(self, col: C) -> Self
    where
        D: SupportsNullsOrdering,
        Tables: TableInScope<C::Table, P>,
    {
        self.order_by_nulls(col, OrderDirection::Asc, NullsOrder::First)
    }

    /// Appends an ascending ordering criterion putting NULL values last.
    pub fn order_by_asc_nulls_last<C: Column, P>(self, col: C) -> Self
    where
        D: SupportsNullsOrdering,
        Tables: TableInScope<C::Table, P>,
    {
        self.order_by_nulls(col, OrderDirection::Asc, NullsOrder::Last)
    }

    /// Appends a descending ordering criterion putting NULL values first.
    pub fn order_by_desc_nulls_first<C: Column, P>(self, col: C) -> Self
    where
        D: SupportsNullsOrdering,
        Tables: TableInScope<C::Table, P>,
    {
        self.order_by_nulls(col, OrderDirection::Desc, NullsOrder::First)
    }

    /// Appends a descending ordering criterion putting NULL values last.
    pub fn order_by_desc_nulls_last<C: Column, P>(self, col: C) -> Self
    where
        D: SupportsNullsOrdering,
        Tables: TableInScope<C::Table, P>,
    {
        self.order_by_nulls(col, OrderDirection::Desc, NullsOrder::Last)
    }

    /// Records `criterion` on the clause the query currently orders with.
    ///
    /// An ordering written after a set operation belongs to the combined rows
    /// rather than to the left query, so it is recorded on the compound statement
    /// and rendered against the output column names of the projections.
    pub(crate) fn push_ordering(&mut self, criterion: OrderCriterion) {
        if self.set_operations.is_empty() {
            self.order_by.push(criterion);
        } else {
            self.compound_order_by.push(criterion);
        }
    }

    /// Limits the number of returned rows to `limit`.
    ///
    /// A limit written before a set operation belongs to the left query; a limit
    /// written after one applies to the combined rows.
    pub fn limit(mut self, limit: u32) -> Self {
        if self.set_operations.is_empty() {
            self.limit = Some(limit);
        } else {
            self.compound_limit = Some(limit);
        }
        self
    }

    /// Skips the first `offset` rows of the result.
    ///
    /// An offset written before a set operation belongs to the left query; an
    /// offset written after one applies to the combined rows.
    pub fn offset(mut self, offset: u32) -> Self {
        if self.set_operations.is_empty() {
            self.offset = Some(offset);
        } else {
            self.compound_offset = Some(offset);
        }
        self
    }
}
