//! The type-safe `SELECT` builder and its single-pass SQL renderer.

use core::fmt::Write;
use core::marker::PhantomData;
use std::sync::Arc;

use crate::dialect::{Dialect, SupportsDistinctOn, SupportsLateralJoin};
use crate::expr::{render_expr, BinaryOp, Expr, ExprKind, SqlVal, Subquery};
use crate::schema::{Column, ExprInScope, Table, TableInScope, TableRef};

use super::cte::{Cte, CteRenderer, Lateral};
use super::group_by::{EmptyGroupSet, GroupBy, GroupByColumns, GroupByInScope, GroupedQuery};
use super::scope::{
    CrossLateralJoined, JoinKind, Joined, LeftJoined, LeftLateralJoined, ScopePositions,
    TableScope,
};
use super::selection::{Nullable, SelectionFor};
use super::set_ops::{SetOpKind, SetOperation};

/// The rendered SQL text of a query together with its bound parameters.
#[derive(Debug, Clone, PartialEq)]
pub struct BuiltQuery {
    /// The SQL text using the dialect's positional placeholders.
    pub sql: String,

    /// The parameter values referenced by the placeholders, in placeholder order.
    pub params: Vec<SqlVal>,
}

/// The direction of an `ORDER BY` clause.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OrderDirection {
    /// Ascending order (`ASC`).
    Asc,
    /// Descending order (`DESC`).
    Desc,
}

impl OrderDirection {
    /// The SQL text of the direction.
    pub const fn as_str(self) -> &'static str {
        match self {
            OrderDirection::Asc => "ASC",
            OrderDirection::Desc => "DESC",
        }
    }
}

/// Phantom marker carrying the dialect, scope, and row type of a query.
type QueryMarker<D, Tables, Output> = PhantomData<fn() -> (D, Tables, Output)>;

/// A type-erased renderer producing a list of projection items into the SQL buffer.
///
/// The renderer receives the relations of the statement, so a projected column is
/// qualified exactly like any other column reference of the same statement.
pub(crate) type ProjectionRenderer =
    Arc<dyn Fn(&mut String, &mut Vec<SqlVal>, &[TableRef]) + Send + Sync>;

/// A single join clause recorded on a [`SelectQuery`].
#[derive(Clone)]
struct JoinClause {
    kind: JoinKind,
    on: Option<ExprKind>,
    lateral: Option<CteRenderer>,
}

/// Internal definition of a single Common Table Expression (`WITH cte AS (...)`).
#[derive(Clone)]
struct CteDefinition {
    name: &'static str,
    renderer: CteRenderer,
}

/// A `SELECT` query over the table `Tables` producing rows of type `Output`.
pub struct SelectQuery<D: Dialect, Tables = (), Output = ()> {
    ctes: Vec<CteDefinition>,
    projections: Option<ProjectionRenderer>,
    joins: Vec<JoinClause>,
    filter: Option<ExprKind>,
    pub(crate) group_by: Vec<ExprKind>,
    having: Option<ExprKind>,
    order_by: Vec<(ExprKind, OrderDirection)>,
    limit: Option<u32>,
    offset: Option<u32>,
    set_operations: Vec<SetOperation>,
    distinct: bool,
    distinct_on: Vec<ExprKind>,
    _marker: QueryMarker<D, Tables, Output>,
}

impl<D: Dialect, Tables, Output> Clone for SelectQuery<D, Tables, Output> {
    fn clone(&self) -> Self {
        Self {
            ctes: self.ctes.clone(),
            projections: self.projections.clone(),
            joins: self.joins.clone(),
            filter: self.filter.clone(),
            group_by: self.group_by.clone(),
            having: self.having.clone(),
            order_by: self.order_by.clone(),
            limit: self.limit,
            offset: self.offset,
            set_operations: self.set_operations.clone(),
            distinct: self.distinct,
            distinct_on: self.distinct_on.clone(),
            _marker: PhantomData,
        }
    }
}

