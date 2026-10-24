//! Ordering, pagination, and the clause-by-clause recording of both.

use crate::dialect::{Dialect, SupportsNullsOrdering};
use crate::expr::{ExprKind, NullsOrder, OrderCriterion, OrderDirection};
use crate::schema::{At, Column, TableInScope};

use super::SelectQuery;

/// A column, or a tuple of columns, an `ORDER BY` clause sorts by.
///
/// A single column marker orders by that column, while a tuple orders by its
/// columns in declaration order — which is what a keyset cursor needs, because it
/// compares the leading column first and breaks the ties by the columns after it:
///
/// ```text
/// ORDER BY "created_at" DESC, "id" DESC
/// ```
///
/// Every ordered column has to belong to the query scope, so ordering by a column
/// of a table the query does not select from is rejected at the call site:
///
/// ```compile_fail
/// use rurqe_core::dialect::Dialect;
/// use rurqe_core::query::SelectQuery;
///
/// struct MockDialect;
///
/// impl Dialect for MockDialect {
///     fn push_quoted_identifier(out: &mut String, ident: &str) {
///         out.push_str(ident);
///     }
///
///     fn push_placeholder(out: &mut String, index: usize) {
///         let _ = index;
///     }
/// }
///
/// rurqe_core::table! {
///     User => "users" {
///         UserId => "id": i64,
///     }
/// }
///
/// rurqe_core::table! {
///     Order => "orders" {
///         OrderId => "id": i64,
///     }
/// }
///
/// fn main() {
///     // Fails to compile: `OrderId` belongs to `Order`, which the query does not
///     // select from.
///     let _ = SelectQuery::<MockDialect>::new()
///         .from::<User>()
///         .select((UserId,))
///         .order_by_asc((OrderId,));
/// }
/// ```
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a valid ORDER BY clause",
    label = "order by a column or a tuple of columns",
    note = "ORDER BY accepts column markers or tuples of column markers, such as `UserId` or `(OrderCreatedAt, OrderId)`"
)]
pub trait OrderBy<Tables, Positions = At<0>> {
    /// Appends the ordered column expressions to `out` in declaration order.
    fn push_orderings(&self, out: &mut Vec<ExprKind>);
}

impl<Tables, C, P> OrderBy<Tables, P> for C
where
    C: Column,
    Tables: TableInScope<C::Table, P>,
{
    fn push_orderings(&self, out: &mut Vec<ExprKind>) {
        out.push(C::expr_kind());
    }
}

macro_rules! impl_order_by {
    ($($column:ident $position:ident),+) => {
        impl<Tables, $($column, $position),+> OrderBy<Tables, ($($position,)+)>
            for ($($column,)+)
        where
            $($column: Column, Tables: TableInScope<$column::Table, $position>,)+
        {
            fn push_orderings(&self, out: &mut Vec<ExprKind>) {
                $(out.push($column::expr_kind());)+
            }
        }
    };
}

impl_order_by!(C1 P1);
impl_order_by!(C1 P1, C2 P2);
impl_order_by!(C1 P1, C2 P2, C3 P3);
impl_order_by!(C1 P1, C2 P2, C3 P3, C4 P4);
impl_order_by!(C1 P1, C2 P2, C3 P3, C4 P4, C5 P5);
impl_order_by!(C1 P1, C2 P2, C3 P3, C4 P4, C5 P5, C6 P6);
impl_order_by!(C1 P1, C2 P2, C3 P3, C4 P4, C5 P5, C6 P6, C7 P7);
impl_order_by!(C1 P1, C2 P2, C3 P3, C4 P4, C5 P5, C6 P6, C7 P7, C8 P8);

impl<D: Dialect, Tables, Output, Projected, Lock> SelectQuery<D, Tables, Output, Projected, Lock> {
    /// Appends an ordering criterion for `columns` with `direction`.
    ///
    /// `columns` is a column marker or a tuple of them, so a keyset cursor is
    /// ordered as one clause — `.order_by_desc((OrderCreatedAt, OrderId))` sorts by
    /// the timestamp first and breaks the ties by the identifier:
    ///
    /// ```text
    /// ORDER BY "created_at" DESC, "id" DESC
    /// ```
    ///
    /// An ordering written before a set operation belongs to the left query and
    /// is rendered inside its parentheses; an ordering written after one applies
    /// to the combined rows.
    pub fn order_by<O, P>(mut self, columns: O, direction: OrderDirection) -> Self
    where
        O: OrderBy<Tables, P>,
    {
        let mut expressions = Vec::new();
        columns.push_orderings(&mut expressions);
        for expression in expressions {
            self.push_ordering(OrderCriterion::new(expression, direction));
        }
        self
    }

    /// Appends an ordering criterion fixing where the NULL values of `columns` sort.
    pub fn order_by_nulls<O, P>(
        mut self,
        columns: O,
        direction: OrderDirection,
        nulls: NullsOrder,
    ) -> Self
    where
        D: SupportsNullsOrdering,
        O: OrderBy<Tables, P>,
    {
        let mut expressions = Vec::new();
        columns.push_orderings(&mut expressions);
        for expression in expressions {
            self.push_ordering(OrderCriterion::new(expression, direction).nulls(nulls));
        }
        self
    }

    /// Appends an ascending ordering criterion for `columns`.
    pub fn order_by_asc<O, P>(self, columns: O) -> Self
    where
        O: OrderBy<Tables, P>,
    {
        self.order_by(columns, OrderDirection::Asc)
    }

    /// Appends a descending ordering criterion for `columns`.
    pub fn order_by_desc<O, P>(self, columns: O) -> Self
    where
        O: OrderBy<Tables, P>,
    {
        self.order_by(columns, OrderDirection::Desc)
    }

    /// Appends an ascending ordering criterion putting NULL values first.
    pub fn order_by_asc_nulls_first<O, P>(self, columns: O) -> Self
    where
        D: SupportsNullsOrdering,
        O: OrderBy<Tables, P>,
    {
        self.order_by_nulls(columns, OrderDirection::Asc, NullsOrder::First)
    }

    /// Appends an ascending ordering criterion putting NULL values last.
    pub fn order_by_asc_nulls_last<O, P>(self, columns: O) -> Self
    where
        D: SupportsNullsOrdering,
        O: OrderBy<Tables, P>,
    {
        self.order_by_nulls(columns, OrderDirection::Asc, NullsOrder::Last)
    }

    /// Appends a descending ordering criterion putting NULL values first.
    pub fn order_by_desc_nulls_first<O, P>(self, columns: O) -> Self
    where
        D: SupportsNullsOrdering,
        O: OrderBy<Tables, P>,
    {
        self.order_by_nulls(columns, OrderDirection::Desc, NullsOrder::First)
    }

    /// Appends a descending ordering criterion putting NULL values last.
    pub fn order_by_desc_nulls_last<O, P>(self, columns: O) -> Self
    where
        D: SupportsNullsOrdering,
        O: OrderBy<Tables, P>,
    {
        self.order_by_nulls(columns, OrderDirection::Desc, NullsOrder::Last)
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
