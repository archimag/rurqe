//! The type-safe `SELECT` builder and its single-pass SQL renderer.

use core::marker::PhantomData;

use crate::dialect::{Dialect, SupportsDistinctOn, SupportsLateralJoin};
use crate::expr::{
    BinaryOp, CteAst, DialectInScope, Expr, ExprKind, JoinAst, SelectAst, SqlVal, Subquery,
};
use crate::schema::{Column, ExprInScope, Table, TableInScope, TableRef};

// `ORDER BY` directions describe the statement AST shared with the expression
// module, so the canonical definition lives there and is re-exported here.
pub use crate::expr::OrderDirection;

use super::cte::{Cte, Lateral};
use super::group_by::{EmptyGroupSet, GroupBy, GroupByColumns, GroupByInScope, GroupedQuery};
use super::scope::{
    CrossLateralJoined, JoinKind, Joined, LeftJoined, LeftLateralJoined, ScopePositions,
    TableScope,
};
use super::selection::{Nullable, Selection, SelectionFor};
use super::set_ops::{SetOpAst, SetOpKind};

/// The rendered SQL text of a query together with its bound parameters.
#[derive(Debug, Clone, PartialEq)]
pub struct BuiltQuery {
    /// The SQL text using the dialect's positional placeholders.
    pub sql: String,

    /// The parameter values referenced by the placeholders, in placeholder order.
    pub params: Vec<SqlVal>,
}

/// Typestate marker indicating that a `SELECT` query records no projection yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoProjection;

/// Typestate marker indicating that a `SELECT` query records the rows it projects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HasProjection;

/// Statically asserts that a `SELECT` query records the rows it projects.
///
/// A query starts without projections, where rendering it would produce the
/// invalid `SELECT  FROM "users"` statement. Only [`SelectQuery::select`]
/// transitions the query into [`HasProjection`], the sole state implementing this
/// trait, so [`SelectQuery::build`], [`SelectQuery::to_ast`], and
/// [`SelectQuery::as_cte`] stay unreachable before a projection is given — in
/// debug and release builds alike.
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
/// // Fails to compile: the query records no projection yet.
/// let _ = SelectQuery::<MockDialect>::new().from::<User>().build();
/// ```
#[diagnostic::on_unimplemented(
    message = "refusing to build a SELECT query without a projection",
    label = "this query does not project any row yet",
    note = "call `.select(...)` with the columns to project before building, rendering, or embedding the query"
)]
pub trait ProjectionPresent {}

impl ProjectionPresent for HasProjection {}

/// Phantom marker carrying the dialect, scope, row type, and projection state of a query.
type QueryMarker<D, Tables, Output, Projected> =
    PhantomData<fn() -> (D, Tables, Output, Projected)>;

/// A single join clause recorded on a [`SelectQuery`].
///
/// The clause keeps the joined relation as dialect-neutral data, so the query
/// is described without a rendering closure and renders with the dialect of the
/// statement it belongs to.
#[derive(Clone, Debug, PartialEq)]
struct JoinClause {
    kind: JoinKind,
    on: Option<ExprKind>,
    lateral: Option<SelectAst>,
}

/// A `SELECT` query over the table `Tables` producing rows of type `Output`.
///
/// Every clause is accumulated as dialect-neutral statement data, so building a
/// query allocates no rendering closure and rendering it is a single pass over
/// the recorded statement. The ordering and pagination of a query chained with a
/// set operation are recorded separately: they apply to the combined rows rather
/// than to the left query.
///
/// `Projected` records whether the query already projects the rows it selects: a
/// fresh query carries [`NoProjection`], and [`SelectQuery::select`] moves it into
/// [`HasProjection`], the only state whose queries can be rendered. The parameter
/// defaults to [`NoProjection`], so `SelectQuery::<D>::new()` starts an unprojected
/// query while a finished query spells
/// `SelectQuery<D, Tables, Output, HasProjection>`.
pub struct SelectQuery<D: Dialect, Tables = (), Output = (), Projected = NoProjection> {
    ctes: Vec<CteAst>,
    projections: Vec<ExprKind>,
    joins: Vec<JoinClause>,
    filter: Option<ExprKind>,
    pub(crate) group_by: Vec<ExprKind>,
    having: Option<ExprKind>,
    order_by: Vec<(ExprKind, OrderDirection)>,
    limit: Option<u32>,
    offset: Option<u32>,
    set_operations: Vec<SetOpAst>,
    distinct: bool,
    distinct_on: Vec<ExprKind>,
    compound_order_by: Vec<(ExprKind, OrderDirection)>,
    compound_limit: Option<u32>,
    compound_offset: Option<u32>,
    _marker: QueryMarker<D, Tables, Output, Projected>,
}

