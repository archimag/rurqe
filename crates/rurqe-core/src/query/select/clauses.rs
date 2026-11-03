//! Clause builders of the `SELECT` query.
//!
//! Every builder records one clause into the statement the query accumulates and
//! restates the typestate parameters the clause changed, so the rendered statement
//! is assembled from plain data instead of from a rendering closure.

use core::marker::PhantomData;

use crate::dialect::{Dialect, SupportsDistinctOn};
use crate::expr::{BinaryOp, CteAst, DialectInScope, Expr, ExprKind, NonAggregate};
use crate::query::cte::Cte;
use crate::query::group_by::{
    EmptyGroupSet, GroupBy, GroupByColumns, GroupByInScope, GroupedQuery,
};
use crate::query::scope::{ScopePositions, TableScope};
use crate::query::selection::{AggregateInDialect, Nullable, Selection, SelectionFor};
use crate::schema::{Column, ExprInScope, TableInScope};

use super::{HasProjection, LockUnavailable, NoLock, NoProjection, ProjectionOpen, SelectQuery};

impl<D: Dialect, Tables> SelectQuery<D, Tables, (), NoProjection, NoLock> {
    /// Groups query results by `columns`, moving the query into the
    /// [`GroupedQuery`] state.
    ///
    /// `group_by` must be established before projections are selected so that
    /// every projected column is verified to be either grouped or an aggregate.
    /// The grouped query is not lockable: PostgreSQL evaluates a row-level lock on
    /// the rows the statement produced, while a grouped statement produces one row
    /// per group, so the [`GroupedQuery`] state keeps the locking builders out of
    /// reach.
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
    /// rurqe_core::table!(
    ///     User => "users" {
    ///         UserId => "id": i64,
    ///     }
    /// );
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

impl<D: Dialect, Tables, Output, Projected> SelectQuery<D, Tables, Output, Projected, NoLock> {
    /// Marks the query as `DISTINCT`, removing duplicate rows.
    ///
    /// Every expression of the `ORDER BY` clause of a `DISTINCT` statement has to
    /// appear in the projection list as well, because the engine sorts the rows it
    /// produced rather than the rows it read; PostgreSQL rejects the statement
    /// otherwise. The rule compares the assembled expressions rather than their
    /// types, so it is validated during `.build()` and surfaces as
    /// [`BuildError::DistinctOrderNotProjected`](crate::query::BuildError::DistinctOrderNotProjected);
    /// the related `DISTINCT ON (...)` prefix rule is validated there as well and
    /// surfaces as
    /// [`BuildError::DistinctOnOrderMismatch`](crate::query::BuildError::DistinctOnOrderMismatch).
    ///
    /// A deduplicated statement cannot request a row-level lock, so the query
    /// moves out of [`NoLock`] and [`SelectQuery::for_update`] and
    /// [`SelectQuery::for_share`] stop applying to it:
    ///
    /// A statement requests one deduplication flavour. `distinct_on` records the
    /// other one on the state this call leaves the query in, so
    /// `.distinct().distinct_on(...)` compiles, and `.build()` rejects the pair as
    /// [`DistinctWithDistinctOn`](crate::query::BuildError::DistinctWithDistinctOn):
    /// the statement renders a single prefix before its projection list, so the
    /// whole-row deduplication recorded here would otherwise disappear without a
    /// diagnostic.
    ///
    /// ```compile_fail
    /// use rurqe_core::dialect::{Dialect, SupportsRowLocking};
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
    /// impl SupportsRowLocking for MockDialect {}
    ///
    /// rurqe_core::table!(
    ///     User => "users" {
    ///         UserId => "id": i64,
    ///     }
    /// );
    ///
    /// fn main() {
    ///     // Fails to compile: a deduplicated statement cannot be locked.
    ///     let _ = SelectQuery::<MockDialect>::new()
    ///         .from::<User>()
    ///         .select((UserId,))
    ///         .distinct()
    ///         .for_update();
    /// }
    /// ```
    pub fn distinct(mut self) -> SelectQuery<D, Tables, Output, Projected, LockUnavailable> {
        self.distinct = true;
        self.lock_unavailable()
    }
}

impl<D: Dialect, Tables, Output, Projected> SelectQuery<D, Tables, Output, Projected, NoLock> {
    /// Appends a column to the `DISTINCT ON (...)` clause.
    ///
    /// `DISTINCT ON` is a dialect-specific feature supported by PostgreSQL and
    /// gated by [`SupportsDistinctOn`]. The expressions of the clause have to
    /// match the leading expressions of the `ORDER BY` clause, because that
    /// ordering decides which of the duplicate rows survives; PostgreSQL rejects
    /// the statement otherwise. The rule compares the assembled expressions
    /// rather than their types, so it is checked during `.build()` and surfaces
    /// as [`BuildError::DistinctOnOrderMismatch`](crate::query::BuildError::DistinctOnOrderMismatch).
    ///
    /// A deduplicated statement cannot request a row-level lock: PostgreSQL
    /// refuses `FOR UPDATE` and `FOR SHARE` on a statement that keeps one row per
    /// group, because the lock claims the rows the statement read while the
    /// deduplication leaves one of them. The method is therefore reachable only
    /// from [`NoLock`], and it moves the query into [`LockUnavailable`], where
    /// [`SelectQuery::for_update`] and [`SelectQuery::for_share`] stop applying to
    /// it — a query that already locked its rows cannot start deduplicating them
    /// either. The pair is additionally rejected by `.build()` as
    /// [`BuildError::DistinctOnWithLock`](crate::query::BuildError::DistinctOnWithLock),
    /// so a statement assembled from AST data cannot reach the engine as well.
    ///
    /// ```compile_fail
    /// use rurqe_core::dialect::{Dialect, SupportsDistinctOn, SupportsRowLocking};
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
    /// impl SupportsDistinctOn for MockDialect {}
    ///
    /// impl SupportsRowLocking for MockDialect {}
    ///
    /// rurqe_core::table!(
    ///     User => "users" {
    ///         UserId => "id": i64,
    ///     }
    /// );
    ///
    /// fn main() {
    ///     // Fails to compile: a statement that already locked its rows cannot
    ///     // start deduplicating them, because PostgreSQL refuses the pair.
    ///     let _ = SelectQuery::<MockDialect>::new()
    ///         .from::<User>()
    ///         .select((UserId,))
    ///         .for_update()
    ///         .distinct_on(UserId);
    /// }
    /// ```
    pub fn distinct_on<C: Column, P>(
        mut self,
        _col: C,
    ) -> SelectQuery<D, Tables, Output, Projected, LockUnavailable>
    where
        D: SupportsDistinctOn,
        Tables: TableInScope<C::Table, P>,
    {
        self.distinct_on.push(C::expr_kind());
        self.lock_unavailable()
    }
}

impl<D: Dialect + SupportsDistinctOn, Tables, Output, Projected>
    SelectQuery<D, Tables, Output, Projected, LockUnavailable>
{
    /// Appends another expression to the `DISTINCT ON (...)` clause.
    ///
    /// A statement deduplicates its rows on every expression of the clause — one
    /// row per `(user_id, tenant_id)` pair, for instance — so the criteria
    /// accumulate. The first criterion moves the query into [`LockUnavailable`],
    /// and recording a further one keeps it there: the statement stays
    /// deduplicated, which is exactly what keeps [`SelectQuery::for_update`] and
    /// [`SelectQuery::for_share`] out of reach. PostgreSQL refuses
    /// `SELECT DISTINCT ON (...) ... FOR UPDATE`, because the lock claims the rows
    /// the statement read while the deduplication keeps one row per group, and a
    /// statement assembled from AST data is rejected during `.build()` as
    /// [`BuildError::DistinctOnWithLock`](crate::query::BuildError::DistinctOnWithLock)
    /// as well.
    ///
    /// This is also the state [`SelectQuery::distinct`] leaves the query in, so a
    /// statement that already selected `DISTINCT` can record a `DISTINCT ON`
    /// criterion beside it. The two clauses deduplicate different things — the whole
    /// projected row against one row per named group — and the statement renders a
    /// single prefix before its projection list, so `.build()` rejects the pair as
    /// [`DistinctWithDistinctOn`](crate::query::BuildError::DistinctWithDistinctOn)
    /// instead of dropping the whole-row deduplication.
    ///
    /// ```compile_fail
    /// use rurqe_core::dialect::{Dialect, SupportsDistinctOn, SupportsRowLocking};
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
    /// impl SupportsDistinctOn for MockDialect {}
    ///
    /// impl SupportsRowLocking for MockDialect {}
    ///
    /// rurqe_core::table!(
    ///     User => "users" {
    ///         UserId => "id": i64,
    ///         UserTenant => "tenant_id": i64,
    ///     }
    /// );
    ///
    /// fn main() {
    ///     // Fails to compile: extending the DISTINCT ON clause keeps the
    ///     // statement deduplicated, so it cannot start locking its rows.
    ///     let _ = SelectQuery::<MockDialect>::new()
    ///         .from::<User>()
    ///         .select((UserId,))
    ///         .distinct_on(UserId)
    ///         .distinct_on(UserTenant)
    ///         .for_update();
    /// }
    /// ```
    pub fn distinct_on<C: Column, P>(mut self, _col: C) -> Self
    where
        Tables: TableInScope<C::Table, P>,
    {
        self.distinct_on.push(C::expr_kind());
        self
    }
}

impl<D: Dialect, Tables, Output, Projected, Lock: ProjectionOpen>
    SelectQuery<D, Tables, Output, Projected, Lock>
{
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
    /// table reaches the row as `Option<T>`.
    ///
    /// A statement records its projection once. After `UNION`, `INTERSECT`, or
    /// `EXCEPT` the output columns are the ones the first projection list named —
    /// the operands were combined on that understanding — so the query carries
    /// [`SetOpApplied`](super::SetOpApplied), [`ProjectionOpen`] stays unimplemented
    /// for it, and the call is rejected instead of rebinding the left operand behind
    /// the back of the operands already combined:
    ///
    /// ```compile_fail
    /// use rurqe_core::dialect::Dialect;
    /// use rurqe_core::query::{HasProjection, SelectQuery};
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
    /// rurqe_core::table!(
    ///     User => "users" {
    ///         UserId => "id": i64,
    ///     }
    /// );
    ///
    /// fn base() -> SelectQuery<MockDialect, User, (i64,), HasProjection> {
    ///     SelectQuery::<MockDialect>::new()
    ///         .from::<User>()
    ///         .select((UserId,))
    /// }
    ///
    /// fn main() {
    ///     // Fails to compile: the set operation already fixed the rows the
    ///     // combined statement produces, so `select` cannot rebind them.
    ///     let _ = base().union(base()).select((UserId,));
    /// }
    /// ```
    pub fn select<S, Positions>(
        mut self,
        selection: S,
    ) -> SelectQuery<D, Tables, <S as SelectionFor<Tables, Positions, D>>::Row, HasProjection, Lock>
    where
        Tables: TableScope,
        S: SelectionFor<Tables, Positions, D>,
    {
        let mut projections = Vec::new();
        Selection::push_projections(&selection, &mut projections);
        self.projections = projections;
        self.with_projection()
    }
}

impl<D: Dialect, Tables, Output, Projected, Lock> SelectQuery<D, Tables, Output, Projected, Lock> {
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

