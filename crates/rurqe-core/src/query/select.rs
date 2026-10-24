//! The type-safe `SELECT` builder and its single-pass SQL renderer.

use core::marker::PhantomData;

use crate::dialect::{Dialect, SupportsDistinctOn};
use crate::expr::{
    BinaryOp, CteAst, DerivedAst, DialectInScope, Expr, ExprKind, JoinAst, LockClause,
    NonAggregate, SqlVal,
};
use crate::schema::{Column, ExprInScope, Table, TableInScope, TableRef};

// `ORDER BY` criteria describe the statement AST shared with the expression
// module, so the canonical definitions live there and are re-exported here.
pub use crate::expr::{NullsOrder, OrderCriterion, OrderDirection};

use super::cte::{Cte, DerivedTable};
use super::group_by::{EmptyGroupSet, GroupBy, GroupByColumns, GroupByInScope, GroupedQuery};
use super::scope::{ScopePositions, TableScope};
use super::selection::{AggregateInDialect, Nullable, Selection, SelectionFor};
use super::set_ops::SetOpAst;

// The builder is decomposed by responsibility: joins establish the relational
// scope, ordering and pagination move rows of the clause they are written on,
// locking and set operations change the typestate of the statement, and the
// rendering adapters convert the query into its statement AST.
mod joins;
mod locking;
mod ordering;
mod render;
mod set_ops;

pub use self::ordering::OrderBy;

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

/// Typestate marker indicating that a `SELECT` query cannot request a row-level lock.
///
/// PostgreSQL refuses `FOR UPDATE` and `FOR SHARE` on a statement that
/// deduplicates its rows (`DISTINCT`, `DISTINCT ON`) or combines several
/// statements (`UNION`, `INTERSECT`, `EXCEPT`), so recording one of those clauses
/// moves the query out of [`NoLock`] and leaves the locking builders unreachable.
/// Grouping moves the query out of the locking path as well: a grouped query
/// reaches [`SelectQuery`] again only through
/// [`GroupedQuery::into_select`](crate::query::GroupedQuery::into_select), which
/// returns an unlocked query carrying this marker.
///
/// Combining the query with a set operation also moves it into the
/// [`Compound`](crate::query::Compound) scope, where the ordering of the combined
/// rows names output columns and positions instead of relations in scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LockUnavailable;

/// Typestate marker indicating that a compound statement recorded its ordering tail.
///
/// A statement combined with `UNION`, `INTERSECT`, or `EXCEPT` sorts the rows the
/// combination produced, so its ordering belongs to the whole result rather than
/// to one operand. SQL binds it there: no operand can follow it, and
/// `A UNION B ORDER BY ... UNION C` is rejected by the engine.
///
/// The state is reached from the set-operation path — `order_by_index` and
/// `order_by_output` move the query here — and it stays reachable for every
/// further criterion, so a compound statement still orders by as many output
/// columns as it projects. What the state rules out is another operand:
/// [`SetOperand`] has no implementation for it, which leaves `union`,
/// `union_all`, `intersect`, and `except` unreachable.
///
/// ```text
/// first.union(second).order_by_index_asc(1).union(third) // rejected
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TailClosed;

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

/// Statically asserts that a `SELECT` query may be combined with another statement.
///
/// A set operation compares the rows of several queries, so the combined
/// statement can neither keep the row-level lock of an operand nor accept a new
/// one: a query that already requests a lock carries [`Locked`] and does not
/// implement this trait.
///
/// A compound statement that already recorded the ordering of its combined rows
/// carries [`TailClosed`] for the same reason: SQL binds that ordering to the
/// whole result, so another operand would have to be combined before it instead
/// of after it.
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
/// rurqe_core::table! {
///     User => "users" {
///         UserId => "id": i64,
///     }
/// }
///
/// fn base() -> SelectQuery<MockDialect, User, (i64,), HasProjection> {
///     SelectQuery::<MockDialect>::new()
///         .from::<User>()
///         .select((UserId,))
/// }
///
/// fn main() {
///     // Fails to compile: the ordering of the combined rows already exists, so no
///     // operand may follow it.
///     let _ = base()
///         .union(base())
///         .order_by_output_asc(UserId)
///         .union(base());
/// }
/// ```
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot be combined with another statement",
    label = "this statement requests a row-level lock or already ordered its combined rows",
    note = "remove the `FOR UPDATE` or `FOR SHARE` clause before combining the statement with `UNION`, `INTERSECT`, or `EXCEPT`",
    note = "a compound statement that already recorded the ordering of its combined rows takes no further operand, because SQL binds that ordering to the whole result"
)]
pub trait SetOperand {}

impl SetOperand for NoLock {}

impl SetOperand for LockUnavailable {}