impl<D: Dialect> SelectQuery<D, (), ()> {
    /// Creates a query without a table, projections, filter, or pagination.
    pub fn new() -> Self {
        SelectQuery {
            ctes: Vec::new(),
            projections: None,
            joins: Vec::new(),
            filter: None,
            group_by: Vec::new(),
            having: None,
            order_by: Vec::new(),
            limit: None,
            offset: None,
            set_operations: Vec::new(),
            distinct: false,
            distinct_on: Vec::new(),
            _marker: PhantomData,
        }
    }
}

impl<D: Dialect> Default for SelectQuery<D, (), ()> {
    fn default() -> Self {
        Self::new()
    }
}

impl<D: Dialect, Output> SelectQuery<D, (), Output> {
    /// Introduces `T` as the table rows are selected from.
    pub fn from<T: Table>(self) -> SelectQuery<D, T, Output> {
        self.retype()
    }

    /// Introduces `table` as the table rows are selected from.
    pub fn from_table<T: Table>(self, _table: T) -> SelectQuery<D, T, Output> {
        self.retype()
    }
}

impl<D: Dialect, Tables> SelectQuery<D, Tables, ()> {
    /// Selects `selection`, fixing the row type of the query to the selection row.
    pub fn select<S, Positions>(
        mut self,
        selection: S,
    ) -> SelectQuery<D, Tables, <S as SelectionFor<Tables, Positions>>::Row>
    where
        S: SelectionFor<Tables, Positions> + Send + Sync + 'static,
    {
        self.projections = Some(Arc::new(move |sql, params, relations| {
            selection.render_projections::<D>(sql, params, relations);
        }));
        self.retype()
    }
}

impl<D: Dialect, Tables, Output> SelectQuery<D, Tables, Output> {
    /// Adds a Common Table Expression (`WITH cte AS (...)`) to this query.
    pub fn with<T, Row>(mut self, cte: Cte<T, Row>) -> Self {
        self.ctes.push(CteDefinition {
            name: cte.name,
            renderer: cte.renderer,
        });
        self
    }

    /// Converts this query into a CTE for the table `T`.
    pub fn as_cte<T: Table>(self) -> Cte<T, Output>
    where
        Tables: TableScope + 'static,
        Output: 'static,
    {
        Cte::new(self)
    }

