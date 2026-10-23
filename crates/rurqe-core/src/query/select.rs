//! The type-safe `SELECT` builder and its single-pass SQL renderer.

use core::marker::PhantomData;

use crate::dialect::{
    Dialect, SupportsDistinctOn, SupportsFullOuterJoin, SupportsLateralJoin, SupportsRowLocking,
};
use crate::expr::{
    BinaryOp, CteAst, DerivedAst, DialectInScope, Expr, ExprKind, JoinAst, LockClause,
    LockStrength, SelectAst, SqlVal, StatementRef, Subquery,
};
use crate::schema::{Column, ExprInScope, Table, TableInScope, TableRef};

// `ORDER BY` criteria describe the statement AST shared with the expression
// module, so the canonical definitions live there and are re-exported here.
pub use crate::expr::{NullsOrder, OrderCriterion, OrderDirection};

use super::cte::{Cte, Lateral, DerivedTable};
use super::group_by::{EmptyGroupSet, GroupBy, GroupByColumns, GroupByInScope, GroupedQuery};
use super::scope::{
    CrossJoined, CrossLateralJoined, FullOuterJoined, JoinKind, Joined, LeftJoined,
    LeftLateralJoined, ScopePositions, TableScope,
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

/// Typestate marker indicating that a `SELECT` query requests no row-level lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoLock;

/// Typestate marker indicating that a `SELECT` query requests a row-level lock.
///
/// The state is reached through [`SelectQuery::for_update`] or
/// [`SelectQuery::for_share`], the only builders of the marker, so a locking
/// clause can neither be requested twice nor modified before it exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Locked;

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
type QueryMarker<D, Tables, Output, Projected, Lock> =
    PhantomData<fn() -> (D, Tables, Output, Projected, Lock)>;

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
///
/// `Lock` records whether the query requests a row-level lock: a plain query
/// carries [`NoLock`], and [`SelectQuery::for_update`] or
/// [`SelectQuery::for_share`] move it into [`Locked`], where the closing
/// [`SelectQuery::nowait`] and [`SelectQuery::skip_locked`] modifiers stay
/// reachable. The requested clause is copied into the rendered statement, so a
/// locked query executes like any other.
pub struct SelectQuery<D: Dialect, Tables = (), Output = (), Projected = NoProjection, Lock = NoLock>
{
    ctes: Vec<CteAst>,
    from: Option<TableRef>,
    projections: Vec<ExprKind>,
    joins: Vec<JoinAst>,
    derived: Vec<DerivedAst>,
    filter: Option<ExprKind>,
    pub(crate) group_by: Vec<ExprKind>,
    having: Option<ExprKind>,
    order_by: Vec<OrderCriterion>,
    limit: Option<u32>,
    offset: Option<u32>,
    set_operations: Vec<SetOpAst>,
    distinct: bool,
    distinct_on: Vec<ExprKind>,
    compound_order_by: Vec<OrderCriterion>,
    compound_limit: Option<u32>,
    compound_offset: Option<u32>,
    lock: Option<LockClause>,
    _marker: QueryMarker<D, Tables, Output, Projected, Lock>,
}

impl<D: Dialect, Tables, Output, Projected, Lock> Clone
    for SelectQuery<D, Tables, Output, Projected, Lock>
{
    fn clone(&self) -> Self {
        Self {
            ctes: self.ctes.clone(),
            from: self.from,
            projections: self.projections.clone(),
            joins: self.joins.clone(),
            derived: self.derived.clone(),
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
            lock: self.lock,
            _marker: PhantomData,
        }
    }
}

impl<D: Dialect> SelectQuery<D, (), (), NoProjection> {
    /// Creates a query without a table, projections, filter, or pagination.
    pub fn new() -> Self {
        SelectQuery {
            ctes: Vec::new(),
            from: None,
            projections: Vec::new(),
            joins: Vec::new(),
            derived: Vec::new(),
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
            lock: None,
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
    pub fn from<T: Table>(mut self) -> SelectQuery<D, T, Output, Projected> {
        self.from = Some(T::table_ref());
        self.retype()
    }

    /// Introduces `table` as the table rows are selected from.
    pub fn from_table<T: Table>(mut self, _table: T) -> SelectQuery<D, T, Output, Projected> {
        self.from = Some(T::table_ref());
        self.retype()
    }

    /// Introduces the derived table `derived` as the relation rows are selected from.
    ///
    /// The query the derived table wraps is rendered inline as
    /// `(SELECT ...) AS "name"`, so the statement selects from its rows without a
    /// temporary relation, while the columns of the subquery are referenced
    /// through the `table!` marker the derived table was built with:
    ///
    /// ```text
    /// SELECT "user_id", "order_count"
    /// FROM (SELECT "user_id", COUNT(*) FROM "orders" GROUP BY "user_id") AS "user_summary"
    /// ```
    pub fn from_subquery<T: Table, Row, SubD>(
        mut self,
        derived: DerivedTable<T, Row, SubD>,
    ) -> SelectQuery<D, T, Output, Projected>
    where
        SubD: DialectInScope<D>,
    {
        self.from = Some(T::table_ref());
        self.derived.push(derived.into_ast());
        self.retype()
    }
}

impl<D: Dialect, Tables> SelectQuery<D, Tables, (), NoProjection, NoLock> {
    /// Groups query results by `columns`, moving the query into the
    /// [`GroupedQuery`] state.
    ///
    /// `group_by` must be established before projections are selected so that
    /// every projected column is verified to be either grouped or an aggregate.
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
    /// // Fails to compile: calling `group_by` after `select` is rejected.
    /// let _ = SelectQuery::<MockDialect>::new()
    ///     .from::<User>()
    ///     .select((UserId,))
    ///     .group_by(UserId);
    /// ```
    pub fn group_by<G, Positions>(
        mut self,
        columns: G,
    ) -> GroupedQuery<D, Tables, (), <G as GroupByColumns<EmptyGroupSet>>::Columns, NoProjection>
    where
        G: GroupBy + GroupByInScope<Tables, Positions> + GroupByColumns<EmptyGroupSet>,
    {
        columns.push_column_names(&mut self.group_by);
        GroupedQuery {
            query: self,
            _marker: PhantomData,
        }
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
    ///
    /// The query must already select from a table, so the row type is derived
    /// from the scope the projection is read from: a column of a `LEFT JOIN`ed
    /// table reaches the row as `Option<T>`. Selecting before the first `from`
    /// is therefore rejected at compile time, which also rules out a projection
    /// fixed before an outer join could lift it; see [`TableScope`].
    pub fn select<S, Positions>(
        mut self,
        selection: S,
    ) -> SelectQuery<D, Tables, <S as SelectionFor<Tables, Positions>>::Row, HasProjection>
    where
        Tables: TableScope,
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
        self.push_filter(predicate.into_kind());
        self
    }

    /// Restricts the query to rows matching `predicate` when one is given.
    ///
    /// A `SELECT` records its filter as statement data rather than as a typestate,
    /// so an absent predicate leaves the query unchanged and the returned query
    /// type stays the same. The method is the entry point for a filter assembled
    /// from optional caller input:
    ///
    /// ```text
    /// let query = select_from(Users)
    ///     .select(cols!(UsersId))
    ///     .where_opt(search.map(|search| UsersName.eq(search)));
    /// ```
    pub fn where_opt<Scope, P>(self, predicate: Option<Expr<bool, Scope>>) -> Self
    where
        Scope: ExprInScope<Tables, P> + ScopePositions<P>,
    {
        match predicate {
            Some(predicate) => self.where_(predicate),
            None => self,
        }
    }

    /// Applies `build` to the query when `condition` holds.
    ///
    /// The closure returns the query it receives, so it restricts, orders, or
    /// paginates the statement, while a clause that changes the query type — a
    /// projection, a join, or a row lock — stays outside the conditional:
    ///
    /// ```text
    /// let query = select_from(Users)
    ///     .select(cols!(UsersId))
    ///     .when(only_active, |query| query.where_(UsersActive.eq(true)))
    ///     .when(page_size > 0, |query| query.limit(page_size));
    /// ```
    pub fn when(self, condition: bool, build: impl FnOnce(Self) -> Self) -> Self {
        if condition {
            build(self)
        } else {
            self
        }
    }

    /// Applies `build` to the query when `value` is present, handing it over.
    ///
    /// ```text
    /// let query = select_from(Users)
    ///     .select(cols!(UsersId))
    ///     .if_some(name, |query, name| query.where_(UsersName.eq(name)));
    /// ```
    pub fn if_some<T>(self, value: Option<T>, build: impl FnOnce(Self, T) -> Self) -> Self {
        match value {
            Some(value) => build(self, value),
            None => self,
        }
    }

    /// Merges `predicate` into the recorded filter with a boolean `AND`.
    fn push_filter(&mut self, predicate: ExprKind) {
        let filter = match self.filter.take() {
            Some(existing) => ExprKind::Binary {
                op: BinaryOp::And,
                lhs: Box::new(existing),
                rhs: Box::new(predicate),
            },
            None => predicate,
        };
        self.filter = Some(filter);
    }

    /// Restricts grouped rows to those satisfying `predicate`.
    ///
    /// A `HAVING` clause filters the groups an aggregate produced, so the
    /// predicate is written over an aggregate — `count_all().gt(5i64)`, for
    /// instance — and keeps the aggregation marker of that comparison.
    pub fn having<Scope, Aggr, P>(mut self, predicate: Expr<bool, Scope, Aggr>) -> Self
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
        self.push_ordering(OrderCriterion::new(C::expr_kind(), direction));
        self
    }

    /// Appends an ordering criterion fixing where the NULL values of `col` sort.
    ///
    /// The placement follows the direction, so a page over a nullable column keeps
    /// its order on every engine instead of following the default of the one that
    /// serves the statement.
    pub fn order_by_nulls<C: Column, P>(
        mut self,
        _col: C,
        direction: OrderDirection,
        nulls: NullsOrder,
    ) -> Self
    where
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
        Tables: TableInScope<C::Table, P>,
    {
        self.order_by_nulls(col, OrderDirection::Asc, NullsOrder::First)
    }

    /// Appends an ascending ordering criterion putting NULL values last.
    pub fn order_by_asc_nulls_last<C: Column, P>(self, col: C) -> Self
    where
        Tables: TableInScope<C::Table, P>,
    {
        self.order_by_nulls(col, OrderDirection::Asc, NullsOrder::Last)
    }

    /// Appends a descending ordering criterion putting NULL values first.
    pub fn order_by_desc_nulls_first<C: Column, P>(self, col: C) -> Self
    where
        Tables: TableInScope<C::Table, P>,
    {
        self.order_by_nulls(col, OrderDirection::Desc, NullsOrder::First)
    }

    /// Appends a descending ordering criterion putting NULL values last.
    pub fn order_by_desc_nulls_last<C: Column, P>(self, col: C) -> Self
    where
        Tables: TableInScope<C::Table, P>,
    {
        self.order_by_nulls(col, OrderDirection::Desc, NullsOrder::Last)
    }

    /// Records `criterion` on the clause the query currently orders with.
    ///
    /// An ordering written after a set operation belongs to the combined rows
    /// rather than to the left query, so it is recorded on the compound statement.
    fn push_ordering(&mut self, criterion: OrderCriterion) {
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

    /// Joins the table `T` to the query with an `INNER JOIN` restricted by `on`.
    pub fn inner_join<T: Table, Scope, P>(
        self,
        on: Expr<bool, Scope>,
    ) -> SelectQuery<D, Joined<Tables, T>, Output, Projected>
    where
        Tables: TableScope,
        Scope: ExprInScope<Joined<Tables, T>, P> + ScopePositions<P>,
    {
        self.push_join(JoinKind::Inner, T::table_ref(), None, Some(on.into_kind()))
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
        self.push_join(JoinKind::Inner, T::table_ref(), None, Some(on.into_kind()))
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
        self.push_join(JoinKind::Left, T::table_ref(), None, Some(on.into_kind()))
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
        self.push_join(JoinKind::Left, T::table_ref(), None, Some(on.into_kind()))
            .retype()
    }

    /// Joins the table `T` to the query with an unconditional `CROSS JOIN`.
    pub fn cross_join<T: Table>(
        self,
    ) -> SelectQuery<D, CrossJoined<Tables, T>, Output, Projected>
    where
        Tables: TableScope,
    {
        self.push_join(JoinKind::Cross, T::table_ref(), None, None)
            .retype()
    }

    /// Joins `table` to the query with an unconditional `CROSS JOIN`.
    pub fn cross_join_table<T: Table>(
        self,
        _table: T,
    ) -> SelectQuery<D, CrossJoined<Tables, T>, Output, Projected>
    where
        Tables: TableScope,
    {
        self.push_join(JoinKind::Cross, T::table_ref(), None, None)
            .retype()
    }

    /// Joins the table `T` to the query with a `FULL OUTER JOIN` restricted by `on`.
    pub fn full_outer_join<T: Table, Scope, P>(
        self,
        on: Expr<bool, Scope>,
    ) -> SelectQuery<D, FullOuterJoined<Tables, T>, Output, Projected>
    where
        D: SupportsFullOuterJoin,
        Tables: TableScope,
        Scope: ExprInScope<FullOuterJoined<Tables, T>, P> + ScopePositions<P>,
    {
        self.push_join(
            JoinKind::FullOuter,
            T::table_ref(),
            None,
            Some(on.into_kind()),
        )
        .retype()
    }

    /// Joins `table` to the query with a `FULL OUTER JOIN` restricted by `on`.
    pub fn full_outer_join_table<T: Table, Scope, P>(
        self,
        _table: T,
        on: Expr<bool, Scope>,
    ) -> SelectQuery<D, FullOuterJoined<Tables, T>, Output, Projected>
    where
        D: SupportsFullOuterJoin,
        Tables: TableScope,
        Scope: ExprInScope<FullOuterJoined<Tables, T>, P> + ScopePositions<P>,
    {
        self.push_join(
            JoinKind::FullOuter,
            T::table_ref(),
            None,
            Some(on.into_kind()),
        )
        .retype()
    }

    /// Joins the derived table `derived` to the query with an `INNER JOIN`.
    ///
    /// The query the derived table wraps is rendered inline as
    /// `(SELECT ...) AS "name"`, so the joined relation behaves like a stored one
    /// while its rows come from the subquery:
    ///
    /// ```text
    /// SELECT "users"."id", "user_summary"."order_count" FROM "users"
    /// INNER JOIN (SELECT "user_id", COUNT(*) FROM "orders" GROUP BY "user_id")
    ///     AS "user_summary" ON ("user_summary"."user_id" = "users"."id")
    /// ```
    pub fn inner_join_subquery<T: Table, Row, SubD, Scope, P>(
        mut self,
        derived: DerivedTable<T, Row, SubD>,
        on: Expr<bool, Scope>,
    ) -> SelectQuery<D, Joined<Tables, T>, Output, Projected>
    where
        Tables: TableScope,
        SubD: DialectInScope<D>,
        Scope: ExprInScope<Joined<Tables, T>, P> + ScopePositions<P>,
    {
        self.derived.push(derived.into_ast());
        self.push_join(JoinKind::Inner, T::table_ref(), None, Some(on.into_kind()))
            .retype()
    }

    /// Joins the derived table `derived` to the query with a `LEFT JOIN`.
    ///
    /// An outer join leaves the rows of the derived table unmatched whenever the
    /// condition does not hold, so its columns reach the projected row as
    /// `Option<T>` exactly like those of a left-joined stored table.
    pub fn left_join_subquery<T: Table, Row, SubD, Scope, P>(
        mut self,
        derived: DerivedTable<T, Row, SubD>,
        on: Expr<bool, Scope>,
    ) -> SelectQuery<D, LeftJoined<Tables, T>, Output, Projected>
    where
        Tables: TableScope,
        SubD: DialectInScope<D>,
        Scope: ExprInScope<LeftJoined<Tables, T>, P> + ScopePositions<P>,
    {
        self.derived.push(derived.into_ast());
        self.push_join(JoinKind::Left, T::table_ref(), None, Some(on.into_kind()))
            .retype()
    }

    /// Records a regular join clause whose relational effect is expressed by the
    /// caller's return type.
    fn push_join(
        mut self,
        kind: JoinKind,
        table: TableRef,
        lateral: Option<SelectAst>,
        on: Option<ExprKind>,
    ) -> Self {
        self.joins.push(JoinAst {
            kind,
            table,
            lateral,
            on,
        });
        self
    }

    fn retype<D2: Dialect, Tables2, Output2, Projected2, Lock2>(
        self,
    ) -> SelectQuery<D2, Tables2, Output2, Projected2, Lock2> {
        SelectQuery {
            ctes: self.ctes,
            from: self.from,
            projections: self.projections,
            joins: self.joins,
            derived: self.derived,
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
            lock: self.lock,
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
        self.joins.push(JoinAst {
            kind: JoinKind::CrossLateral,
            table: T::table_ref(),
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
        self.joins.push(JoinAst {
            kind: JoinKind::LeftLateral,
            table: T::table_ref(),
            on: Some(ExprKind::Raw {
                fragments: vec!["true".to_owned()],
                params: Vec::new(),
            }),
            lateral: Some(lateral.query),
        });
        self.retype()
    }
}

impl<D: Dialect + SupportsRowLocking, Tables, Output>
    SelectQuery<D, Tables, Output, HasProjection>
{
    /// Locks the selected rows for the rest of the transaction (`FOR UPDATE`).
    ///
    /// The clause closes the statement, so it is requested once the query
    /// projects its rows; the resulting [`Locked`] state then keeps the closing
    /// [`nowait`](SelectQuery::nowait) and [`skip_locked`](SelectQuery::skip_locked)
    /// modifiers available. Row-level locking is a dialect capability, so a query
    /// built for a dialect without it — SQLite, which locks the whole database
    /// instead — does not compile.
    ///
    /// ```
    /// use rurqe_core::dialect::{Dialect, SupportsRowLocking};
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
    /// impl SupportsRowLocking for MockDialect {}
    ///
    /// rurqe_core::table! {
    ///     /// The `jobs` table.
    ///     Job => "jobs" {
    ///         JobId => "id": i64,
    ///     }
    /// }
    ///
    /// let built = SelectQuery::<MockDialect>::new()
    ///     .from::<Job>()
    ///     .select((JobId,))
    ///     .limit(1)
    ///     .for_update()
    ///     .skip_locked()
    ///     .build();
    ///
    /// assert_eq!(
    ///     built.sql,
    ///     "SELECT \"id\" FROM \"jobs\" LIMIT 1 FOR UPDATE SKIP LOCKED"
    /// );
    /// ```
    pub fn for_update(mut self) -> SelectQuery<D, Tables, Output, HasProjection, Locked> {
        self.lock = Some(LockClause::new(LockStrength::Update));
        self.retype()
    }

    /// Locks the selected rows in shared mode (`FOR SHARE`).
    ///
    /// Concurrent transactions may still read and share-lock the rows, but none
    /// of them can update or delete them until this transaction ends.
    ///
    /// ```
    /// use rurqe_core::expr::Expr;
    /// use rurqe_core::query::SelectQuery;
    ///
    /// struct MockDialect;
    ///
    /// impl rurqe_core::dialect::Dialect for MockDialect {
    ///     fn push_quoted_identifier(out: &mut String, ident: &str) {
    ///         out.push('"');
    ///         out.push_str(ident);
    ///         out.push('"');
    ///     }
    ///
    ///     fn push_placeholder(out: &mut String, index: usize) {
    ///         out.push('$');
    ///         out.push_str(&index.to_string());
    ///     }
    /// }
    ///
    /// impl rurqe_core::dialect::SupportsRowLocking for MockDialect {}
    ///
    /// rurqe_core::table! {
    ///     /// The `jobs` table.
    ///     Job => "jobs" {
    ///         JobId => "id": i64,
    ///     }
    /// }
    ///
    /// let built = SelectQuery::<MockDialect>::new()
    ///     .from::<Job>()
    ///     .select((JobId,))
    ///     .where_(Expr::column::<JobId>().eq(7i64))
    ///     .for_share()
    ///     .nowait()
    ///     .build();
    ///
    /// assert_eq!(
    ///     built.sql,
    ///     "SELECT \"id\" FROM \"jobs\" WHERE (\"id\" = $1) FOR SHARE NOWAIT"
    /// );
    /// ```
    pub fn for_share(mut self) -> SelectQuery<D, Tables, Output, HasProjection, Locked> {
        self.lock = Some(LockClause::new(LockStrength::Share));
        self.retype()
    }
}

impl<D: Dialect + SupportsRowLocking, Tables, Output>
    SelectQuery<D, Tables, Output, HasProjection, Locked>
{
    /// Fails instead of waiting for rows locked by concurrent transactions (`NOWAIT`).
    ///
    /// The method modifies the lock the query already requests, so it stays
    /// unreachable until [`SelectQuery::for_update`] or
    /// [`SelectQuery::for_share`] was called.
    pub fn nowait(mut self) -> Self {
        self.lock = self.lock.map(LockClause::nowait);
        self
    }

    /// Skips rows locked by concurrent transactions (`SKIP LOCKED`).
    ///
    /// This is how competing workers partition a queue of pending rows between
    /// themselves: none of them waits for a row another worker already holds.
    pub fn skip_locked(mut self) -> Self {
        self.lock = self.lock.map(LockClause::skip_locked);
        self
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

impl<D: Dialect, Tables: TableScope, Output, P: ProjectionPresent, Lock>
    SelectQuery<D, Tables, Output, P, Lock>
{
    fn as_statement_ref(&self) -> StatementRef<'_> {
        let from = self.from.or_else(|| {
            let mut tables: Vec<TableRef> = Vec::new();
            Tables::push_table_refs(&mut tables);
            tables.first().copied()
        });

        StatementRef {
            ctes: &self.ctes,
            distinct: self.distinct,
            distinct_on: &self.distinct_on,
            projections: &self.projections,
            projection_aliases: None,
            from,
            joins: &self.joins,
            derived: &self.derived,
            filter: self.filter.as_ref(),
            group_by: &self.group_by,
            having: self.having.as_ref(),
            order_by: &self.order_by,
            limit: self.limit,
            offset: self.offset,
            set_operations: &self.set_operations,
            compound_order_by: &self.compound_order_by,
            compound_limit: self.compound_limit,
            compound_offset: self.compound_offset,
            lock: self.lock,
        }
    }

    /// Describes the query as a dialect-neutral statement AST.
    pub fn to_ast(&self) -> SelectAst {
        let from = self.from.or_else(|| {
            let mut tables: Vec<TableRef> = Vec::new();
            Tables::push_table_refs(&mut tables);
            tables.first().copied()
        });

        SelectAst {
            ctes: self.ctes.clone(),
            distinct: self.distinct,
            distinct_on: self.distinct_on.clone(),
            projections: self.projections.clone(),
            from,
            joins: self.joins.clone(),
            derived: self.derived.clone(),
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
            lock: self.lock,
        }
    }

    /// Renders the query statement and parameters into the target buffers.
    pub fn render_into(&self, sql: &mut String, params: &mut Vec<SqlVal>) {
        self.as_statement_ref().render::<D>(sql, params);
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

    /// Exposes this query as a derived table referenced through the marker `T`.
    ///
    /// The marker declares the columns of the projected rows, which the caller
    /// states by choosing it: the relation is then referenced like a stored table
    /// — `"name"."column"` — while the database materialises nothing.
    pub fn as_table<T: Table>(self) -> DerivedTable<T, Output, D> {
        DerivedTable::new(self)
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