impl<D: Dialect, Tables, Output, Projected> Clone for SelectQuery<D, Tables, Output, Projected> {
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
            compound_order_by: self.compound_order_by.clone(),
            compound_limit: self.compound_limit,
            compound_offset: self.compound_offset,
            _marker: PhantomData,
        }
    }
}

impl<D: Dialect> SelectQuery<D, (), (), NoProjection> {
    /// Creates a query without a table, projections, filter, or pagination.
    pub fn new() -> Self {
        SelectQuery {
            ctes: Vec::new(),
            projections: Vec::new(),
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
            compound_order_by: Vec::new(),
            compound_limit: None,
            compound_offset: None,
            _marker: PhantomData,
        }
    }
}

impl<D: Dialect> Default for SelectQuery<D, (), (), NoProjection> {
    fn default() -> Self {
        Self::new()
    }
}

impl<D: Dialect, Output, Projected> SelectQuery<D, (), Output, Projected> {
    /// Introduces `T` as the table rows are selected from.
    pub fn from<T: Table>(self) -> SelectQuery<D, T, Output, Projected> {
        self.retype()
    }

    /// Introduces `table` as the table rows are selected from.
    pub fn from_table<T: Table>(self, _table: T) -> SelectQuery<D, T, Output, Projected> {
        self.retype()
    }
}

impl<D: Dialect, Tables, Output, Projected> SelectQuery<D, Tables, Output, Projected> {
    /// Selects `selection`, fixing the row type of the query to the selection row.
    ///
    /// The selection is captured as dialect-neutral AST nodes at the call site,
    /// so the query keeps its projection as plain data and renders it with the
    /// dialect of the statement, exactly like the filter, the joins, and the
    /// ordering it already carries. Recording a projection moves the query into
    /// [`HasProjection`], the only state where it can be rendered.
    pub fn select<S, Positions>(
        mut self,
        selection: S,
    ) -> SelectQuery<D, Tables, <S as SelectionFor<Tables, Positions>>::Row, HasProjection>
    where
        S: SelectionFor<Tables, Positions>,
    {
        let mut projections = Vec::new();
        Selection::push_projections(&selection, &mut projections);
        self.projections = projections;
        self.retype()
    }