    /// Converts this query into a named CTE with alias `name`.
    pub fn as_named_cte(self, name: &'static str) -> Cte<(), Output>
    where
        Tables: TableScope + 'static,
        Output: 'static,
    {
        Cte::named(name, self)
    }

    /// Converts this query into a `LATERAL` subquery exposed under the table marker `T`.
    pub fn as_lateral<T: Table>(self) -> Lateral<T, Output>
    where
        Tables: TableScope + 'static,
        Output: 'static,
    {
        Lateral::new(self)
    }

    /// Marks the query as `DISTINCT`, removing duplicate rows.
    pub fn distinct(mut self) -> Self {
        self.distinct = true;
        self
    }

    /// Appends a column to the `DISTINCT ON (...)` clause.
    ///
    /// `DISTINCT ON` is a dialect-specific feature supported by PostgreSQL and
    /// gated by [`SupportsDistinctOn`].
    pub fn distinct_on<C: Column, P>(mut self, _col: C) -> Self
    where
        D: SupportsDistinctOn,
        Tables: TableInScope<C::Table, P>,
    {
        self.distinct_on.push(C::expr_kind());
        self
    }

    /// Lifts the column `C` into its nullable form [`Nullable`].
    pub fn nullable<C: Column>(&self) -> Nullable<C> {
        Nullable::new()
    }

    /// Restricts the query to rows matching `predicate`.
    pub fn where_<Scope, P>(mut self, predicate: Expr<bool, Scope>) -> Self
    where
        Scope: ExprInScope<Tables, P> + ScopePositions<P>,
    {
        let predicate = predicate.into_kind();
        let filter = match self.filter.take() {
            Some(existing) => ExprKind::Binary {
                op: BinaryOp::And,
                lhs: Box::new(existing),
                rhs: Box::new(predicate),
            },
            None => predicate,
        };
        self.filter = Some(filter);
        self
    }

    /// Groups query results by `columns`, moving the query into the
    /// [`GroupedQuery`] state.
    pub fn group_by<G, Positions>(
        mut self,
        columns: G,
    ) -> GroupedQuery<D, Tables, Output, <G as GroupByColumns<EmptyGroupSet>>::Columns>
    where
        G: GroupBy + GroupByInScope<Tables, Positions> + GroupByColumns<EmptyGroupSet>,
    {
        columns.push_column_names(&mut self.group_by);
        GroupedQuery {
            query: self,
            _marker: PhantomData,
        }
    }

    /// Restricts grouped rows to those satisfying `predicate`.
    pub fn having<Scope, P>(mut self, predicate: Expr<bool, Scope>) -> Self
    where
        Scope: ExprInScope<Tables, P> + ScopePositions<P>,
    {
        let predicate = predicate.into_kind();
        let having = match self.having.take() {
            Some(existing) => ExprKind::Binary {
                op: BinaryOp::And,
                lhs: Box::new(existing),
                rhs: Box::new(predicate),
            },
            None => predicate,
        };
        self.having = Some(having);
        self
    }

    /// Appends an ordering criterion for column `col` with `direction`.
    pub fn order_by<C: Column, P>(mut self, _col: C, direction: OrderDirection) -> Self
    where
        Tables: TableInScope<C::Table, P>,
    {
        self.order_by.push((C::expr_kind(), direction));
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

    /// Limits the number of returned rows to `limit`.
    pub fn limit(mut self, limit: u32) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Skips the first `offset` rows of the result.
    pub fn offset(mut self, offset: u32) -> Self {
        self.offset = Some(offset);
        self
    }

    /// Joins the table `T` to the query with an `INNER JOIN` restricted by `on`.
    pub fn inner_join<T: Table, Scope, P>(
        self,
        on: Expr<bool, Scope>,
    ) -> SelectQuery<D, Joined<Tables, T>, Output>
    where
        Tables: TableScope,
        Scope: ExprInScope<Joined<Tables, T>, P> + ScopePositions<P>,
    {
        self.push_join(JoinKind::Inner, Some(on.into_kind()))
            .retype()
    }

    /// Joins `table` to the query with an `INNER JOIN` restricted by `on`.
    pub fn inner_join_table<T: Table, Scope, P>(
        self,
        _table: T,
        on: Expr<bool, Scope>,
    ) -> SelectQuery<D, Joined<Tables, T>, Output>
    where
        Tables: TableScope,
        Scope: ExprInScope<Joined<Tables, T>, P> + ScopePositions<P>,
    {
        self.push_join(JoinKind::Inner, Some(on.into_kind()))
            .retype()
    }

    /// Joins the table `T` to the query with a `LEFT JOIN` restricted by `on`.
    pub fn left_join<T: Table, Scope, P>(
        self,
        on: Expr<bool, Scope>,
    ) -> SelectQuery<D, LeftJoined<Tables, T>, Output>
    where
        Tables: TableScope,
        Scope: ExprInScope<LeftJoined<Tables, T>, P> + ScopePositions<P>,
    {
        self.push_join(JoinKind::Left, Some(on.into_kind()))
            .retype()
    }

    /// Joins `table` to the query with a `LEFT JOIN` restricted by `on`.
    pub fn left_join_table<T: Table, Scope, P>(
        self,
        _table: T,
        on: Expr<bool, Scope>,
    ) -> SelectQuery<D, LeftJoined<Tables, T>, Output>
    where
        Tables: TableScope,
        Scope: ExprInScope<LeftJoined<Tables, T>, P> + ScopePositions<P>,
    {
        self.push_join(JoinKind::Left, Some(on.into_kind()))
            .retype()
    }

    /// Records a regular join clause whose relational effect is expressed by the
    /// caller's return type.
    fn push_join(mut self, kind: JoinKind, on: Option<ExprKind>) -> Self {
        self.joins.push(JoinClause {
            kind,
            on,
            lateral: None,
        });
        self
    }

    fn retype<Tables2, Output2>(self) -> SelectQuery<D, Tables2, Output2> {
        SelectQuery {
            ctes: self.ctes,
            projections: self.projections,
            joins: self.joins,
            filter: self.filter,
            group_by: self.group_by,
            having: self.having,
            order_by: self.order_by,
            limit: self.limit,
            offset: self.offset,
            set_operations: self.set_operations,
            distinct: self.distinct,
            distinct_on: self.distinct_on,
            _marker: PhantomData,
        }
    }
}

impl<D: Dialect + SupportsLateralJoin, Tables: TableScope, Output> SelectQuery<D, Tables, Output> {
    /// Joins `lateral` to the query with a `CROSS JOIN LATERAL`.
    pub fn cross_join_lateral<T: Table, Row: 'static>(
        mut self,
        lateral: Lateral<T, Row>,
    ) -> SelectQuery<D, CrossLateralJoined<Tables, T>, Output> {
        self.joins.push(JoinClause {
            kind: JoinKind::CrossLateral,
            on: None,
            lateral: Some(lateral.renderer),
        });
        self.retype()
    }

    /// Joins `lateral` to the query with a `LEFT JOIN LATERAL ... ON true`.
    pub fn left_join_lateral<T: Table, Row: 'static>(
        mut self,
        lateral: Lateral<T, Row>,
    ) -> SelectQuery<D, LeftLateralJoined<Tables, T>, Output> {
        self.joins.push(JoinClause {
            kind: JoinKind::LeftLateral,
            on: Some(ExprKind::Raw {
                fragments: vec!["true".to_owned()],
                params: Vec::new(),
            }),
            lateral: Some(lateral.renderer),
        });
        self.retype()
    }
}

