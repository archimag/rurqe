//! Ordering, pagination, and the clause-by-clause recording of both.
//!
//! The module is decomposed by responsibility: what an ordering may name — a
//! column of a relation in scope, a computed expression over it, or a window
//! expression evaluated with an `OVER (...)` clause — is declared in the `traits`
//! submodule, the ordering of the rows a set operation combined is built in the
//! `compound` submodule, and the plain ordering and pagination builders of a
//! statement stay here, where every criterion is recorded on the clause it belongs
//! to.

use crate::dialect::{Dialect, SupportsNullsOrdering};
use crate::expr::{NullsOrder, OrderCriterion, OrderDirection};

use super::SelectQuery;

mod compound;
mod traits;

pub use self::traits::{OrderBy, OrderExprTarget};

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

    /// Appends an ordering criterion for `expression` with `direction`.
    ///
    /// The criterion renders the expression instead of a bare column, so a
    /// computed value — an arithmetic expression over the columns of the query
    /// scope, a `CASE` expression, or a window expression — sorts the rows it
    /// evaluates:
    ///
    /// ```text
    /// ORDER BY "total" * "quantity" DESC
    /// ```
    ///
    /// The criterion carries a per-row expression, so the statement sorts the
    /// rows it produced without grouping them first: an aggregate collapses the
    /// rows of a group and orders a grouped statement through
    /// [`GroupedQuery::order_by_expr`](crate::query::GroupedQuery::order_by_expr)
    /// instead, which is why passing one here is rejected at the call site.
    ///
    /// The expression is checked against the query scope like a projection,
    /// so an ordering that references a table the query does not select from
    /// is rejected at the call site:
    ///
    /// ```
    /// use rurqe_core::dialect::Dialect;
    /// use rurqe_core::expr::Expr;
    /// use rurqe_core::query::SelectQuery;
    ///
    /// struct MockDialect;
    ///
    /// impl Dialect for MockDialect {
    ///     fn push_quoted_identifier(out: &mut String, ident: &str) {
    ///         out.push('"');
    ///         out.push_str(ident);
    ///         out.push('"');
    ///     }
    ///
    ///     fn push_placeholder(out: &mut String, index: usize) {
    ///         let _ = index;
    ///     }
    /// }
    ///
    /// rurqe_core::table! {
    ///     Order => "orders" {
    ///         OrderUserId => "user_id": i64,
    ///         OrderTotal => "total": f64,
    ///     }
    /// }
    ///
    /// fn main() {
    ///     let built = SelectQuery::<MockDialect>::new()
    ///         .from::<Order>()
    ///         .select((OrderUserId,))
    ///         .order_by_expr_desc(Expr::column::<OrderTotal>())
    ///         .build()
    ///         .expect("valid query");
    ///
    ///     assert_eq!(
    ///         built.sql,
    ///         "SELECT \"user_id\" FROM \"orders\" ORDER BY \"total\" DESC"
    ///     );
    /// }
    /// ```
    ///
    /// A window expression orders the statement through the same builder,
    /// keeping its `OVER (...)` specification inside the criterion:
    ///
    /// ```
    /// use rurqe_core::dialect::{Dialect, SupportsWindowFunctions};
    /// use rurqe_core::query::{SelectQuery, Window, row_number};
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
    /// impl SupportsWindowFunctions for MockDialect {}
    ///
    /// rurqe_core::table! {
    ///     Order => "orders" {
    ///         OrderUserId => "user_id": i64,
    ///     }
    /// }
    ///
    /// fn main() {
    ///     let built = SelectQuery::<MockDialect>::new()
    ///         .from::<Order>()
    ///         .select((OrderUserId,))
    ///         .order_by_expr_desc(row_number().over(Window::partition_by(OrderUserId)))
    ///         .build()
    ///         .expect("valid query");
    ///
    ///     assert!(
    ///         built.sql.contains("OVER"),
    ///         "window specification rendered: {}",
    ///         built.sql
    ///     );
    /// }
    /// ```
    pub fn order_by_expr<E, P>(mut self, expression: E, direction: OrderDirection) -> Self
    where
        E: OrderExprTarget<Tables, P, D>,
    {
        let mut expressions = Vec::new();
        expression.into_order_exprs(&mut expressions);
        for expression in expressions {
            self.push_ordering(OrderCriterion::new(expression, direction));
        }
        self
    }

    /// Appends an ascending ordering criterion for `expression`.
    pub fn order_by_expr_asc<E, P>(self, expression: E) -> Self
    where
        E: OrderExprTarget<Tables, P, D>,
    {
        self.order_by_expr(expression, OrderDirection::Asc)
    }

    /// Appends a descending ordering criterion for `expression`.
    pub fn order_by_expr_desc<E, P>(self, expression: E) -> Self
    where
        E: OrderExprTarget<Tables, P, D>,
    {
        self.order_by_expr(expression, OrderDirection::Desc)
    }

    /// Appends an ordering criterion for `expression` fixing where its NULL
    /// values sort.
    pub fn order_by_expr_nulls<E, P>(
        mut self,
        expression: E,
        direction: OrderDirection,
        nulls: NullsOrder,
    ) -> Self
    where
        D: SupportsNullsOrdering,
        E: OrderExprTarget<Tables, P, D>,
    {
        let mut expressions = Vec::new();
        expression.into_order_exprs(&mut expressions);
        for expression in expressions {
            self.push_ordering(OrderCriterion::new(expression, direction).nulls(nulls));
        }
        self
    }

    /// Appends an ascending ordering criterion for `expression` putting NULL
    /// values first.
    pub fn order_by_expr_asc_nulls_first<E, P>(self, expression: E) -> Self
    where
        D: SupportsNullsOrdering,
        E: OrderExprTarget<Tables, P, D>,
    {
        self.order_by_expr_nulls(expression, OrderDirection::Asc, NullsOrder::First)
    }

    /// Appends an ascending ordering criterion for `expression` putting NULL
    /// values last.
    pub fn order_by_expr_asc_nulls_last<E, P>(self, expression: E) -> Self
    where
        D: SupportsNullsOrdering,
        E: OrderExprTarget<Tables, P, D>,
    {
        self.order_by_expr_nulls(expression, OrderDirection::Asc, NullsOrder::Last)
    }

    /// Appends a descending ordering criterion for `expression` putting NULL
    /// values first.
    pub fn order_by_expr_desc_nulls_first<E, P>(self, expression: E) -> Self
    where
        D: SupportsNullsOrdering,
        E: OrderExprTarget<Tables, P, D>,
    {
        self.order_by_expr_nulls(expression, OrderDirection::Desc, NullsOrder::First)
    }

    /// Appends a descending ordering criterion for `expression` putting NULL
    /// values last.
    pub fn order_by_expr_desc_nulls_last<E, P>(self, expression: E) -> Self
    where
        D: SupportsNullsOrdering,
        E: OrderExprTarget<Tables, P, D>,
    {
        self.order_by_expr_nulls(expression, OrderDirection::Desc, NullsOrder::Last)
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