    /// Adds a Common Table Expression (`WITH cte AS (...)`) to this query.
    ///
    /// The expression was built for the dialect `CteD`, which must be the
    /// dialect of this statement or dialect-neutral, so a CTE of another engine
    /// does not compile into the query.
    pub fn with<T, Row, CteD>(mut self, cte: Cte<T, Row, CteD>) -> Self
    where
        CteD: DialectInScope<D>,
    {
        self.ctes.push(CteAst {
            name: cte.name,
            query: cte.query,
        });
        self
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
    ) -> GroupedQuery<D, Tables, Output, <G as GroupByColumns<EmptyGroupSet>>::Columns, Projected>
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
    ///
    /// An ordering written before a set operation belongs to the left query and
    /// is rendered inside its parentheses; an ordering written after one applies
    /// to the combined rows.
    pub fn order_by<C: Column, P>(mut self, _col: C, direction: OrderDirection) -> Self
    where
        Tables: TableInScope<C::Table, P>,
    {
        let criterion = (C::expr_kind(), direction);
        if self.set_operations.is_empty() {
            self.order_by.push(criterion);
        } else {
            self.compound_order_by.push(criterion);
        }
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

    /// Joins the table `T` to the query with an `INNER JOIN` restricted by `on`.
    pub fn inner_join<T: Table, Scope, P>(
        self,
        on: Expr<bool, Scope>,
    ) -> SelectQuery<D, Joined<Tables, T>, Output, Projected>
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
    ) -> SelectQuery<D, Joined<Tables, T>, Output, Projected>
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
    ) -> SelectQuery<D, LeftJoined<Tables, T>, Output, Projected>
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
    ) -> SelectQuery<D, LeftJoined<Tables, T>, Output, Projected>
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

    fn retype<D2: Dialect, Tables2, Output2, Projected2>(
        self,
    ) -> SelectQuery<D2, Tables2, Output2, Projected2> {
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
            compound_order_by: self.compound_order_by,
            compound_limit: self.compound_limit,
            compound_offset: self.compound_offset,
            _marker: PhantomData,
        }
    }
}

impl<D: Dialect + SupportsLateralJoin, Tables: TableScope, Output, Projected>
    SelectQuery<D, Tables, Output, Projected>
{
    /// Joins `lateral` to the query with a `CROSS JOIN LATERAL`.
    pub fn cross_join_lateral<T: Table, Row, LatD>(
        mut self,
        lateral: Lateral<T, Row, LatD>,
    ) -> SelectQuery<D, CrossLateralJoined<Tables, T>, Output, Projected>
    where
        LatD: DialectInScope<D>,
    {
        self.joins.push(JoinClause {
            kind: JoinKind::CrossLateral,
            on: None,
            lateral: Some(lateral.query),
        });
        self.retype()
    }

    /// Joins `lateral` to the query with a `LEFT JOIN LATERAL ... ON true`.
    pub fn left_join_lateral<T: Table, Row, LatD>(
        mut self,
        lateral: Lateral<T, Row, LatD>,
    ) -> SelectQuery<D, LeftLateralJoined<Tables, T>, Output, Projected>
    where
        LatD: DialectInScope<D>,
    {
        self.joins.push(JoinClause {
            kind: JoinKind::LeftLateral,
            on: Some(ExprKind::Raw {
                fragments: vec!["true".to_owned()],
                params: Vec::new(),
            }),
            lateral: Some(lateral.query),
        });
        self.retype()
    }
}

impl<D: Dialect, Tables, Output> SelectQuery<D, Tables, Output, HasProjection> {
    /// Combines this query with `other` using `UNION`, removing duplicate rows.
    pub fn union<Tables2: TableScope>(
        self,
        other: SelectQuery<D, Tables2, Output, HasProjection>,
    ) -> Self {
        self.with_set_operation(SetOpKind::Union, other)
    }

    /// Combines this query with `other` using `UNION ALL`, keeping duplicates.
    pub fn union_all<Tables2: TableScope>(
        self,
        other: SelectQuery<D, Tables2, Output, HasProjection>,
    ) -> Self {
        self.with_set_operation(SetOpKind::UnionAll, other)
    }

    /// Keeps only the rows produced by both this query and `other` (`INTERSECT`).
    pub fn intersect<Tables2: TableScope>(
        self,
        other: SelectQuery<D, Tables2, Output, HasProjection>,
    ) -> Self {
        self.with_set_operation(SetOpKind::Intersect, other)
    }

    /// Keeps the rows of this query that are absent from `other` (`EXCEPT`).
    pub fn except<Tables2: TableScope>(
        self,
        other: SelectQuery<D, Tables2, Output, HasProjection>,
    ) -> Self {
        self.with_set_operation(SetOpKind::Except, other)
    }