impl<D: Dialect, Tables, Output: 'static> SelectQuery<D, Tables, Output> {
    /// Combines this query with `other` using `UNION`, removing duplicate rows.
    pub fn union<Tables2: TableScope + 'static>(
        self,
        other: SelectQuery<D, Tables2, Output>,
    ) -> Self {
        self.with_set_operation(SetOpKind::Union, other)
    }

    /// Combines this query with `other` using `UNION ALL`, keeping duplicates.
    pub fn union_all<Tables2: TableScope + 'static>(
        self,
        other: SelectQuery<D, Tables2, Output>,
    ) -> Self {
        self.with_set_operation(SetOpKind::UnionAll, other)
    }

    /// Keeps only the rows produced by both this query and `other` (`INTERSECT`).
    pub fn intersect<Tables2: TableScope + 'static>(
        self,
        other: SelectQuery<D, Tables2, Output>,
    ) -> Self {
        self.with_set_operation(SetOpKind::Intersect, other)
    }

    /// Keeps the rows of this query that are absent from `other` (`EXCEPT`).
    pub fn except<Tables2: TableScope + 'static>(
        self,
        other: SelectQuery<D, Tables2, Output>,
    ) -> Self {
        self.with_set_operation(SetOpKind::Except, other)
    }

    fn with_set_operation<Tables2: TableScope + 'static>(
        mut self,
        kind: SetOpKind,
        other: SelectQuery<D, Tables2, Output>,
    ) -> Self {
        self.set_operations.push(SetOperation {
            kind,
            render: Arc::new(move |sql, params, _relations| other.render_into(sql, params)),
        });
        self
    }
}

