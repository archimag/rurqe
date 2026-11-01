//! Ordering of the rows a set operation combined.
//!
//! A statement combined with `UNION`, `INTERSECT`, or `EXCEPT` sorts the rows its
//! projection list produced rather than the rows of a relation in scope, so its
//! `ORDER BY` clause names an output column or a 1-based position of the projection
//! list. The builders live here alone: they are reachable from the two tail states
//! a compound statement passes through, and every call moves the query into
//! `TailClosed`, where no further operand can be combined with it.

use crate::dialect::{Dialect, SupportsNullsOrdering};
use crate::expr::{ExprKind, NullsOrder, OrderCriterion, OrderDirection};
use crate::query::scope::Compound;
use crate::query::select::{HasProjection, SelectQuery, SetOpApplied, TailClosed};
use crate::schema::{Column, Table};

/// Builds the `ORDER BY` node naming the 1-based position `index` of a compound result.
///
/// A compound statement sorts the rows its projections produced, so the criterion
/// names an output column rather than a column of the statements it combines. The
/// position renders as a bare number, and the pre-flight validation of the
/// assembled statement checks it against the projection list.
fn compound_order_position(index: u32) -> ExprKind {
    ExprKind::Raw {
        fragments: vec![index.to_string()].into(),
        params: Vec::new(),
    }
}

/// Builds the `ORDER BY` node naming the output column `C` of a compound result.
///
/// The node carries the name of `C` without the qualifier its own marker renders:
/// a compound statement sorts the columns its projection list produced, so a
/// table-qualified reference would name a relation the combined statement no
/// longer declares and is rejected by the engine.
fn compound_order_column<C: Column>() -> ExprKind {
    ExprKind::Column {
        table: <C::Table as Table>::NAME,
        name: C::NAME,
    }
}

/// Implements the ordering builders of a compound statement for one tail state.
///
/// The builders are reachable while the ordering of the combined rows is still
/// being written: from [`SetOpApplied`], where the last set operation left the
/// query, and from [`TailClosed`], where the previous criterion left it. Every call
/// moves the query into [`TailClosed`], so a criterion can never be written after
/// another operand was combined with the statement.
macro_rules! impl_compound_ordering {
    ($lock:ty) => {
        impl<D: Dialect, Output> SelectQuery<D, Compound, Output, HasProjection, $lock> {
            /// Orders the combined rows by the 1-based position `index` of the
            /// projection list.
            ///
            /// ```text
            /// SELECT "id", "name" FROM "users" UNION SELECT "id", "name" FROM "archive"
            ///     ORDER BY 2 DESC
            /// ```
            ///
            /// The position is checked against the projection list when the
            /// statement is built, so an out-of-range index is reported as
            /// [`BuildError::CompoundOrderIndex`](crate::query::BuildError::CompoundOrderIndex)
            /// instead of reaching the engine.
            pub fn order_by_index(
                mut self,
                index: u32,
                direction: OrderDirection,
            ) -> SelectQuery<D, Compound, Output, HasProjection, TailClosed> {
                self.push_ordering(OrderCriterion::new(
                    compound_order_position(index),
                    direction,
                ));
                self.tail_closed()
            }

            /// Orders the combined rows by the 1-based position `index`, ascending.
            pub fn order_by_index_asc(
                self,
                index: u32,
            ) -> SelectQuery<D, Compound, Output, HasProjection, TailClosed> {
                self.order_by_index(index, OrderDirection::Asc)
            }

            /// Orders the combined rows by the 1-based position `index`, descending.
            pub fn order_by_index_desc(
                self,
                index: u32,
            ) -> SelectQuery<D, Compound, Output, HasProjection, TailClosed> {
                self.order_by_index(index, OrderDirection::Desc)
            }

            /// Orders the combined rows by their `column` output column.
            ///
            /// ```text
            /// SELECT "id", "total" FROM "orders" UNION SELECT "id", "total" FROM "archive"
            ///     ORDER BY "total" ASC
            /// ```
            ///
            /// The column is rendered by its bare name, and the pre-flight
            /// validation of the assembled statement checks that the projection list
            /// produces it — reported as
            /// [`BuildError::CompoundOrderColumn`](crate::query::BuildError::CompoundOrderColumn)
            /// otherwise. An expression projected without an alias produces no output
            /// name, so such a column is ordered by its position instead.
            pub fn order_by_output<C: Column>(
                mut self,
                _column: C,
                direction: OrderDirection,
            ) -> SelectQuery<D, Compound, Output, HasProjection, TailClosed> {
                self.push_ordering(OrderCriterion::new(compound_order_column::<C>(), direction));
                self.tail_closed()
            }

            /// Orders the combined rows by their `column` output column, ascending.
            pub fn order_by_output_asc<C: Column>(
                self,
                column: C,
            ) -> SelectQuery<D, Compound, Output, HasProjection, TailClosed> {
                self.order_by_output(column, OrderDirection::Asc)
            }

            /// Orders the combined rows by their `column` output column, descending.
            pub fn order_by_output_desc<C: Column>(
                self,
                column: C,
            ) -> SelectQuery<D, Compound, Output, HasProjection, TailClosed> {
                self.order_by_output(column, OrderDirection::Desc)
            }

            /// Orders the combined rows by their `column` output column, fixing where
            /// its NULL values sort.
            pub fn order_by_output_nulls<C: Column>(
                mut self,
                _column: C,
                direction: OrderDirection,
                nulls: NullsOrder,
            ) -> SelectQuery<D, Compound, Output, HasProjection, TailClosed>
            where
                D: SupportsNullsOrdering,
            {
                self.push_ordering(
                    OrderCriterion::new(compound_order_column::<C>(), direction).nulls(nulls),
                );
                self.tail_closed()
            }

            /// Orders the combined rows by their `column` output column, ascending,
            /// putting NULL values first.
            pub fn order_by_output_asc_nulls_first<C: Column>(
                self,
                column: C,
            ) -> SelectQuery<D, Compound, Output, HasProjection, TailClosed>
            where
                D: SupportsNullsOrdering,
            {
                self.order_by_output_nulls(column, OrderDirection::Asc, NullsOrder::First)
            }

            /// Orders the combined rows by their `column` output column, ascending,
            /// putting NULL values last.
            pub fn order_by_output_asc_nulls_last<C: Column>(
                self,
                column: C,
            ) -> SelectQuery<D, Compound, Output, HasProjection, TailClosed>
            where
                D: SupportsNullsOrdering,
            {
                self.order_by_output_nulls(column, OrderDirection::Asc, NullsOrder::Last)
            }

            /// Orders the combined rows by their `column` output column, descending,
            /// putting NULL values first.
            pub fn order_by_output_desc_nulls_first<C: Column>(
                self,
                column: C,
            ) -> SelectQuery<D, Compound, Output, HasProjection, TailClosed>
            where
                D: SupportsNullsOrdering,
            {
                self.order_by_output_nulls(column, OrderDirection::Desc, NullsOrder::First)
            }

            /// Orders the combined rows by their `column` output column, descending,
            /// putting NULL values last.
            pub fn order_by_output_desc_nulls_last<C: Column>(
                self,
                column: C,
            ) -> SelectQuery<D, Compound, Output, HasProjection, TailClosed>
            where
                D: SupportsNullsOrdering,
            {
                self.order_by_output_nulls(column, OrderDirection::Desc, NullsOrder::Last)
            }
        }
    };
}

impl_compound_ordering!(SetOpApplied);
impl_compound_ordering!(TailClosed);