/// A `SELECT` query that projects its rows and requests no row-level lock.
///
/// The alias names the everyday shape of a rendered statement, hiding the
/// projection and lock typestates [`SelectQuery`] carries: every query reaches
/// [`HasProjection`] through [`SelectQuery::select`] and keeps [`NoLock`] unless
/// it requests a row-level lock.
///
/// ```text
/// let query: Query<Postgres, Users, (i64, String)> = ...
/// ```
pub type Query<D, Tables = (), Output = ()> = SelectQuery<D, Tables, Output, HasProjection, NoLock>;

/// A `SELECT` query that projects its rows and requests a row-level lock.
///
/// The alias names a statement inside a transaction that claims its rows until
/// the transaction ends, hiding the projection and lock typestates
/// [`SelectQuery`] carries: it is reached after [`SelectQuery::for_update`] or
/// [`SelectQuery::for_share`] moved the query into [`Locked`].
///
/// ```text
/// let query: LockedQuery<Postgres, Users, (i64,)> = ...
/// ```
pub type LockedQuery<D, Tables = (), Output = ()> =
    SelectQuery<D, Tables, Output, HasProjection, Locked>;

/// Phantom marker carrying the dialect, scope, row type, and typestates of a query.
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
/// [`HasProjection`], the only state whose queries can be rendered.
///
/// `Lock` records whether the query requests a row-level lock: a plain query
/// carries [`NoLock`], and [`SelectQuery::for_update`] or
/// [`SelectQuery::for_share`] move it into [`Locked`], where the closing
/// [`SelectQuery::nowait`] and [`SelectQuery::skip_locked`] modifiers stay
/// reachable. A query that cannot be locked carries [`LockUnavailable`] instead:
/// deduplicating its rows with [`SelectQuery::distinct`] or
/// [`SelectQuery::distinct_on`], grouping it, or combining it with
/// [`SelectQuery::union`] and the other set operations moves it out of [`NoLock`],
/// because PostgreSQL refuses a row-level lock on such a statement.
pub struct SelectQuery<
    D: Dialect,
    Tables = (),
    Output = (),
    Projected = NoProjection,
    Lock = NoLock,
> {
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
    /// through the `table!` marker the derived table was built with.
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
    /// The grouped query is not lockable: PostgreSQL evaluates a row-level lock on
    /// the rows the statement produced, while a grouped statement produces one row
    /// per group, so [`GroupedQuery::into_select`](crate::query::GroupedQuery::into_select)
    /// hands the query back with [`LockUnavailable`].
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

impl<D: Dialect, Tables, Output, Projected> SelectQuery<D, Tables, Output, Projected, NoLock> {
    /// Marks the query as `DISTINCT`, removing duplicate rows.
    ///
    /// Every expression of the `ORDER BY` clause of a `DISTINCT` statement has to
    /// appear in the projection list as well, because the engine sorts the rows it
    /// produced rather than the rows it read; PostgreSQL rejects the statement
    /// otherwise. The rule compares the projected expressions rather than their
    /// types, so it is documented here instead of being enforced by the builder;
    /// the related `DISTINCT ON (...)` prefix rule is validated during `.build()`
    /// and surfaces as
    /// [`BuildError::DistinctOnOrderMismatch`](crate::query::BuildError::DistinctOnOrderMismatch).
    ///
    /// A deduplicated statement cannot request a row-level lock, so the query
    /// moves out of [`NoLock`] and [`SelectQuery::for_update`] and
    /// [`SelectQuery::for_share`] stop applying to it:
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
    /// rurqe_core::table! {
    ///     User => "users" {
    ///         UserId => "id": i64,
    ///     }
    /// }
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
        self.retype()
    }
}

impl<D: Dialect, Tables, Output, Projected, Lock> SelectQuery<D, Tables, Output, Projected, Lock> {
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
    /// A deduplicated statement cannot request a row-level lock, so the query
    /// moves out of [`NoLock`] and the locking builders stop applying to it.
    pub fn distinct_on<C: Column, P>(
        mut self,
        _col: C,
    ) -> SelectQuery<D, Tables, Output, Projected, LockUnavailable>
    where
        D: SupportsDistinctOn,
        Tables: TableInScope<C::Table, P>,
    {
        self.distinct_on.push(C::expr_kind());
        self.retype()
    }
}

impl<D: Dialect, Tables, Output, Projected, Lock> SelectQuery<D, Tables, Output, Projected, Lock> {
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
    pub fn select<S, Positions>(
        mut self,
        selection: S,
    ) -> SelectQuery<
        D,
        Tables,
        <S as SelectionFor<Tables, Positions, D>>::Row,
        HasProjection,
        Lock,
    >
    where
        Tables: TableScope,
        S: SelectionFor<Tables, Positions, D>,
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
    pub fn having<Scope, Aggr, P, PredD>(
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

    /// Reinterprets the recorded statement under another set of typestates.
    ///
    /// The statement data is untouched, so the method only renames the phantom
    /// parameters the callers want to reach: the grouped query builder hands a
    /// query back with [`LockUnavailable`] through it.
    pub(super) fn retype<D2: Dialect, Tables2, Output2, Projected2, Lock2>(
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