impl<D: Dialect, Tables: TableScope, Output> SelectQuery<D, Tables, Output> {
    /// Renders the query statement and parameters into the target buffers.
    pub fn render_into(&self, sql: &mut String, params: &mut Vec<SqlVal>) {
        let Some(render_projections) = self.projections.as_ref() else {
            debug_assert!(
                false,
                "a SELECT query requires at least one projection; call `select` before `build`"
            );
            return;
        };

        // Every clause of the statement is rendered against the same relation
        // list, so a column reference is qualified consistently across the
        // projection, the joins, the filter, and the ordering.
        let mut tables: Vec<TableRef> = Vec::new();
        Tables::push_table_refs(&mut tables);
        debug_assert_eq!(
            tables.len(),
            self.joins.len() + 1,
            "every joined table requires an ON condition"
        );

        if !self.ctes.is_empty() {
            sql.push_str("WITH ");
            for (index, cte) in self.ctes.iter().enumerate() {
                if index > 0 {
                    sql.push_str(", ");
                }
                D::push_quoted_identifier(sql, cte.name);
                sql.push_str(" AS (");
                (cte.renderer)(sql, params);
                sql.push(')');
            }
            sql.push(' ');
        }

        sql.push_str("SELECT ");
        if !self.distinct_on.is_empty() {
            sql.push_str("DISTINCT ON (");
            for (index, expression) in self.distinct_on.iter().enumerate() {
                if index > 0 {
                    sql.push_str(", ");
                }
                render_expr::<D>(sql, params, expression, &tables);
            }
            sql.push_str(") ");
        } else if self.distinct {
            sql.push_str("DISTINCT ");
        }
        render_projections(sql, params, &tables);

        sql.push_str(" FROM ");
        match tables.first() {
            Some(first) => first.render::<D>(sql),
            None => {
                debug_assert!(false, "a SELECT query requires a source table");
            }
        }

        for (index, join) in self.joins.iter().enumerate() {
            let Some(table) = tables.get(index + 1) else {
                debug_assert!(false, "every joined table requires an ON condition");
                break;
            };
            sql.push(' ');
            sql.push_str(join.kind.as_str());
            sql.push(' ');
            match &join.lateral {
                Some(render_lateral) => {
                    sql.push('(');
                    render_lateral(sql, params);
                    sql.push_str(") AS ");
                    D::push_quoted_identifier(sql, table.name);
                }
                None => table.render::<D>(sql),
            }
            if let Some(on) = &join.on {
                sql.push_str(" ON ");
                render_expr::<D>(sql, params, on, &tables);
            }
        }

        if let Some(filter) = &self.filter {
            sql.push_str(" WHERE ");
            render_expr::<D>(sql, params, filter, &tables);
        }

        if !self.group_by.is_empty() {
            sql.push_str(" GROUP BY ");
            for (index, expression) in self.group_by.iter().enumerate() {
                if index > 0 {
                    sql.push_str(", ");
                }
                render_expr::<D>(sql, params, expression, &tables);
            }
        }

        if let Some(having) = &self.having {
            sql.push_str(" HAVING ");
            render_expr::<D>(sql, params, having, &tables);
        }

        for operation in &self.set_operations {
            sql.push(' ');
            sql.push_str(operation.kind.as_str());
            sql.push(' ');
            (operation.render)(sql, params, &tables);
        }

        if !self.order_by.is_empty() {
            sql.push_str(" ORDER BY ");
            for (index, (expression, direction)) in self.order_by.iter().enumerate() {
                if index > 0 {
                    sql.push_str(", ");
                }
                render_expr::<D>(sql, params, expression, &tables);
                sql.push(' ');
                sql.push_str(direction.as_str());
            }
        }

        if let Some(limit) = self.limit {
            let _ = write!(sql, " LIMIT {limit}");
        }
        if let Some(offset) = self.offset {
            let _ = write!(sql, " OFFSET {offset}");
        }
    }

    /// Renders the query into SQL text and its ordered parameter list.
    pub fn build(&self) -> BuiltQuery {
        let mut sql = String::new();
        let mut params = Vec::new();
        self.render_into(&mut sql, &mut params);
        BuiltQuery { sql, params }
    }
}

impl<D: Dialect, Tables: TableScope + 'static, Output: 'static> SelectQuery<D, Tables, Output> {
    /// Wraps the query into a [`Subquery`] rendered with the enclosing statement's parameters.
    pub fn into_subquery(self) -> Subquery {
        Subquery::new(move |sql, params| self.render_into(sql, params))
    }

    /// Builds an `EXISTS (subquery)` predicate over this query.
    pub fn exists(&self) -> Expr<bool> {
        Expr::exists(self.clone())
    }

    /// Builds a `NOT EXISTS (subquery)` predicate over this query.
    pub fn not_exists(&self) -> Expr<bool> {
        Expr::not_exists(self.clone())
    }
}

impl<D: Dialect, Tables: TableScope + 'static, Output: 'static>
    From<SelectQuery<D, Tables, Output>> for Subquery
{
    fn from(query: SelectQuery<D, Tables, Output>) -> Self {
        query.into_subquery()
    }
}