    /// Lifts the column `C` into its nullable form [`Nullable`].
    pub fn nullable<C: Column>(&self) -> Nullable<C> {
        Nullable::new()
    }

    /// Restricts the query to rows matching `predicate`.
    pub fn where_<Scope, P, PredD>(
        mut self,
        predicate: Expr<bool, Scope, NonAggregate, PredD>,
    ) -> Self
    where
        Scope: ExprInScope<Tables, P> + ScopePositions<P>,
        PredD: DialectInScope<D>,
    {
        self.push_filter(predicate.into_kind());
        self
    }

    /// Restricts the query to rows matching `predicate` when one is given.
    ///
    /// A `SELECT` records its filter as statement data rather than as a typestate,
    /// so an absent predicate leaves the query unchanged and the returned query
    /// type stays the same.
    pub fn where_opt<Scope, P, PredD>(
        self,
        predicate: Option<Expr<bool, Scope, NonAggregate, PredD>>,
    ) -> Self
    where
        Scope: ExprInScope<Tables, P> + ScopePositions<P>,
        PredD: DialectInScope<D>,
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
    /// projection, a join, or a row lock — stays outside the conditional.
    pub fn when(self, condition: bool, build: impl FnOnce(Self) -> Self) -> Self {
        if condition {
            build(self)
        } else {
            self
        }
    }