    fn with_set_operation<Tables2: TableScope>(
        mut self,
        kind: SetOpKind,
        other: SelectQuery<D, Tables2, Output, HasProjection>,
    ) -> Self {
        self.set_operations.push(SetOpAst {
            kind,
            query: other.to_ast(),
        });
        self
    }
}

impl<D: Dialect, Tables: TableScope, Output, P: ProjectionPresent>
    SelectQuery<D, Tables, Output, P>
{
    /// Describes the query as a dialect-neutral statement AST.
    ///
    /// The relations of the statement follow the scope of the query: the table
    /// the rows are selected from comes first, and every join appends the relation
    /// it introduced in the same order. Joins are paired with the relations of the
    /// scope instead of being indexed by them, so a join without a matching
    /// relation is dropped rather than aborting the renderer.
    pub fn to_ast(&self) -> SelectAst {
        let mut tables: Vec<TableRef> = Vec::new();
        Tables::push_table_refs(&mut tables);

        let joins = tables
            .iter()
            .skip(1)
            .copied()
            .zip(self.joins.iter())
            .map(|(table, join)| JoinAst {
                kind: join.kind,
                table,
                lateral: join.lateral.clone(),
                on: join.on.clone(),
            })
            .collect();

        SelectAst {
            ctes: self.ctes.clone(),
            distinct: self.distinct,
            distinct_on: self.distinct_on.clone(),
            projections: self.projections.clone(),
            from: tables.first().copied(),
            joins,
            filter: self.filter.clone(),
            group_by: self.group_by.clone(),
            having: self.having.clone(),
            order_by: self.order_by.clone(),
            limit: self.limit,
            offset: self.offset,
            set_operations: self.set_operations.clone(),
            compound_order_by: self.compound_order_by.clone(),
            compound_limit: self.compound_limit,
            compound_offset: self.compound_offset,
        }
    }

    /// Renders the query statement and parameters into the target buffers.
    pub fn render_into(&self, sql: &mut String, params: &mut Vec<SqlVal>) {
        self.to_ast().render::<D>(sql, params);
    }

    /// Renders the query into SQL text and its ordered parameter list.
    ///
    /// The method is reachable only once the query records a projection, so a
    /// query rendered without `select` is rejected at compile time in debug and
    /// release builds alike; see [`ProjectionPresent`].
    pub fn build(&self) -> BuiltQuery {
        let mut sql = String::new();
        let mut params = Vec::new();
        self.render_into(&mut sql, &mut params);
        BuiltQuery { sql, params }
    }

    /// Wraps the query into a [`Subquery`] carrying this statement as plain data.
    pub fn into_subquery(self) -> Subquery {
        Subquery::new(self.to_ast())
    }

    /// Builds an `EXISTS (subquery)` predicate over this query.
    pub fn exists(&self) -> Expr<bool> {
        Expr::exists(Subquery::new(self.to_ast()))
    }

    /// Builds a `NOT EXISTS (subquery)` predicate over this query.
    pub fn not_exists(&self) -> Expr<bool> {
        Expr::not_exists(Subquery::new(self.to_ast()))
    }
}

impl<D: Dialect, Tables: TableScope, Output> SelectQuery<D, Tables, Output, HasProjection> {
    /// Converts this query into a CTE for the table `T`.
    pub fn as_cte<T: Table>(self) -> Cte<T, Output, D> {
        Cte::new(self)
    }

    /// Converts this query into a named CTE with alias `name`.
    pub fn as_named_cte(self, name: &'static str) -> Cte<(), Output, D> {
        Cte::named(name, self)
    }

    /// Converts this query into a `LATERAL` subquery exposed under the table marker `T`.
    pub fn as_lateral<T: Table>(self) -> Lateral<T, Output, D> {
        Lateral::new(self)
    }
}

impl<D: Dialect, Tables: TableScope, Output> From<SelectQuery<D, Tables, Output, HasProjection>>
    for Subquery
{
    fn from(query: SelectQuery<D, Tables, Output, HasProjection>) -> Self {
        query.into_subquery()
    }
}