    /// Applies `build` to the query when `value` is present, handing it over.
    pub fn if_some<T>(self, value: Option<T>, build: impl FnOnce(Self, T) -> Self) -> Self {
        match value {
            Some(value) => build(self, value),
            None => self,
        }
    }

    /// Merges `predicate` into the recorded filter with a boolean `AND`.
    pub(crate) fn push_filter(&mut self, predicate: ExprKind) {
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

    /// Records `predicate` as the `HAVING` clause of the statement.
    ///
    /// A `HAVING` clause filters the groups an aggregate produced, so the
    /// predicate is written over an aggregate — `count_all().gt(5i64)`, for
    /// instance — and keeps the aggregation marker of that comparison.
    ///
    /// Only a grouped statement has groups to filter, so the clause is recorded
    /// through [`GroupedQuery::having`](crate::query::GroupedQuery::having) and
    /// [`GroupedQuery::having_col`](crate::query::GroupedQuery::having_col): the
    /// rules that make a predicate valid read the grouping the query recorded, and
    /// that grouping is known only while the query carries the grouped typestate.
    /// The method therefore stays crate-private, which keeps an ungrouped query
    /// from rendering a `HAVING` clause the engine would reject.
    pub(crate) fn having<Scope, Aggr, P, PredD>(
        mut self,
        predicate: Expr<bool, Scope, Aggr, PredD>,
    ) -> Self
    where
        Scope: ExprInScope<Tables, P> + ScopePositions<P>,
        PredD: DialectInScope<D>,
        Aggr: AggregateInDialect<D>,
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
}
